#![cfg(feature = "findesk")]

use std::sync::Arc;

use aionui_common::ProviderWithModel;

use crate::agent_task::AgentInstance;
use crate::error::AgentError;
use crate::factory::AgentFactoryDeps;
use crate::factory::context::FactoryContext;
use crate::manager::finclaw::FinclawAgentManager;
use crate::session_context::FinclawSessionBuildContext;

pub(super) async fn build(
    deps: Arc<AgentFactoryDeps>,
    build_context: FinclawSessionBuildContext,
    model: ProviderWithModel,
    ctx: FactoryContext,
) -> Result<AgentInstance, AgentError> {
    let agent = FinclawAgentManager::new(
        ctx.conversation_id,
        ctx.workspace,
        build_context.config,
        model,
        deps.provider_repo.clone(),
        deps.encryption_key,
    )
    .await?;
    Ok(AgentInstance::Finclaw(Arc::new(agent)))
}
