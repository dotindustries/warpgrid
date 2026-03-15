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

use super::analytics::{AnalyticsService, EVENT_USER_REGISTERED};
use super::auth::{AuthStore, User};
use super::billing::{BillingService, PlanLimits};
use super::domains::{DomainError, DomainStore};
use super::registry::WasmRegistry;
use super::teams::{TeamRole, TeamStore};
use super::tenants;
use super::usage::UsageTracker;

/// Shared state for cloud API routes.
#[derive(Clone)]
pub struct CloudState {
    pub auth: AuthStore,
    pub registry: WasmRegistry,
    pub state_store: warpgrid_state::StateStore,
    pub teams: TeamStore,
    pub analytics: AnalyticsService,
    pub domains: DomainStore,
    pub billing: BillingService,
    pub usage: UsageTracker,
}

/// Build the cloud API router with all routes.
pub fn cloud_router(cloud_state: CloudState) -> Router {
    Router::new()
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/cloud/deployments", get(list_deployments))
        .route("/api/v1/cloud/deploy", post(create_deployment))
        .route("/api/v1/cloud/deploy/upload", post(upload_and_deploy))
        .route("/api/v1/cloud/deploy/{id}", delete(delete_deployment))
        .route("/api/v1/cloud/status", get(platform_status))
        // Team management routes
        .route("/api/v1/cloud/teams", get(list_teams).post(create_team))
        .route("/api/v1/cloud/teams/{id}/members", post(add_team_member))
        .route(
            "/api/v1/cloud/teams/{id}/members/{user_id}",
            delete(remove_team_member),
        )
        // Domain management routes
        .route(
            "/api/v1/cloud/domains",
            get(list_domains).post(add_domain),
        )
        .route(
            "/api/v1/cloud/domains/{domain}",
            delete(remove_domain),
        )
        // Billing routes
        .route("/api/v1/cloud/billing/plan", get(billing_plan))
        .route("/api/v1/cloud/billing/usage", get(billing_usage))
        .route("/api/v1/cloud/billing/portal", post(billing_portal))
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

    auth.validate_sync(token).ok_or((
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

    let (api_key, user) = match state.auth.register(&body.email).await {
        Ok(result) => result,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Registration failed: {e}"),
            )
            .into_response();
        }
    };

    state.analytics.track(
        &user.id,
        EVENT_USER_REGISTERED,
        serde_json::json!({
            "email": body.email,
            "namespace": &user.namespace,
        }),
    );

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

    // Extract deployment data from request body (JSON with base64 wasm).
    // For multipart support, we accept either:
    //   - JSON body: {"name": "...", "region": "...", "wasm_base64": "..."}
    //   - Raw bytes with headers: X-WarpGrid-Name, X-WarpGrid-Region
    error_response(
        StatusCode::NOT_IMPLEMENTED,
        "JSON deploy not yet implemented — use POST /api/v1/cloud/deploy/upload",
    )
    .into_response()
}

