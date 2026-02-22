//! REST API handlers.
//!
//! Each handler reads/writes via `StateStore` and returns JSON responses.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use warpgrid_state::*;

use crate::ApiState;

/// Response wrapper for consistent API format.
#[derive(serde::Serialize)]
struct ApiResponse<T: serde::Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: serde::Serialize> ApiResponse<T> {
    fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
            error: None,
        })
    }
}

fn error_response(msg: &str, status: StatusCode) -> impl IntoResponse {
    (
        status,
        Json(ApiResponse::<()> {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }),
    )
}

// ── Deployments ────────────────────────────────────────────────

/// GET /api/v1/deployments
pub async fn list_deployments(State(state): State<ApiState>) -> impl IntoResponse {
    match state.store.list_deployments() {
        Ok(deployments) => ApiResponse::ok(deployments).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// GET /api/v1/deployments/:id
pub async fn get_deployment(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_deployment(&id) {
        Ok(Some(spec)) => ApiResponse::ok(spec).into_response(),
        Ok(None) => error_response("deployment not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// POST /api/v1/deployments
pub async fn create_deployment(
    State(state): State<ApiState>,
    Json(spec): Json<DeploymentSpec>,
) -> impl IntoResponse {
    match state.store.put_deployment(&spec) {
        Ok(()) => (StatusCode::CREATED, ApiResponse::ok(spec)).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// DELETE /api/v1/deployments/:id
pub async fn delete_deployment(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_deployment(&id) {
        Ok(true) => ApiResponse::ok("deleted").into_response(),
        Ok(false) => error_response("deployment not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

// ── Instances ──────────────────────────────────────────────────

/// GET /api/v1/deployments/:id/instances
pub async fn list_instances(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.list_instances_for_deployment(&id) {
        Ok(instances) => ApiResponse::ok(instances).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

// ── Scaling ────────────────────────────────────────────────────

/// Scale request body.
#[derive(serde::Deserialize)]
pub struct ScaleRequest {
    pub target: u32,
}

/// POST /api/v1/deployments/:id/scale
pub async fn scale_deployment(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<ScaleRequest>,
) -> impl IntoResponse {
    // Validate deployment exists.
    match state.store.get_deployment(&id) {
        Ok(Some(spec)) => {
            if req.target > spec.instances.max {
                return error_response(
                    &format!("target {} exceeds max {}", req.target, spec.instances.max),
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            }
            ApiResponse::ok(serde_json::json!({
                "deployment": id,
                "target": req.target,
                "status": "scaling"
            }))
            .into_response()
        }
        Ok(None) => error_response("deployment not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

// ── Metrics ────────────────────────────────────────────────────

/// GET /api/v1/deployments/:id/metrics
pub async fn get_metrics(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.list_metrics_for_deployment(&id, 60) {
        Ok(metrics) => ApiResponse::ok(metrics).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

// ── Nodes ──────────────────────────────────────────────────────

/// GET /api/v1/nodes
pub async fn list_nodes(State(state): State<ApiState>) -> impl IntoResponse {
    match state.store.list_nodes() {
        Ok(nodes) => ApiResponse::ok(nodes).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

// ── Prometheus ─────────────────────────────────────────────────

/// GET /metrics
pub async fn prometheus_metrics(State(state): State<ApiState>) -> impl IntoResponse {
    // Collect latest metrics for all deployments.
    let deployments = state.store.list_deployments().unwrap_or_default();
    let mut snapshots = Vec::new();

    for d in &deployments {
        if let Ok(metrics) = state.store.list_metrics_for_deployment(&d.id, 1) {
            snapshots.extend(metrics);
        }
    }

    let body = warpgrid_metrics::render_prometheus(&snapshots);
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_state() -> ApiState {
        let store = StateStore::open_in_memory().unwrap();
        ApiState { store }
    }

    fn test_deployment(ns: &str, name: &str) -> DeploymentSpec {
        DeploymentSpec {
            id: format!("{ns}/{name}"),
            namespace: ns.to_string(),
            name: name.to_string(),
            source: "file://test.wasm".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 1, max: 10 },
            resources: ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[tokio::test]
    async fn list_deployments_empty() {
        let state = test_state();
        let resp = list_deployments(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_and_get_deployment() {
        let state = test_state();
        let spec = test_deployment("default", "api");

        let resp = create_deployment(State(state.clone()), Json(spec.clone())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = get_deployment(State(state), Path("default/api".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_nonexistent_deployment() {
        let state = test_state();
        let resp = get_deployment(State(state), Path("nope".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_deployment_exists() {
        let state = test_state();
        let spec = test_deployment("default", "api");
        state.store.put_deployment(&spec).unwrap();

        let resp = delete_deployment(State(state), Path("default/api".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_nonexistent_deployment() {
        let state = test_state();
        let resp = delete_deployment(State(state), Path("nope".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn scale_validates_max() {
        let state = test_state();
        let spec = test_deployment("default", "api");
        state.store.put_deployment(&spec).unwrap();

        let req = ScaleRequest { target: 100 };
        let resp = scale_deployment(
            State(state),
            Path("default/api".to_string()),
            Json(req),
        ).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_nodes_empty() {
        let state = test_state();
        let resp = list_nodes(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn prometheus_endpoint_returns_text() {
        let state = test_state();
        let resp = prometheus_metrics(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(content_type.contains("text/plain"));
    }
}
