#![cfg(feature = "findesk")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aionui_api_types::FinclawBuildExtra;
use aionui_common::{
    AgentKillReason, AgentType, Confirmation, ConfirmationOption, ConversationStatus, ProviderWithModel, decrypt_string,
};
use aionui_db::IProviderRepository;
use aionui_findesk::FindeskConfig;
use aionui_findesk::finclaw::{
    FinclawApprovalRequired, FinclawGatewayConfig, FinclawInferEvent, FinclawToolProgressEvent,
    apply_tool_policy_from_extra,
    build_finclaw_llm_config_slice, build_finclaw_model_fingerprint, decision_from_confirm_data,
    ensure_finclaw_conversation_profile, ensure_finclaw_workspace_profile, post_infer_stream,
    prepare_finclaw_llm_for_profiles,
    resolve_finclaw_infer_capability, resolve_finclaw_serve_cwd, shared_gateway_pool, submit_approval_resolve,
};
use futures_util::{StreamExt, pin_mut};
use reqwest::Client;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, Notify, oneshot};
use tracing::info;

use crate::agent_runtime::AgentRuntime;
use crate::agent_task::IAgentTask;
use crate::error::AgentError;
use crate::protocol::events::{
    AcpToolCallContentItem, AcpToolCallEventData, AcpToolCallKind, AcpToolCallSessionUpdateKind, AcpToolCallStatus,
    AcpToolCallTextBlock, AcpToolCallTextBlockType, AcpToolCallUpdateData,
    AcpPermissionEventData, AcpPermissionOptionData, AcpPermissionOptionKind, AcpPermissionRequestData,
    AcpPermissionToolCall, AgentStreamEvent, TextEventData, ThinkingEventData,
};
use crate::protocol::send_error::AgentSendError;
use crate::types::SendMessageData;

struct PendingApproval {
    approval: FinclawApprovalRequired,
    decision_tx: oneshot::Sender<()>,
}

pub struct FinclawAgentManager {
    runtime: AgentRuntime,
    gateway_profile: String,
    gateway_cwd: PathBuf,
    gateway: Arc<aionui_findesk::finclaw::FinclawGateway>,
    config: FinclawBuildExtra,
    http: Client,
    cancel_notify: Arc<Notify>,
    pending_approvals: Arc<Mutex<HashMap<String, PendingApproval>>>,
}

