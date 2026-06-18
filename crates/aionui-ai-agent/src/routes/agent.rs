#![allow(clippy::disallowed_types)]

//! Agent-related API routes.
//!
//! Endpoints:
//!
//! - `GET  /api/agents`         — list available agents
//! - `POST /api/agents/refresh` — refresh agent list (e.g. after new agent is added to the system)
//! - `POST /api/agents/test`    — test custom agent configuration (e.g. LLM connection)

use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Json, Path, Query, State};
use axum::routing::{get, patch, post, put};

use aionui_api_types::{
    AcpHealthCheckRequest, AcpHealthCheckResponse, AgentMetadata, ApiResponse, CustomAgentUpsertRequest,
    DeleteCustomAgentResponse, FinclawApplyToolPolicyRequest, FinclawApplyToolPolicyResponse,
    FinclawGetToolPolicyQuery, FinclawHostContext, FinclawHostUser, FinclawToolPolicyResponse,
    ProviderHealthCheckRequest, ProviderHealthCheckResponse, SetEnabledRequest, TryConnectCustomAgentRequest,
    TryConnectCustomAgentResponse,
};
use aionui_auth::CurrentUser;
use aionui_common::ApiError;
#[cfg(feature = "findesk")]
use aionui_findesk::finclaw::{
    FinclawToolPolicy, HostAgentRow, apply_tool_policy_serve_overlay, finclaw_tool_policy_to_wire,
    ensure_finclaw_workspace_profile, is_finclaw_tool_policy, read_tool_policy_from_serve_overlay,
    resolve_finclaw_host_context, shared_gateway_pool,
    validate_finclaw_workspace_path,
};

use crate::routes::error_mapping::agent_error_to_api_error;
use crate::routes::state::AgentRouterState;

pub fn agent_routes(state: AgentRouterState) -> Router {
    let router = Router::new()
        .route("/api/agents", get(list_agents))
        .route("/api/agents/refresh", post(refresh_agents))
        .route("/api/agents/health-check", post(health_check))
        .route("/api/agents/provider-health-check", post(provider_health_check))
        .route("/api/agents/{id}/enabled", patch(set_agent_enabled))
        .route("/api/agents/custom", post(create_custom))
        .route("/api/agents/custom/{id}", put(update_custom).delete(delete_custom))
        .route("/api/agents/custom/try-connect", post(try_connect_custom));

    #[cfg(feature = "findesk")]
    let router = router
        .route("/api/agents/finclaw/host-context", get(finclaw_host_context))
        .route("/api/agents/finclaw/tool-policy", get(finclaw_get_tool_policy))
        .route("/api/agents/finclaw/apply-tool-policy", post(finclaw_apply_tool_policy));

    router.with_state(state)
}