/// Upload Wasm binary and create a deployment.
///
/// Accepts raw bytes in the request body with metadata in headers:
/// - `X-WarpGrid-Name`: deployment name (required)
/// - `X-WarpGrid-Region`: target region (default: "iad")
/// - `Authorization: Bearer wg_live_...` (required)
///
/// The Wasm binary is stored in the registry and a DeploymentSpec
/// is created in the state store.
async fn upload_and_deploy(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Extract metadata from headers.
    let name = match headers.get("x-warpgrid-name").and_then(|v| v.to_str().ok()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            return error_response(StatusCode::BAD_REQUEST, "X-WarpGrid-Name header required")
                .into_response()
        }
    };
    let region = headers
        .get("x-warpgrid-region")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("iad")
        .to_string();

    // Validate body.
    if body.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Empty Wasm binary").into_response();
    }
    let wasm_size = body.len() as u64;
    if wasm_size > user.quota.max_wasm_size_bytes {
        return error_response(
            StatusCode::FORBIDDEN,
            &format!(
                "Wasm too large ({} KB, max {} KB)",
                wasm_size / 1024,
                user.quota.max_wasm_size_bytes / 1024
            ),
        )
        .into_response();
    }

    // Check deployment quota.
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

    // Check for duplicate deployment name.
    let deployment_id = tenants::scoped_deployment_id(&user.namespace, &name);
    if state.state_store.get_deployment(&deployment_id).ok().flatten().is_some() {
        return error_response(
            StatusCode::CONFLICT,
            &format!("Deployment '{}' already exists", name),
        )
        .into_response();
    }

    // Store Wasm in registry.
    let stored = match state.registry.store(&user.namespace, &name, &body) {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to store Wasm: {e}"),
            )
            .into_response()
        }
    };

    // Create deployment spec.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let spec = warpgrid_state::DeploymentSpec {
        id: deployment_id.clone(),
        namespace: user.namespace.clone(),
        name: name.clone(),
        source: format!("registry://{}/{}/{}", user.namespace, name, stored.hash),
        trigger: warpgrid_state::TriggerConfig::Http { port: Some(8080) },
        instances: warpgrid_state::InstanceConstraints { min: 1, max: 5 },
        resources: warpgrid_state::ResourceLimits {
            memory_bytes: 64 * 1024 * 1024,
            cpu_weight: 100,
        },
        scaling: None,
        health: None,
        shims: warpgrid_state::ShimsEnabled::default(),
        env: std::collections::HashMap::new(),
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = state.state_store.put_deployment(&spec) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create deployment: {e}"),
        )
        .into_response();
    }

    // Track analytics.
    state.analytics.track(
        &user.id,
        super::analytics::EVENT_DEPLOYMENT_CREATED,
        serde_json::json!({
            "deployment_id": deployment_id,
            "region": region,
            "wasm_size_bytes": wasm_size,
            "wasm_hash": stored.hash,
        }),
    );

    (
        StatusCode::CREATED,
        CloudResponse::ok(serde_json::json!({
            "deployment_id": deployment_id,
            "name": name,
            "namespace": user.namespace,
            "region": region,
            "wasm_hash": stored.hash,
            "wasm_size_bytes": wasm_size,
            "url": format!("https://{}.{}.edge.warpgrid.dev", name, user.namespace),
        })),
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
    if let Some((ns, name)) = tenants::extract_namespace(&id) {
        if ns != user.namespace {
            return error_response(StatusCode::FORBIDDEN, "Not your deployment").into_response();
        }
        // Clean up registry.
        let _ = state.registry.delete_deployment(ns, name);
    }

    // Track analytics.
    state.analytics.track(
        &user.id,
        super::analytics::EVENT_DEPLOYMENT_DELETED,
        serde_json::json!({ "deployment_id": id }),
    );

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

// ── Domain request/response types ───────────────────────────────

#[derive(Deserialize)]
struct AddDomainRequest {
    domain: String,
    deployment_id: String,
}

// ── Domain route handlers ───────────────────────────────────────

async fn add_domain(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    Json(body): Json<AddDomainRequest>,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    match state
        .domains
        .add_domain(&body.domain, &body.deployment_id, &user.namespace)
    {
        Ok(resp) => (StatusCode::CREATED, CloudResponse::ok(resp)).into_response(),
        Err(e) => {
            let status = match &e {
                DomainError::InvalidDomain { .. } => StatusCode::BAD_REQUEST,
                DomainError::AlreadyExists { .. } => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            };
            error_response(status, &e.to_string()).into_response()
        }
    }
}

async fn list_domains(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    let domains = state.domains.list_domains_for_namespace(&user.namespace);
    CloudResponse::ok(domains).into_response()
}

async fn remove_domain(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    Path(domain): Path<String>,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Verify the domain belongs to this user's namespace.
    if let Some(mapping) = state.domains.get_domain(&domain) {
        if mapping.namespace != user.namespace {
            return error_response(StatusCode::FORBIDDEN, "Not your domain").into_response();
        }
    }

    match state.domains.remove_domain(&domain) {
        Ok(()) => CloudResponse::ok("Domain removed").into_response(),
        Err(e) => {
            let status = match &e {
                DomainError::NotFound { .. } => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            };
            error_response(status, &e.to_string()).into_response()
        }
    }
}

// ── Billing response types ──────────────────────────────────────

#[derive(Serialize)]
struct BillingPlanResponse {
    plan: super::billing::Plan,
    price: &'static str,
    limits: PlanLimits,
}

#[derive(Serialize)]
struct BillingPortalResponse {
    url: String,
}

// ── Billing route handlers ──────────────────────────────────────

async fn billing_plan(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let _user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // For beta, all users are on the Free plan.
    let plan = super::billing::Plan::Free;
    let limits = PlanLimits::for_plan(plan);

    CloudResponse::ok(BillingPlanResponse {
        plan,
        price: plan.price_label(),
        limits,
    })
    .into_response()
}

async fn billing_usage(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    let usage = state.usage.peek(&user.namespace);
    CloudResponse::ok(usage).into_response()
}

async fn billing_portal(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_user(&headers, &state.auth) {
        Ok(u) => u,
        Err((status, msg)) => return error_response(status, &msg).into_response(),
    };

    // Use the user's namespace as a stand-in customer ID for beta.
    match state
        .billing
        .create_billing_portal_session(&user.namespace)
        .await
    {
        Ok(url) => CloudResponse::ok(BillingPortalResponse { url }).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create billing portal session: {e}"),
        )
        .into_response(),
    }
}
