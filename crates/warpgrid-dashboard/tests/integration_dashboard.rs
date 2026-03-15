//! Integration tests for warpgrid-dashboard.
//!
//! Validates dashboard page rendering, actions, and view builder helpers
//! using in-memory state stores. No external services.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use warpgrid_dashboard::views;
use warpgrid_dashboard::{DashboardState, dashboard_router};
use warpgrid_state::*;

// ── Helpers ────────────────────────────────────────────────────────

fn test_state() -> DashboardState {
    let store = StateStore::open_in_memory().unwrap();
    DashboardState {
        store,
        rollouts: Arc::new(RwLock::new(HashMap::new())),
    }
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

fn populated_state() -> DashboardState {
    let state = test_state();

    state
        .store
        .put_deployment(&test_deployment("default", "api"))
        .unwrap();
    state
        .store
        .put_deployment(&test_deployment("prod", "worker"))
        .unwrap();

    // Add an instance
    state
        .store
        .put_instance(&InstanceState {
            id: "inst-0".to_string(),
            deployment_id: "default/api".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 32 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        })
        .unwrap();

    // Add a node
    state
        .store
        .put_node(&NodeInfo {
            id: "node-1".to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            capacity_memory_bytes: 8 * 1024 * 1024 * 1024,
            capacity_cpu_weight: 1000,
            used_memory_bytes: 2 * 1024 * 1024 * 1024,
            used_cpu_weight: 300,
            labels: HashMap::new(),
            last_heartbeat: 1000,
        })
        .unwrap();

    state
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── 1. All pages render with populated state (200 + HTML) ──────────

#[tokio::test]
async fn overview_page_renders_200_with_html() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(html.contains("<html"), "overview should return HTML");
    assert!(
        html.contains("api") || html.contains("Overview"),
        "overview should contain deployment info or page title"
    );
}

#[tokio::test]
async fn deployments_page_renders_200_with_html() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/deployments")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(
        html.contains("<html"),
        "deployments page should return HTML"
    );
}

#[tokio::test]
async fn nodes_page_renders_200_with_html() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/nodes")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(html.contains("<html"), "nodes page should return HTML");
    assert!(html.contains("node-1"), "nodes page should show node id");
}

#[tokio::test]
async fn rollouts_page_renders_200_with_html() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/rollouts")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(html.contains("<html"), "rollouts page should return HTML");
}

#[tokio::test]
async fn density_demo_page_renders_200() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/density-demo")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(
        html.contains("<html"),
        "density demo page should return HTML"
    );
}

#[tokio::test]
async fn empty_state_pages_still_render() {
    let state = test_state();
    let router = dashboard_router(state);

    let pages = ["/", "/deployments", "/nodes", "/rollouts", "/density-demo"];

    for page in pages {
        let req = Request::builder().uri(page).body(Body::empty()).unwrap();

        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "page {page} should render 200 even with empty state"
        );
    }
}

// ── 2. Deployment detail page with metrics ─────────────────────────

#[tokio::test]
async fn deployment_detail_renders_with_metrics() {
    let state = populated_state();

    // Add metrics for the deployment
    state
        .store
        .put_metrics(&MetricsSnapshot {
            deployment_id: "default/api".to_string(),
            epoch: 1000,
            rps: 150.0,
            latency_p50_ms: 5.0,
            latency_p99_ms: 25.0,
            error_rate: 0.02,
            total_memory_bytes: 64 * 1024 * 1024,
            active_instances: 2,
        })
        .unwrap();

    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/deployments/default%2Fapi")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(html.contains("<html"), "detail page should return HTML");
}

#[tokio::test]
async fn deployment_detail_missing_deployment_still_renders() {
    let state = test_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/deployments/nonexistent%2Fdeployment")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    // The dashboard renders a placeholder for missing deployments, not 404
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn node_detail_renders() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .uri("/nodes/node-1")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(html.contains("node-1"), "node detail should show node id");
}

// ── 3. Dashboard scale action validates boundaries ─────────────────

#[tokio::test]
async fn scale_action_within_bounds_succeeds() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/deployments/default%2Fapi/scale")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("target=5"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(
        html.contains("Scaling"),
        "successful scale should show scaling message"
    );
}

#[tokio::test]
async fn scale_action_exceeds_max_returns_warning() {
    let state = populated_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/deployments/default%2Fapi/scale")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("target=100"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(
        html.contains("exceeds max"),
        "over-max scale should return warning about exceeding max"
    );
}

#[tokio::test]
async fn scale_action_missing_deployment_returns_not_found() {
    let state = test_state();
    let router = dashboard_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/deployments/nonexistent/scale")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("target=1"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_string(resp).await;
    assert!(
        html.contains("not found"),
        "missing deployment should return not found message"
    );
}

// ── 4. Density demo deploy and teardown idempotent ─────────────────