#[cfg(feature = "findesk")]
async fn finclaw_host_context(
    State(state): State<AgentRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<FinclawHostContext>>, ApiError> {
    let agents = state.agent_registry.list_all().await;
    let rows: Vec<HostAgentRow> = agents
        .into_iter()
        .map(|agent| HostAgentRow {
            agent_type: agent.agent_type.serde_name().to_string(),
            backend: agent.backend,
            name: agent.name,
            enabled: agent.enabled,
            native_skills_dirs: agent.native_skills_dirs,
        })
        .collect();

    let mut context = resolve_finclaw_host_context(&rows);
    context.user = Some(FinclawHostUser {
        id: user.id,
        email: Some(user.username),
    });

    Ok(Json(ApiResponse::ok(context)))
}

#[cfg(feature = "findesk")]
async fn finclaw_get_tool_policy(
    Extension(_user): Extension<CurrentUser>,
    Query(query): Query<FinclawGetToolPolicyQuery>,
) -> Result<Json<ApiResponse<FinclawToolPolicyResponse>>, ApiError> {
    let workspace = query.workspace.trim();
    if workspace.is_empty() {
        return Err(ApiError::BadRequest("workspace is required".into()));
    }
    let serve_cwd = validate_finclaw_workspace_path(workspace).map_err(ApiError::BadRequest)?;
    let profile = query
        .profile
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string();
    let derived_profile = ensure_finclaw_workspace_profile(&profile, &serve_cwd).map_err(ApiError::BadRequest)?;
    let policy = read_tool_policy_from_serve_overlay(&serve_cwd).unwrap_or(FinclawToolPolicy::AutoAll);
    let mut pool_ref_count = shared_gateway_pool()
        .ref_count_for_workspace(&derived_profile, &serve_cwd)
        .await;
    if pool_ref_count == 0
        && query
            .conversation_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
    {
        pool_ref_count = 1;
    }

    Ok(Json(ApiResponse::ok(FinclawToolPolicyResponse {
        tool_policy: finclaw_tool_policy_to_wire(policy).to_string(),
        pool_ref_count,
        profile: derived_profile,
    })))
}

#[cfg(feature = "findesk")]
async fn finclaw_apply_tool_policy(
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<FinclawApplyToolPolicyRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<FinclawApplyToolPolicyResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let workspace = req.workspace.trim();
    if workspace.is_empty() {
        return Err(ApiError::BadRequest("workspace is required".into()));
    }
    if !is_finclaw_tool_policy(&req.tool_policy) {
        return Err(ApiError::BadRequest("invalid finclaw tool_policy".into()));
    }

    let profile = req
        .profile
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string();
    let serve_cwd = validate_finclaw_workspace_path(workspace).map_err(ApiError::BadRequest)?;
    let derived_profile = ensure_finclaw_workspace_profile(&profile, &serve_cwd).map_err(ApiError::BadRequest)?;

    apply_tool_policy_serve_overlay(&serve_cwd, &req.tool_policy).map_err(ApiError::BadRequest)?;

    let (restarted, pool_ref_count) = shared_gateway_pool()
        .restart_gateways_for_workspace(&derived_profile, &serve_cwd)
        .await;

    Ok(Json(ApiResponse::ok(FinclawApplyToolPolicyResponse {
        tool_policy: req.tool_policy,
        pool_ref_count,
        profile: derived_profile,
        restarted,
    })))
}

async fn list_agents(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<AgentMetadata>>>, ApiError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_agents().await.map_err(agent_error_to_api_error)?,
    )))
}

async fn refresh_agents(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<AgentMetadata>>>, ApiError> {
    Ok(Json(ApiResponse::ok(
        state.service.refresh_agents().await.map_err(agent_error_to_api_error)?,
    )))
}

async fn health_check(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<AcpHealthCheckRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<AcpHealthCheckResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .acp_health_check(req)
            .await
            .map_err(agent_error_to_api_error)?,
    )))
}

async fn provider_health_check(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<ProviderHealthCheckRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<ProviderHealthCheckResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .provider_health_check(req)
            .await
            .map_err(agent_error_to_api_error)?,
    )))
}

async fn try_connect_custom(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<TryConnectCustomAgentRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<TryConnectCustomAgentResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .try_connect_custom_agent(req)
            .await
            .map_err(agent_error_to_api_error)?,
    )))
}

async fn create_custom(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<CustomAgentUpsertRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<AgentMetadata>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .create_custom_agent(req)
            .await
            .map_err(agent_error_to_api_error)?,
    )))
}

async fn update_custom(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<CustomAgentUpsertRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<AgentMetadata>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .update_custom_agent(&id, req)
            .await
            .map_err(agent_error_to_api_error)?,
    )))
}

async fn delete_custom(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<DeleteCustomAgentResponse>>, ApiError> {
    state
        .service
        .delete_custom_agent(&id)
        .await
        .map_err(agent_error_to_api_error)?;
    Ok(Json(ApiResponse::ok(DeleteCustomAgentResponse { deleted: true })))
}

async fn set_agent_enabled(
    State(state): State<AgentRouterState>,
    Extension(_user): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<SetEnabledRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<AgentMetadata>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .set_agent_enabled(&id, req.enabled)
            .await
            .map_err(agent_error_to_api_error)?,
    )))
}
