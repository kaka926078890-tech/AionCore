#![cfg(feature = "findesk")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aionui_api_types::FinclawBuildExtra;
use aionui_common::{
    decrypt_string, AgentKillReason, AgentType, Confirmation, ConfirmationOption, ConversationStatus,
    ProviderWithModel,
};
use aionui_db::IProviderRepository;
use aionui_findesk::FindeskConfig;
use aionui_findesk::finclaw::{
    apply_tool_policy_from_extra, build_finclaw_llm_config_slice, build_finclaw_model_fingerprint,
    ensure_finclaw_workspace_profile, prepare_finclaw_llm_for_profiles, resolve_finclaw_serve_cwd,
    FinclawApprovalRequired, FinclawGatewayConfig, FinclawInferEvent, decision_from_confirm_data,
    post_infer_stream, resolve_finclaw_infer_capability, shared_gateway_pool, submit_approval_resolve,
};
use futures_util::{pin_mut, StreamExt};
use reqwest::Client;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, Notify, oneshot};
use tracing::info;

use crate::agent_runtime::AgentRuntime;
use crate::agent_task::IAgentTask;
use crate::error::AgentError;
use crate::protocol::events::{
    AcpPermissionEventData, AcpPermissionOptionData, AcpPermissionOptionKind, AcpPermissionRequestData,
    AcpPermissionToolCall, AgentStreamEvent, TextEventData,
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
        let derived_profile = ensure_finclaw_workspace_profile(base_profile, &serve_cwd)
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
            .ok_or_else(|| {
                AgentError::bad_request(format!("Provider '{}' not found", model.provider_id))
            })?;

        let api_key = decrypt_string(&row.api_key_encrypted, &encryption_key)
            .map_err(|e| AgentError::internal(e.to_string()))?;

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
            &derived_profile,
            &llm_slice,
        )
        .await
        .map_err(AgentError::bad_request)?;

        let gateway_config = FinclawGatewayConfig {
            cli_path: config.cli_path.clone(),
            profile: derived_profile.clone(),
            security_mode: config.finclaw_security_mode.clone(),
            serve_cwd: serve_cwd.clone(),
            llm_serve_env,
            model_fingerprint: Some(build_finclaw_model_fingerprint(&llm_slice)),
        };

        let pool = shared_gateway_pool();
        let (gateway, _) = pool.acquire(gateway_config).await;
        gateway
            .ensure_started()
            .await
            .map_err(AgentError::bad_request)?;

        Ok(Self {
            runtime: AgentRuntime::new(conversation_id, workspace, 128),
            gateway_profile: derived_profile,
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

    pub fn confirm(
        &self,
        _msg_id: &str,
        call_id: &str,
        data: Value,
        _always_allow: bool,
    ) -> Result<(), AgentError> {
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

        let port = self.gateway.claw_port().ok_or_else(|| {
            AgentError::bad_request("FinClaw claw_port is not available")
        })?;
        let decision = decision_from_confirm_data(&data);
        let user_id = self.resolve_user_id();
        let approval = item.approval.clone();
        let http = self.http.clone();
        let session_id = self.conversation_id().to_string();
        let reason = data
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string);

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

        self.runtime.emit(AgentStreamEvent::AcpPermission(
            AcpPermissionEventData::Request(AcpPermissionRequestData {
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
                    meta.insert(
                        "finclawApprovalRequestId".into(),
                        json!(approval.approval_request_id),
                    );
                    if let Some(run_id) = &approval.run_id {
                        meta.insert("finclawRunId".into(), json!(run_id));
                    }
                    meta
                }),
            }),
        ));

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
            AgentSendError::from_agent_error(AgentError::bad_request(
                "FinClaw claw_port is not available",
            ))
        })?;

        info!(
            conversation_id = %self.conversation_id(),
            msg_id = %data.msg_id,
            "FinClaw send_message started"
        );
        self.runtime.bump_activity();
        self.runtime.reset_for_new_turn(ConversationStatus::Running);

        let user_id = self.resolve_user_id();
        let capability =
            resolve_finclaw_infer_capability(self.config.session_mode.as_deref()).to_string();
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

        let send_result = tokio::select! {
            result = async {
                while let Some(item) = stream.next().await {
                    let event = item.map_err(AgentError::bad_gateway)?;
                    match event {
                        FinclawInferEvent::TextChunk(delta) => {
                            self.runtime.emit(AgentStreamEvent::Text(TextEventData { content: delta }));
                        }
                        FinclawInferEvent::ApprovalRequired(approval) => {
                            self.handle_approval_required(approval).await?;
                        }
                        FinclawInferEvent::Finished => {
                            self.runtime.emit_finish(None);
                            return Ok(());
                        }
                        FinclawInferEvent::Error(message) => {
                            self.runtime.emit_error(message);
                            return Ok(());
                        }
                    }
                }
                self.runtime.emit_finish(None);
                Ok(())
            } => result,
            _ = self.cancel_notify.notified() => {
                info!(conversation_id = %self.conversation_id(), "FinClaw infer cancelled");
                self.pending_approvals.lock().await.clear();
                Ok(())
            }
        };

        self.runtime.bump_activity();
        send_result.map_err(AgentSendError::from_agent_error)
    }

    async fn cancel(&self) -> Result<(), AgentError> {
        self.cancel_notify.notify_waiters();
        self.pending_approvals.lock().await.clear();
        self.runtime.transition_to(ConversationStatus::Finished);
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