#[tokio::test]
async fn density_demo_deploy_creates_resources() {
    let state = test_state();
    let router = dashboard_router(state.clone());

    let req = Request::builder()
        .method("POST")
        .uri("/density-demo/deploy")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("instance_count=10"))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    // deploy_demo returns a redirect on success
    assert!(
        resp.status().is_redirection(),
        "deploy should redirect, got: {}",
        resp.status()
    );

    // Verify deployment was created
    let dep = state
        .store
        .get_deployment(views::DENSITY_DEMO_DEPLOYMENT_ID)
        .unwrap();
    assert!(dep.is_some(), "density demo deployment should exist");

    // Verify instances were created
    let instances = state
        .store
        .list_instances_for_deployment(views::DENSITY_DEMO_DEPLOYMENT_ID)
        .unwrap();
    assert_eq!(instances.len(), 10, "should have 10 instances");
}

#[tokio::test]
async fn density_demo_deploy_is_idempotent() {
    let state = test_state();
    let router = dashboard_router(state.clone());

    // First deploy
    let req = Request::builder()
        .method("POST")
        .uri("/density-demo/deploy")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("instance_count=10"))
        .unwrap();
    router.clone().oneshot(req).await.unwrap();

    // Second deploy with different count
    let req = Request::builder()
        .method("POST")
        .uri("/density-demo/deploy")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("instance_count=50"))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert!(resp.status().is_redirection());

    // Still should have 10 instances (idempotent, not 50)
    let instances = state
        .store
        .list_instances_for_deployment(views::DENSITY_DEMO_DEPLOYMENT_ID)
        .unwrap();
    assert_eq!(
        instances.len(),
        10,
        "idempotent deploy should not create additional instances"
    );
}

#[tokio::test]
async fn density_demo_teardown_removes_everything() {
    let state = test_state();
    let router = dashboard_router(state.clone());

    // Deploy first
    let req = Request::builder()
        .method("POST")
        .uri("/density-demo/deploy")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("instance_count=5"))
        .unwrap();
    router.clone().oneshot(req).await.unwrap();

    // Teardown
    let req = Request::builder()
        .method("POST")
        .uri("/density-demo/teardown")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert!(resp.status().is_redirection());

    // Verify everything is gone
    let dep = state
        .store
        .get_deployment(views::DENSITY_DEMO_DEPLOYMENT_ID)
        .unwrap();
    assert!(dep.is_none(), "deployment should be removed");

    let instances = state
        .store
        .list_instances_for_deployment(views::DENSITY_DEMO_DEPLOYMENT_ID)
        .unwrap();
    assert!(instances.is_empty(), "instances should be removed");
}

#[tokio::test]
async fn density_demo_teardown_is_idempotent() {
    let state = test_state();
    let router = dashboard_router(state.clone());

    // Teardown without deploying first should still redirect successfully
    let req = Request::builder()
        .method("POST")
        .uri("/density-demo/teardown")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert!(
        resp.status().is_redirection(),
        "teardown on empty state should still redirect"
    );
}

// ── 5. View builder helpers ────────────────────────────────────────

#[test]
fn format_bytes_covers_all_units() {
    assert_eq!(views::format_bytes(0), "0 B");
    assert_eq!(views::format_bytes(500), "500 B");
    assert_eq!(views::format_bytes(1023), "1023 B");
    assert_eq!(views::format_bytes(1024), "1 KB");
    assert_eq!(views::format_bytes(2048), "2 KB");
    assert_eq!(views::format_bytes(1024 * 1024), "1 MB");
    assert_eq!(views::format_bytes(64 * 1024 * 1024), "64 MB");
    assert_eq!(views::format_bytes(1024 * 1024 * 1024), "1.0 GB");
    assert_eq!(views::format_bytes(1536 * 1024 * 1024), "1.5 GB");
    assert_eq!(views::format_bytes(8 * 1024 * 1024 * 1024), "8.0 GB");
}

#[test]
fn format_relative_time_zero_returns_never() {
    assert_eq!(views::format_relative_time(0), "never");
}

#[test]
fn format_relative_time_recent_shows_seconds() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let result = views::format_relative_time(now - 30);
    assert!(
        result.contains("30s ago"),
        "expected '30s ago', got: {result}"
    );
}

#[test]
fn format_relative_time_shows_minutes() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let result = views::format_relative_time(now - 120);
    assert!(
        result.contains("2m ago"),
        "expected '2m ago', got: {result}"
    );
}

#[test]
fn format_relative_time_shows_hours() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let result = views::format_relative_time(now - 7200);
    assert!(
        result.contains("2h ago"),
        "expected '2h ago', got: {result}"
    );
}

#[test]
fn format_relative_time_shows_days() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let result = views::format_relative_time(now - 172800);
    assert!(
        result.contains("2d ago"),
        "expected '2d ago', got: {result}"
    );
}

