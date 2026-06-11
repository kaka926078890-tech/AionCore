#![cfg(feature = "findesk")]

use std::sync::Arc;

use crate::agent_task::AgentInstance;
use crate::error::AgentError;
use crate::factory::context::FactoryContext;
use crate::manager::finclaw::FinclawAgentManager;
use crate::session_context::FinclawSessionBuildContext;

pub(super) async fn build(
    build_context: FinclawSessionBuildContext,
    ctx: FactoryContext,
) -> Result<AgentInstance, AgentError> {
    let agent = FinclawAgentManager::new(
        ctx.conversation_id,
        ctx.workspace,
        build_context.config,
    )
    .await?;
    Ok(AgentInstance::Finclaw(Arc::new(agent)))
}