impl FinclawAgentManager {
    pub async fn new(
        conversation_id: String,
        workspace: String,
        config: FinclawBuildExtra,
        model: ProviderWithModel,
        provider_repo: Arc<dyn IProviderRepository>,
        encryption_key: [u8; 32],
    ) -> Result<Self, AgentError> {
        let findesk = FindeskConfig::from_env();
        let base_profile = config
            .finclaw_profile
            .as_deref()
            .filter(|p| !p.is_empty())
            .unwrap_or("default");
        let serve_cwd = resolve_finclaw_serve_cwd(&workspace);
        let derived_profile =
            ensure_finclaw_workspace_profile(base_profile, &serve_cwd).map_err(AgentError::bad_request)?;
        let serve_profile = ensure_finclaw_conversation_profile(&derived_profile, &conversation_id)
            .map_err(AgentError::bad_request)?;

        apply_tool_policy_from_extra(&serve_cwd, config.finclaw_tool_policy.as_deref())
            .map_err(AgentError::bad_request)?;

        if model.provider_id.trim().is_empty() {
            return Err(AgentError::bad_request(
                "FinClaw requires a model from FinDesk settings. Configure a provider in Settings → Models, then retry.",
            ));
        }

        let row = provider_repo
            .find_by_id(&model.provider_id)
            .await
            .map_err(|e| AgentError::internal(format!("Failed to load provider config: {e}")))?
            .ok_or_else(|| AgentError::bad_request(format!("Provider '{}' not found", model.provider_id)))?;

        let api_key =
            decrypt_string(&row.api_key_encrypted, &encryption_key).map_err(|e| AgentError::internal(e.to_string()))?;

        let model_id = model
            .use_model
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(&model.model)
            .to_owned();

        let llm_slice = build_finclaw_llm_config_slice(
            &row.platform,
            &row.base_url,
            &model_id,
            &api_key,
            row.model_protocols.as_deref(),
        );
        let llm_serve_env = prepare_finclaw_llm_for_profiles(
            &findesk,
            config.cli_path.as_deref(),
            base_profile,
            &serve_profile,
            &llm_slice,
        )
        .await
        .map_err(AgentError::bad_request)?;

        let gateway_config = FinclawGatewayConfig {
            cli_path: config.cli_path.clone(),
            profile: serve_profile.clone(),
            security_mode: config.finclaw_security_mode.clone(),
            serve_cwd: serve_cwd.clone(),
            llm_serve_env,
            model_fingerprint: Some(build_finclaw_model_fingerprint(&llm_slice)),
        };

        let pool = shared_gateway_pool();
        let (gateway, _) = pool.acquire(gateway_config).await;
        gateway.ensure_started().await.map_err(AgentError::bad_request)?;

        Ok(Self {
            runtime: AgentRuntime::new(conversation_id, workspace, 128),
            gateway_profile: serve_profile,
            gateway_cwd: serve_cwd,
            gateway,
            config,
            http: Client::new(),
            cancel_notify: Arc::new(Notify::new()),
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn resolve_user_id(&self) -> String {
        self.config
            .user_id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    pub fn get_confirmations(&self) -> Vec<Confirmation> {
        self.pending_approvals
            .try_lock()
            .map(|pending| {
                pending
                    .values()
                    .map(|item| approval_to_confirmation(&item.approval))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn confirm(&self, _msg_id: &str, call_id: &str, data: Value, _always_allow: bool) -> Result<(), AgentError> {
        let item = self
            .pending_approvals
            .try_lock()
            .map_err(|_| AgentError::internal("FinClaw approval lock poisoned"))?
            .remove(call_id);

        let Some(item) = item else {
            return Err(AgentError::bad_request(format!(
                "No pending FinClaw approval for call_id '{call_id}'"
            )));
        };

        let port = self
            .gateway
            .claw_port()
            .ok_or_else(|| AgentError::bad_request("FinClaw claw_port is not available"))?;
        let decision = decision_from_confirm_data(&data);
        let user_id = self.resolve_user_id();
        let approval = item.approval.clone();
        let http = self.http.clone();
        let session_id = self.conversation_id().to_string();
        let reason = data.get("reason").and_then(Value::as_str).map(str::to_string);

        let _ = item.decision_tx.send(());

        tokio::spawn(async move {
            let _ = submit_approval_resolve(
                &http,
                port,
                &user_id,
                &session_id,
                &approval,
                decision,
                reason.as_deref(),
            )
            .await;
        });

        Ok(())
    }

    async fn handle_approval_required(&self, approval: FinclawApprovalRequired) -> Result<(), AgentError> {
        let call_id = approval
            .tool_call_id
            .clone()
            .unwrap_or_else(|| approval.approval_request_id.clone());
        let (decision_tx, decision_rx) = oneshot::channel();

        {
            let mut pending = self.pending_approvals.lock().await;
            pending.insert(
                call_id.clone(),
                PendingApproval {
                    approval: approval.clone(),
                    decision_tx,
                },
            );
        }

        self.runtime
            .emit(AgentStreamEvent::AcpPermission(AcpPermissionEventData::Request(
                AcpPermissionRequestData {
                    session_id: self.conversation_id().to_string(),
                    tool_call: AcpPermissionToolCall {
                        tool_call_id: call_id,
                        status: None,
                        title: Some(approval.tool_name.clone()),
                        kind: None,
                        raw_input: approval.arguments.clone(),
                        raw_output: None,
                        content: None,
                        locations: None,
                        meta: None,
                    },
                    options: vec![
                        AcpPermissionOptionData {
                            option_id: "allow_once".into(),
                            name: "Allow".into(),
                            kind: AcpPermissionOptionKind::AllowOnce,
                            meta: None,
                        },
                        AcpPermissionOptionData {
                            option_id: "reject_once".into(),
                            name: "Reject".into(),
                            kind: AcpPermissionOptionKind::RejectOnce,
                            meta: None,
                        },
                    ],
                    meta: Some({
                        let mut meta = Map::new();
                        meta.insert("finclawApprovalRequestId".into(), json!(approval.approval_request_id));
                        if let Some(run_id) = &approval.run_id {
                            meta.insert("finclawRunId".into(), json!(run_id));
                        }
                        meta
                    }),
                },
            )));

        tokio::select! {
            _ = decision_rx => Ok(()),
            _ = self.cancel_notify.notified() => {
                self.pending_approvals.lock().await.clear();
                Err(AgentError::bad_request("FinClaw approval cancelled"))
            }
        }
    }
}

fn approval_to_confirmation(approval: &FinclawApprovalRequired) -> Confirmation {
    let call_id = approval
        .tool_call_id
        .clone()
        .unwrap_or_else(|| approval.approval_request_id.clone());
    Confirmation {
        id: call_id.clone(),
        call_id,
        title: Some(approval.tool_name.clone()),
        action: Some(approval.tool_name.clone()),
        description: approval
            .reason
            .clone()
            .or_else(|| approval.arguments.as_ref().map(|args| args.to_string()))
            .unwrap_or_else(|| approval.tool_name.clone()),
        command_type: None,
        options: vec![
            ConfirmationOption {
                label: "Allow".into(),
                value: json!("allow_once"),
                params: None,
            },
            ConfirmationOption {
                label: "Reject".into(),
                value: json!("reject_once"),
                params: None,
            },
        ],
    }
}

fn map_finclaw_tool_status(status: Option<&str>) -> AcpToolCallStatus {
    let normalized = status.unwrap_or("in_progress").to_ascii_lowercase();
    match normalized.as_str() {
        "pending" | "queued" => AcpToolCallStatus::Pending,
        "completed" | "done" | "success" | "succeeded" => AcpToolCallStatus::Completed,
        "failed" | "error" | "cancelled" | "canceled" | "rejected" => AcpToolCallStatus::Failed,
        _ => AcpToolCallStatus::InProgress,
    }
}

fn map_finclaw_tool_kind(kind: Option<&str>, title: Option<&str>) -> AcpToolCallKind {
    let normalized = kind
        .or(title)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "execute".to_string());
    if normalized.contains("read") || normalized.contains("search") || normalized.contains("glob") {
        AcpToolCallKind::Read
    } else if normalized.contains("edit")
        || normalized.contains("write")
        || normalized.contains("patch")
        || normalized.contains("replace")
    {
        AcpToolCallKind::Edit
    } else {
        AcpToolCallKind::Execute
    }
}

fn map_finclaw_tool_progress(
    event: FinclawToolProgressEvent,
    session_id: &str,
    is_first: bool,
) -> AcpToolCallEventData {
    let status = Some(map_finclaw_tool_status(event.status.as_deref()));
    let title = Some(
        event
            .title
            .clone()
            .unwrap_or_else(|| event.custom_name.clone()),
    );
    let kind = Some(map_finclaw_tool_kind(event.kind.as_deref(), event.title.as_deref()));
    let content = event.content_text.map(|text| {
        vec![AcpToolCallContentItem::Content {
            content: AcpToolCallTextBlock {
                block_type: AcpToolCallTextBlockType::Text,
                text,
            },
        }]
    });

    AcpToolCallEventData {
        session_id: session_id.to_string(),
        update: AcpToolCallUpdateData {
            session_update: if is_first {
                AcpToolCallSessionUpdateKind::ToolCall
            } else {
                AcpToolCallSessionUpdateKind::ToolCallUpdate
            },
            tool_call_id: event.tool_call_id,
            status,
            title,
            kind,
            raw_input: event.raw_input,
            raw_output: event.raw_output,
            content,
            locations: None,
        },
        meta: None,
    }
}

#[async_trait::async_trait]
impl IAgentTask for FinclawAgentManager {
    fn agent_type(&self) -> AgentType {
        AgentType::Finclaw
    }

    fn conversation_id(&self) -> &str {
        self.runtime.conversation_id()
    }

    fn workspace(&self) -> &str {
        self.runtime.workspace()
    }

    fn status(&self) -> Option<ConversationStatus> {
        self.runtime.status()
    }

    fn last_activity_at(&self) -> i64 {
        self.runtime.last_activity_at()
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentStreamEvent> {
        self.runtime.subscribe()
    }

    async fn send_message(&self, data: SendMessageData) -> Result<(), AgentSendError> {
        let port = self.gateway.claw_port().ok_or_else(|| {
            AgentSendError::from_agent_error(AgentError::bad_request("FinClaw claw_port is not available"))
        })?;

        info!(
            conversation_id = %self.conversation_id(),
            msg_id = %data.msg_id,
            "FinClaw send_message started"
        );
        self.runtime.bump_activity();
        self.runtime.reset_for_new_turn(ConversationStatus::Running);

        let user_id = self.resolve_user_id();
        let capability = resolve_finclaw_infer_capability(self.config.session_mode.as_deref()).to_string();
        let max_tokens = self.config.max_tokens;

        let stream = post_infer_stream(
            &self.http,
            port,
            self.conversation_id(),
            &user_id,
            &data.content,
            &capability,
            max_tokens,
        )
        .await
        .map_err(|e| AgentSendError::from_agent_error(AgentError::bad_gateway(e)))?;
        pin_mut!(stream);
        let mut seen_tool_calls: std::collections::HashSet<String> = std::collections::HashSet::new();

        let send_result = tokio::select! {
            result = async {
                while let Some(item) = stream.next().await {
                    let event = item.map_err(AgentError::bad_gateway)?;
                    match event {
                        FinclawInferEvent::TextChunk(delta) => {
                            self.runtime.emit(AgentStreamEvent::Text(TextEventData { content: delta }));
                        }
                        FinclawInferEvent::ThinkingChunk(delta) => {
                            self.runtime.emit(AgentStreamEvent::Thinking(ThinkingEventData {
                                content: delta,
                                subject: None,
                                duration: None,
                                status: Some("thinking".into()),
                            }));
                        }
                        FinclawInferEvent::ToolProgress(tool_progress) => {
                            let tool_call_id = tool_progress.tool_call_id.clone();
                            let custom_name = tool_progress.custom_name.clone();
                            let title = tool_progress.title.clone().unwrap_or_default();
                            let status = tool_progress.status.clone().unwrap_or_default();
                            let kind = tool_progress.kind.clone().unwrap_or_default();
                            let has_raw_input = tool_progress.raw_input.is_some();
                            let has_raw_output = tool_progress.raw_output.is_some();
                            let has_content_text = tool_progress.content_text.is_some();
                            let is_first = seen_tool_calls.insert(tool_call_id.clone());
                            let is_fallback_call_id = tool_call_id.starts_with("finclaw:");

                            info!(
                                conversation_id = %self.conversation_id(),
                                tool_event = %custom_name,
                                tool_call_id = %tool_call_id,
                                is_first,
                                is_fallback_call_id,
                                status = %status,
                                kind = %kind,
                                title = %title,
                                has_raw_input,
                                has_raw_output,
                                has_content_text,
                                "FinClaw tool event mapped to AcpToolCall"
                            );

                            self.runtime.emit(AgentStreamEvent::AcpToolCall(map_finclaw_tool_progress(
                                tool_progress,
                                self.conversation_id(),
                                is_first,
                            )));
                        }
                        FinclawInferEvent::ApprovalRequired(approval) => {
                            self.handle_approval_required(approval).await?;
                        }
                        FinclawInferEvent::Finished => {
                            info!(
                                conversation_id = %self.conversation_id(),
                                "FinClaw infer finished from terminal SSE"
                            );
                            self.runtime.emit_finish(None);
                            return Ok(());
                        }
                        FinclawInferEvent::Error(message) => {
                            self.runtime.emit_error(message);
                            return Ok(());
                        }
                    }
                }
                info!(
                    conversation_id = %self.conversation_id(),
                    "FinClaw infer stream ended without terminal SSE; emitting synthetic finish"
                );
                self.runtime.emit_finish(None);
                Ok(())
            } => result,
            _ = self.cancel_notify.notified() => {
                info!(conversation_id = %self.conversation_id(), "FinClaw infer cancelled");
                self.pending_approvals.lock().await.clear();
                // Ensure stream relay receives a terminal event on stop so
                // runtime_state releases active_turn_id immediately.
                self.runtime.emit_finish(None);
                Ok(())
            }
        };

        self.runtime.bump_activity();
        send_result.map_err(AgentSendError::from_agent_error)
    }

    async fn cancel(&self) -> Result<(), AgentError> {
        self.cancel_notify.notify_waiters();
        self.pending_approvals.lock().await.clear();
        // Idempotent: if send_message already emitted Finish, this is a no-op.
        self.runtime.emit_finish(None);
        Ok(())
    }

    fn kill(&self, _reason: Option<AgentKillReason>) -> Result<(), AgentError> {
        let profile = self.gateway_profile.clone();
        let cwd = self.gateway_cwd.clone();
        tokio::spawn(async move {
            shared_gateway_pool().release(&profile, &cwd).await;
        });
        self.runtime.transition_to(ConversationStatus::Finished);
        Ok(())
    }
}
