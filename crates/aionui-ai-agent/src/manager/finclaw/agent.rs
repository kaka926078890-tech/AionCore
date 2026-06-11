#![cfg(feature = "findesk")]

use std::sync::Arc;

use aionui_api_types::FinclawBuildExtra;
use aionui_common::{AgentKillReason, AgentType, ConversationStatus};
use aionui_findesk::finclaw::{
    FinclawGateway, FinclawGatewayConfig, FinclawInferEvent, post_infer_stream,
    resolve_finclaw_infer_capability,
};
use aionui_findesk::FindeskConfig;
use futures_util::{pin_mut, StreamExt};
use reqwest::Client;
use tokio::sync::Notify;
use tracing::info;

use crate::agent_runtime::AgentRuntime;
use crate::agent_task::IAgentTask;
use crate::error::AgentError;
use crate::protocol::events::{AgentStreamEvent, TextEventData};
use crate::protocol::send_error::AgentSendError;
use crate::types::SendMessageData;

pub struct FinclawAgentManager {
    runtime: AgentRuntime,
    gateway: Arc<FinclawGateway>,
    config: FinclawBuildExtra,
    http: Client,
    cancel_notify: Arc<Notify>,
}

impl FinclawAgentManager {
    pub async fn new(
        conversation_id: String,
        workspace: String,
        config: FinclawBuildExtra,
    ) -> Result<Self, AgentError> {
        let findesk = FindeskConfig::from_env();
        let profile = config
            .finclaw_profile
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "default".to_string());
        let gateway = Arc::new(FinclawGateway::new(
            findesk,
            FinclawGatewayConfig {
                cli_path: config.cli_path.clone(),
                profile,
                security_mode: config.finclaw_security_mode.clone(),
                serve_cwd: workspace.clone().into(),
            },
        ));
        gateway
            .ensure_started()
            .await
            .map_err(AgentError::bad_request)?;

        Ok(Self {
            runtime: AgentRuntime::new(conversation_id, workspace, 128),
            gateway,
            config,
            http: Client::new(),
            cancel_notify: Arc::new(Notify::new()),
        })
    }

    fn resolve_user_id(&self) -> String {
        self.config
            .user_id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "default".to_string())
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

        let mut stream = post_infer_stream(
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
                    let event = item.map_err(|e| AgentError::bad_gateway(e))?;
                    match event {
                        FinclawInferEvent::TextChunk(delta) => {
                            self.runtime.emit(AgentStreamEvent::Text(TextEventData { content: delta }));
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
                Ok(())
            }
        };

        self.runtime.bump_activity();
        send_result.map_err(AgentSendError::from_agent_error)
    }

    async fn cancel(&self) -> Result<(), AgentError> {
        self.cancel_notify.notify_waiters();
        self.runtime.transition_to(ConversationStatus::Finished);
        Ok(())
    }

    fn kill(&self, _reason: Option<AgentKillReason>) -> Result<(), AgentError> {
        let gateway = Arc::clone(&self.gateway);
        tokio::spawn(async move {
            gateway.shutdown().await;
        });
        self.runtime.transition_to(ConversationStatus::Finished);
        Ok(())
    }
}