#[test]
fn resource_bar_memory_percent_calculation() {
    let bar = views::ResourceBar::memory(512 * 1024 * 1024, 1024 * 1024 * 1024);
    assert!(
        (bar.percent - 50.0).abs() < 0.1,
        "expected ~50%, got {}",
        bar.percent
    );
    assert_eq!(bar.bar_color(), "bg-grid-accent");
    assert_eq!(bar.used_display, "512 MB");
    assert_eq!(bar.total_display, "1.0 GB");
}

#[test]
fn resource_bar_cpu_percent_calculation() {
    let bar = views::ResourceBar::cpu(300, 1000);
    assert!(
        (bar.percent - 30.0).abs() < 0.1,
        "expected ~30%, got {}",
        bar.percent
    );
    assert_eq!(bar.bar_color(), "bg-grid-accent");
}

#[test]
fn resource_bar_color_thresholds() {
    // Under 70% = accent
    let bar = views::ResourceBar::memory(600 * 1024 * 1024, 1024 * 1024 * 1024);
    assert_eq!(bar.bar_color(), "bg-grid-accent");

    // 70-90% = warn
    let bar = views::ResourceBar::memory(800 * 1024 * 1024, 1024 * 1024 * 1024);
    assert_eq!(bar.bar_color(), "bg-grid-warn");

    // Over 90% = danger
    let bar = views::ResourceBar::memory(950 * 1024 * 1024, 1024 * 1024 * 1024);
    assert_eq!(bar.bar_color(), "bg-grid-danger");
}

#[test]
fn resource_bar_zero_total_produces_zero_percent() {
    let bar = views::ResourceBar::memory(0, 0);
    assert_eq!(bar.percent, 0.0);
    assert_eq!(bar.bar_color(), "bg-grid-accent");
}

#[test]
fn build_metrics_rows_normalizes_bar_widths() {
    let snaps = vec![
        MetricsSnapshot {
            deployment_id: "d".to_string(),
            epoch: 1000,
            rps: 100.0,
            latency_p50_ms: 5.0,
            latency_p99_ms: 50.0,
            error_rate: 0.01,
            total_memory_bytes: 64 * 1024 * 1024,
            active_instances: 3,
        },
        MetricsSnapshot {
            deployment_id: "d".to_string(),
            epoch: 1060,
            rps: 200.0,
            latency_p50_ms: 10.0,
            latency_p99_ms: 100.0,
            error_rate: 0.03,
            total_memory_bytes: 128 * 1024 * 1024,
            active_instances: 5,
        },
    ];
    let rows = views::build_metrics_rows(&snaps);
    assert_eq!(rows.len(), 2);

    // First row rps=100 should be 50% of max rps=200
    assert!(
        (rows[0].rps_bar_width - 50.0).abs() < 0.1,
        "first row rps bar should be ~50%"
    );
    // Second row rps=200 should be 100% of max
    assert!(
        (rows[1].rps_bar_width - 100.0).abs() < 0.1,
        "second row rps bar should be ~100%"
    );
}

#[test]
fn build_metrics_rows_empty_input_returns_empty() {
    let rows = views::build_metrics_rows(&[]);
    assert!(rows.is_empty());
}

#[test]
fn deployment_view_from_spec_computes_fields() {
    let spec = test_deployment("default", "api");
    let instances = vec![
        InstanceState {
            id: "inst-0".to_string(),
            deployment_id: "default/api".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 32 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        },
        InstanceState {
            id: "inst-1".to_string(),
            deployment_id: "default/api".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Stopped,
            health: HealthStatus::Unknown,
            restart_count: 1,
            memory_bytes: 0,
            started_at: 1000,
            updated_at: 1000,
        },
    ];

    let view = views::DeploymentView::from_spec(&spec, &instances, None);
    assert_eq!(view.name, "api");
    assert_eq!(view.instances_running, 1);
    assert_eq!(view.instances_max, 10);
    assert_eq!(view.health_dots.len(), 2);
    assert_eq!(view.trigger_display, "HTTP :8080");
    assert!(view.latest_rps.is_none());
}

#[test]
fn cluster_summary_counts_correctly() {
    let deployments = vec![
        test_deployment("default", "a"),
        test_deployment("default", "b"),
        test_deployment("prod", "c"),
    ];

    let instances = vec![
        InstanceState {
            id: "i-0".to_string(),
            deployment_id: "default/a".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 32 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        },
        InstanceState {
            id: "i-1".to_string(),
            deployment_id: "default/b".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Stopped,
            health: HealthStatus::Unknown,
            restart_count: 0,
            memory_bytes: 0,
            started_at: 1000,
            updated_at: 1000,
        },
    ];

    let summary = views::build_cluster_summary(&deployments, &instances, &[], 2);
    assert_eq!(summary.deployment_count, 3);
    assert_eq!(summary.instances.running, 1);
    assert_eq!(summary.instances.stopped, 1);
    assert_eq!(summary.instances.total, 2);
    assert_eq!(summary.active_rollouts, 2);
    assert_eq!(summary.node_count, 0);
}
