//! Cloud API routes for the hosted platform.
//!
//! All routes require API key authentication via the `Authorization: Bearer wg_live_...` header.
//! Routes are namespace-scoped — users can only see/modify their own resources.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::auth::{AuthStore, User};
use super::registry::WasmRegistry;
use super::teams::{TeamRole, TeamStore};
use super::tenants;

/// Shared state for cloud API routes.
#[derive(Clone)]
pub struct CloudState {
    pub auth: AuthStore,
    pub registry: WasmRegistry,
    pub state_store: warpgrid_state::StateStore,
    pub teams: TeamStore,
}

/// Build the cloud API router with all routes.
pub fn cloud_router(cloud_state: CloudState) -> Router {
    Router::new()
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/cloud/deployments", get(list_deployments))
        .route("/api/v1/cloud/deploy", post(create_deployment))
        .route("/api/v1/cloud/deploy/{id}", delete(delete_deployment))
        .route("/api/v1/cloud/status", get(platform_status))
        // Team management routes
        .route("/api/v1/cloud/teams", get(list_teams).post(create_team))
        .route("/api/v1/cloud/teams/{id}/members", post(add_team_member))
        .route(
            "/api/v1/cloud/teams/{id}/members/{user_id}",
            delete(remove_team_member),
        )
        .with_state(Arc::new(cloud_state))
}

// ── Request/Response types ──────────────────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    email: String,
}

#[derive(Serialize)]
struct RegisterResponse {
    success: bool,
    api_key: String,
    user_id: String,
    namespace: String,
}

#[derive(Serialize)]
struct DeploymentInfo {
    id: String,
    namespace: String,
    name: String,
    status: String,
    instances: u32,
    region: String,
}

#[derive(Serialize)]
struct CloudResponse<T: Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: Serialize> CloudResponse<T> {
    fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
            error: None,
        })
    }
}

fn error_response(status: StatusCode, msg: &str) -> impl IntoResponse {
    (
        status,
        Json(CloudResponse::<()> {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }),
    )
}

// ── Auth helper ─────────────────────────────────────────────────

fn extract_user(headers: &HeaderMap, auth: &AuthStore) -> Result<User, (StatusCode, String)> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Missing Authorization header".to_string(),
        ))?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Invalid Authorization format (expected: Bearer wg_live_...)".to_string(),
        ))?;

    auth.validate(token).ok_or((
        StatusCode::UNAUTHORIZED,
        "Invalid API key".to_string(),
    ))
}

// ── Route handlers ──────────────────────────────────────────────

async fn register(
    State(state): State<Arc<CloudState>>,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    if body.email.is_empty() || !body.email.contains('@') {
        return error_response(StatusCode::BAD_REQUEST, "Invalid email address").into_response();
    }

    let (api_key, user) = state.auth.register(&body.email);

    (
        StatusCode::CREATED,
        CloudResponse::ok(RegisterResponse {
            success: true,
            api_key,
            user_id: user.id,
            namespace: user.namespace,
        }),
    )
        .into_response()
}

async fn list_deployments(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    let all_deployments = match state.state_store.list_deployments() {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to list deployments: {e}"),
            )
            .into_response()
        }
    };

    // Filter to only this user's namespace.
    let user_deployments: Vec<DeploymentInfo> = all_deployments
        .into_iter()
        .filter(|d| d.namespace == user.namespace)
        .map(|d| DeploymentInfo {
            id: d.id.clone(),
            namespace: d.namespace.clone(),
            name: d.name.clone(),
            status: "running".to_string(),
            instances: d.instances.min as u32,
            region: "iad".to_string(),
        })
        .collect();

    CloudResponse::ok(user_deployments).into_response()
}

async fn create_deployment(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Check quota.
    let deployments = state.state_store.list_deployments().unwrap_or_default();
    let user_count = deployments
        .iter()
        .filter(|d| d.namespace == user.namespace)
        .count() as u32;

    if user_count >= user.quota.max_deployments {
        return error_response(
            StatusCode::FORBIDDEN,
            &format!(
                "Deployment limit reached ({}/{})",
                user_count, user.quota.max_deployments
            ),
        )
        .into_response();
    }

    // TODO: Accept wasm upload, store in registry, create deployment in state
    error_response(
        StatusCode::NOT_IMPLEMENTED,
        "Full deploy pipeline not yet implemented — use CLI: warp deploy",
    )
    .into_response()
}

async fn delete_deployment(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Verify the deployment belongs to this user's namespace.
    if let Some((ns, _)) = tenants::extract_namespace(&id) {
        if ns != user.namespace {
            return error_response(StatusCode::FORBIDDEN, "Not your deployment").into_response();
        }
    }

    // TODO: Deprovision from edge nodes, remove from registry
    match state.state_store.delete_deployment(&id) {
        Ok(true) => CloudResponse::ok("Deployment deleted").into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "Deployment not found").into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to delete: {e}"),
        )
        .into_response(),
    }
}

async fn platform_status() -> impl IntoResponse {
    CloudResponse::ok(serde_json::json!({
        "status": "operational",
        "version": env!("CARGO_PKG_VERSION"),
        "mode": "cloud",
    }))
}

// ── Team request/response types ─────────────────────────────────

#[derive(Deserialize)]
struct CreateTeamRequest {
    name: String,
}

#[derive(Deserialize)]
struct AddMemberRequest {
    user_id: String,
    #[serde(default = "default_member_role")]
    role: TeamRole,
}

fn default_member_role() -> TeamRole {
    TeamRole::Member
}

// ── Team route handlers ─────────────────────────────────────────

async fn create_team(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    Json(body): Json<CreateTeamRequest>,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    if body.name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Team name is required").into_response();
    }

    let team = state.teams.create_team(&body.name, &user.id);

    (StatusCode::CREATED, CloudResponse::ok(team)).into_response()
}

async fn list_teams(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    let teams = state.teams.list_teams_for_user(&user.id);
    CloudResponse::ok(teams).into_response()
}

async fn add_team_member(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    Path(team_id): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Only admins and owners can add members.
    if !state
        .teams
        .check_permission(&team_id, &user.id, TeamRole::Admin)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "Only team admins and owners can add members",
        )
        .into_response();
    }

    match state.teams.add_member(&team_id, &body.user_id, body.role) {
        Ok(team) => (StatusCode::CREATED, CloudResponse::ok(team)).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()).into_response(),
    }
}

async fn remove_team_member(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    Path((team_id, member_user_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Only admins and owners can remove members.
    if !state
        .teams
        .check_permission(&team_id, &user.id, TeamRole::Admin)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "Only team admins and owners can remove members",
        )
        .into_response();
    }

    match state.teams.remove_member(&team_id, &member_user_id) {
        Ok(team) => CloudResponse::ok(team).into_response(),
        Err(e) => {
            let status = match &e {
                super::teams::TeamError::CannotRemoveOwner => StatusCode::FORBIDDEN,
                super::teams::TeamError::TeamNotFound { .. } => StatusCode::NOT_FOUND,
                super::teams::TeamError::UserNotMember { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            };
            error_response(status, &e.to_string()).into_response()
        }
    }
}
