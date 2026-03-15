//! Integration tests for warpgrid-api.
//!
//! Validates the full REST API surface through the axum router using
//! tower::ServiceExt and in-memory state stores. No external services.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use warpgrid_api::{build_router, build_router_with_rollouts, RolloutStore};
use warpgrid_state::*;

// ── Helpers ────────────────────────────────────────────────────────

fn test_store() -> StateStore {
    StateStore::open_in_memory().unwrap()
}

fn empty_rollout_store() -> RolloutStore {
    Arc::new(RwLock::new(HashMap::new()))
}

fn test_deployment(ns: &str, name: &str) -> DeploymentSpec {
    DeploymentSpec {
        id: format!("{ns}/{name}"),
        namespace: ns.to_string(),
        name: name.to_string(),
        source: "file://test.wasm".to_string(),
        trigger: TriggerConfig::Http { port: Some(8080) },
        instances: InstanceConstraints { min: 1, max: 5 },
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

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── 1. Full deployment CRUD lifecycle ──────────────────────────────

#[tokio::test]
async fn deployment_crud_lifecycle() {
    let store = test_store();
    let router = build_router(store);

    // POST — create deployment
    let spec = test_deployment("default", "web");
    let body = serde_json::to_vec(&spec).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_json(resp).await;
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["id"], "default/web");

    // GET — verify exists
    let req = Request::builder()
        .uri("/api/v1/deployments/default%2Fweb")
        .body(Body::empty())
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["name"], "web");

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/v1/deployments/default%2Fweb")
        .body(Body::empty())
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET — confirm 404
    let req = Request::builder()
        .uri("/api/v1/deployments/default%2Fweb")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── 2. Rollout lifecycle via API ───────────────────────────────────

#[tokio::test]
async fn rollout_lifecycle_start_pause_resume() {
    let store = test_store();
    let spec = test_deployment("prod", "api");
    store.put_deployment(&spec).unwrap();

    let rollouts = empty_rollout_store();
    let router = build_router_with_rollouts(store, rollouts.clone());

    // Start rollout
    let body = r#"{"strategy":"BlueGreen","new_version":"v2"}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/prod%2Fapi/rollout")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_json(resp).await;
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["deployment_id"], "prod/api");

    // Get rollout status
    let req = Request::builder()
        .uri("/api/v1/rollouts/prod%2Fapi")
        .body(Body::empty())
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["data"]["new_version"], "v2");

    // Pause rollout
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/rollouts/prod%2Fapi/pause")
        .body(Body::empty())
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["data"]["phase"], "Paused");

    // Resume rollout
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/rollouts/prod%2Fapi/resume")
        .body(Body::empty())
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["data"]["phase"], "HealthGate");
}

// ── 3. Scale validates max boundary ────────────────────────────────

#[tokio::test]
async fn scale_rejects_target_above_max() {
    let store = test_store();
    let spec = test_deployment("default", "api");
    store.put_deployment(&spec).unwrap();

    let router = build_router(store);

    // target=100 exceeds max=5
    let body = r#"{"target":100}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/default%2Fapi/scale")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let json = body_json(resp).await;
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("exceeds max"));
}

#[tokio::test]
async fn scale_accepts_target_within_max() {
    let store = test_store();
    let spec = test_deployment("default", "api");
    store.put_deployment(&spec).unwrap();

    let router = build_router(store);

    let body = r#"{"target":3}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/default%2Fapi/scale")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["target"], 3);
    assert_eq!(json["data"]["status"], "scaling");
}

#[tokio::test]
async fn scale_missing_deployment_returns_404() {
    let store = test_store();
    let router = build_router(store);

    let body = r#"{"target":1}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/nope%2Fmissing/scale")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── 4. Prometheus endpoint returns valid format ────────────────────

#[tokio::test]
async fn prometheus_endpoint_returns_text_plain() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/plain"),
        "expected text/plain content-type, got: {content_type}"
    );
}

#[tokio::test]
async fn prometheus_endpoint_includes_help_lines_with_data() {
    let store = test_store();
    let spec = test_deployment("default", "api");
    store.put_deployment(&spec).unwrap();

    let snapshot = MetricsSnapshot {
        deployment_id: "default/api".to_string(),
        epoch: 1000,
        rps: 42.5,
        latency_p50_ms: 5.0,
        latency_p99_ms: 25.0,
        error_rate: 0.01,
        total_memory_bytes: 64 * 1024 * 1024,
        active_instances: 2,
    };
    store.put_metrics(&snapshot).unwrap();

    let router = build_router(store);

    let req = Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    let text = body_string(resp).await;

    assert!(
        text.contains("# HELP"),
        "expected prometheus HELP lines in output"
    );
    assert!(
        text.contains("# TYPE"),
        "expected prometheus TYPE lines in output"
    );
    assert!(
        text.contains("warpgrid_"),
        "expected warpgrid_ metric names in output"
    );
}

// ── 5. API response format consistency ─────────────────────────────

#[tokio::test]
async fn success_response_has_correct_shape() {
    let store = test_store();
    let spec = test_deployment("default", "api");
    store.put_deployment(&spec).unwrap();

    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/deployments/default%2Fapi")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    let json = body_json(resp).await;

    assert_eq!(json["success"], true);
    assert!(json["data"].is_object(), "success response must have data");
    assert!(
        json.get("error").is_none() || json["error"].is_null(),
        "success response must not have error"
    );
}

#[tokio::test]
async fn error_response_has_correct_shape() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/deployments/nonexistent")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let json = body_json(resp).await;
    assert_eq!(json["success"], false);
    assert!(
        json["error"].is_string(),
        "error response must have error string"
    );
    assert!(
        json.get("data").is_none() || json["data"].is_null(),
        "error response must not have data"
    );
}

// ── 6. List deployments returns correct count ──────────────────────

#[tokio::test]
async fn list_deployments_empty_returns_empty_array() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/deployments")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["success"], true);
    assert!(json["data"].is_array());
    assert_eq!(json["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_deployments_returns_correct_count() {
    let store = test_store();
    store
        .put_deployment(&test_deployment("ns1", "svc-a"))
        .unwrap();
    store
        .put_deployment(&test_deployment("ns1", "svc-b"))
        .unwrap();
    store
        .put_deployment(&test_deployment("ns2", "worker"))
        .unwrap();

    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/deployments")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["data"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn list_rollouts_returns_active_rollouts() {
    let store = test_store();
    let spec = test_deployment("prod", "api");
    store.put_deployment(&spec).unwrap();

    let rollouts = empty_rollout_store();
    let router = build_router_with_rollouts(store, rollouts);

    // Start a rollout
    let body = r#"{"strategy":"BlueGreen","new_version":"v2"}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/prod%2Fapi/rollout")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // List rollouts
    let req = Request::builder()
        .uri("/api/v1/rollouts")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn duplicate_rollout_returns_conflict() {
    let store = test_store();
    let spec = test_deployment("prod", "api");
    store.put_deployment(&spec).unwrap();

    let rollouts = empty_rollout_store();
    let router = build_router_with_rollouts(store, rollouts);

    let body = r#"{"strategy":"BlueGreen","new_version":"v2"}"#;

    // First start succeeds
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/prod%2Fapi/rollout")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second start conflicts
    let body = r#"{"strategy":"BlueGreen","new_version":"v3"}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/prod%2Fapi/rollout")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
