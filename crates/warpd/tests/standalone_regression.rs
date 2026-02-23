//! Standalone regression tests.
//!
//! Validates that standalone mode works correctly: starts the API server,
//! handles deployments, scales, health checks, and metrics.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use warpgrid_api::build_router;
use warpgrid_state::*;

fn test_store() -> StateStore {
    StateStore::open_in_memory().unwrap()
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

#[tokio::test]
async fn standalone_api_list_deployments_empty() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/deployments")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn standalone_api_create_and_get_deployment() {
    let store = test_store();
    let router = build_router(store);

    let spec = test_deployment("default", "api");
    let body = serde_json::to_vec(&spec).unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Get the deployment back.
    let req = Request::builder()
        .uri("/api/v1/deployments/default%2Fapi")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn standalone_api_delete_deployment() {
    let store = test_store();
    let spec = test_deployment("default", "web");
    store.put_deployment(&spec).unwrap();

    let router = build_router(store);

    let req = Request::builder()
        .method("DELETE")
        .uri("/api/v1/deployments/default%2Fweb")
        .body(Body::empty())
        .unwrap();

    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Confirm gone.
    let req = Request::builder()
        .uri("/api/v1/deployments/default%2Fweb")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn standalone_api_list_nodes_empty() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/nodes")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn standalone_api_list_instances_empty() {
    let store = test_store();
    let spec = test_deployment("default", "svc");
    store.put_deployment(&spec).unwrap();

    let router = build_router(store);

    let req = Request::builder()
        .uri("/api/v1/deployments/default%2Fsvc/instances")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn standalone_api_metrics_endpoint() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn standalone_api_rollout_on_missing_deployment() {
    let store = test_store();
    let router = build_router(store);

    let body = r#"{"strategy":"BlueGreen","new_version":"v2"}"#;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/deployments/nope%2Fmissing/rollout")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn standalone_api_scale_deployment() {
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

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn standalone_dashboard_accessible() {
    let store = test_store();
    let router = build_router(store);

    let req = Request::builder()
        .uri("/dashboard")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    // Dashboard may return 200, redirect, or 307 for trailing-slash — all OK.
    assert!(
        resp.status() == StatusCode::OK
            || resp.status().is_redirection()
            || resp.status() == StatusCode::TEMPORARY_REDIRECT,
        "unexpected status: {}",
        resp.status()
    );
}
