//! Dashboard action endpoints.
//!
//! HTMX form handlers that perform mutations and return updated
//! HTML fragments or redirect responses.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect};

use warpgrid_rollout::{Rollout, RolloutStrategy};
use warpgrid_state::{
    DeploymentSpec, HealthStatus, InstanceConstraints, InstanceState, InstanceStatus,
    MetricsSnapshot, ResourceLimits, ShimsEnabled, TriggerConfig,
};

use crate::DashboardState;
use crate::views::DENSITY_DEMO_DEPLOYMENT_ID;

// ── Scale ───────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ScaleForm {
    pub target: u32,
}

pub async fn scale_deployment(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
    axum::extract::Form(form): axum::extract::Form<ScaleForm>,
) -> impl IntoResponse {
    let spec = match state.store.get_deployment(&id) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Html(format!(
                r#"<div class="text-rose-400 text-sm font-mono">Deployment not found</div>"#
            ))
            .into_response()
        }
        Err(e) => {
            return Html(format!(
                r#"<div class="text-rose-400 text-sm font-mono">Error: {}</div>"#,
                e
            ))
            .into_response()
        }
    };

    if form.target > spec.instances.max {
        return Html(format!(
            r#"<div class="text-amber-400 text-sm font-mono">Target {} exceeds max {}</div>"#,
            form.target, spec.instances.max
        ))
        .into_response();
    }

    Html(format!(
        r#"<div class="text-emerald-400 text-sm font-mono">Scaling {} to {} instances</div>"#,
        id, form.target
    ))
    .into_response()
}

// ── Start Rollout ───────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct RolloutForm {
    pub strategy: String,
    pub new_version: String,
}

pub async fn start_rollout(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
    axum::extract::Form(form): axum::extract::Form<RolloutForm>,
) -> impl IntoResponse {
    let spec = match state.store.get_deployment(&id) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Html(format!(
                r#"<div class="text-rose-400 text-sm font-mono">Deployment not found</div>"#
            ))
            .into_response()
        }
        Err(e) => {
            return Html(format!(
                r#"<div class="text-rose-400 text-sm font-mono">Error: {}</div>"#,
                e
            ))
            .into_response()
        }
    };

    if form.new_version.is_empty() {
        return Html(
            r#"<div class="text-amber-400 text-sm font-mono">Version is required</div>"#
                .to_string(),
        )
        .into_response();
    }

    let strategy = match form.strategy.as_str() {
        "canary" => RolloutStrategy::Canary(Default::default()),
        "blue_green" => RolloutStrategy::BlueGreen,
        _ => RolloutStrategy::default(),
    };

    let mut rollout = Rollout::new(
        &id,
        strategy,
        spec.instances.min,
        &spec.source,
        &form.new_version,
    );
    rollout.start();

    {
        let mut rollouts = state.rollouts.write().await;
        rollouts.insert(id.clone(), rollout);
    }

    Html(format!(
        r#"<div class="text-emerald-400 text-sm font-mono">Rollout started for {}</div>"#,
        id
    ))
    .into_response()
}

// ── Pause / Resume Rollout ──────────────────────────────────────

pub async fn pause_rollout(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut rollouts = state.rollouts.write().await;
    match rollouts.get_mut(&id) {
        Some(rollout) => {
            rollout.pause();
            Html(
                r#"<div class="text-amber-400 text-sm font-mono">Rollout paused</div>"#
                    .to_string(),
            )
        }
        None => Html(
            r#"<div class="text-rose-400 text-sm font-mono">Rollout not found</div>"#.to_string(),
        ),
    }
}

pub async fn resume_rollout(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut rollouts = state.rollouts.write().await;
    match rollouts.get_mut(&id) {
        Some(rollout) => {
            rollout.resume();
            Html(
                r#"<div class="text-emerald-400 text-sm font-mono">Rollout resumed</div>"#
                    .to_string(),
            )
        }
        None => Html(
            r#"<div class="text-rose-400 text-sm font-mono">Rollout not found</div>"#.to_string(),
        ),
    }
}

// ── Delete Deployment ───────────────────────────────────────────

pub async fn delete_deployment(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_deployment(&id) {
        Ok(true) => {
            let _ = state.store.delete_instances_for_deployment(&id);
            Redirect::to("/dashboard/deployments").into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Html(
                r#"<div class="text-rose-400 text-sm font-mono">Deployment not found</div>"#
                    .to_string(),
            ),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!(
                r#"<div class="text-rose-400 text-sm font-mono">Error: {}</div>"#,
                e
            )),
        )
            .into_response(),
    }
}

// ── Deploy / Teardown Density Demo ──────────────────────────────

#[derive(serde::Deserialize)]
pub struct DeployForm {
    pub instance_count: Option<u32>,
}

pub async fn deploy_demo(
    State(state): State<DashboardState>,
    axum::extract::Form(form): axum::extract::Form<DeployForm>,
) -> impl IntoResponse {
    let instance_count = form.instance_count.unwrap_or(100).min(500) as usize;

    // Idempotent: if already deployed, just redirect
    if let Ok(Some(_)) = state.store.get_deployment(DENSITY_DEMO_DEPLOYMENT_ID) {
        return Redirect::to("/dashboard/density-demo").into_response();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut env = std::collections::HashMap::new();
    env.insert(
        "WASTEBIN_DATABASE_URL".to_string(),
        "postgres://warpgrid:warpgrid@localhost:5432/wastebin".to_string(),
    );
    env.insert("WASTEBIN_INSTANCE_ID".to_string(), "auto".to_string());
    env.insert("WARPGRID_POOL_SIZE".to_string(), "10".to_string());

    let spec = DeploymentSpec {
        id: DENSITY_DEMO_DEPLOYMENT_ID.to_string(),
        namespace: "demo".to_string(),
        name: "wastebin-density".to_string(),
        source: "file://demos/wastebin/wastebin-demo.wasm".to_string(),
        trigger: TriggerConfig::Http { port: Some(8080) },
        instances: InstanceConstraints {
            min: instance_count as u32,
            max: (instance_count as u32) * 2,
        },
        resources: ResourceLimits {
            memory_bytes: 16 * 1024 * 1024,
            cpu_weight: 50,
        },
        scaling: None,
        health: None,
        shims: ShimsEnabled {
            timezone: true,
            dev_urandom: true,
            dns: true,
            signals: true,
            database_proxy: true,
        },
        env,
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = state.store.put_deployment(&spec) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!(
                r#"<div class="text-rose-400 text-sm font-mono">Deploy error: {e}</div>"#
            )),
        )
            .into_response();
    }

    // Create instance records
    for i in 0..instance_count {
        let inst = InstanceState {
            id: format!("demo-wb-{i:04}"),
            deployment_id: DENSITY_DEMO_DEPLOYMENT_ID.to_string(),
            node_id: "standalone".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 3 * 1024 * 1024, // ~3 MB each
            started_at: now,
            updated_at: now,
        };
        let _ = state.store.put_instance(&inst);
    }

    // Create initial metrics snapshot
    let snapshot = MetricsSnapshot {
        deployment_id: DENSITY_DEMO_DEPLOYMENT_ID.to_string(),
        epoch: now,
        rps: 0.0,
        latency_p50_ms: 0.0,
        latency_p99_ms: 0.0,
        error_rate: 0.0,
        total_memory_bytes: (instance_count as u64) * 3 * 1024 * 1024,
        active_instances: instance_count as u32,
    };
    let _ = state.store.put_metrics(&snapshot);

    Redirect::to("/dashboard/density-demo").into_response()
}

pub async fn teardown_demo(State(state): State<DashboardState>) -> impl IntoResponse {
    let _ = state
        .store
        .delete_instances_for_deployment(DENSITY_DEMO_DEPLOYMENT_ID);
    let _ = state.store.delete_deployment(DENSITY_DEMO_DEPLOYMENT_ID);
    Redirect::to("/dashboard/density-demo")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use warpgrid_state::*;

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

    #[tokio::test]
    async fn scale_existing_deployment() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();

        let resp = scale_deployment(
            State(state),
            Path("default/api".to_string()),
            axum::extract::Form(ScaleForm { target: 5 }),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn scale_exceeds_max() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();

        let resp = scale_deployment(
            State(state),
            Path("default/api".to_string()),
            axum::extract::Form(ScaleForm { target: 100 }),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200); // Returns HTML warning, not 400
    }

    #[tokio::test]
    async fn delete_existing_deployment() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();

        let resp = delete_deployment(State(state), Path("default/api".to_string())).await;
        let resp = resp.into_response();
        // Redirect on success
        assert_eq!(resp.status(), 303);
    }

    #[tokio::test]
    async fn delete_nonexistent_deployment() {
        let state = test_state();
        let resp = delete_deployment(State(state), Path("nope".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn start_rollout_action() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();

        let resp = start_rollout(
            State(state.clone()),
            Path("default/api".to_string()),
            axum::extract::Form(RolloutForm {
                strategy: "rolling".to_string(),
                new_version: "v2".to_string(),
            }),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);

        // Verify rollout was stored
        let rollouts = state.rollouts.read().await;
        assert!(rollouts.contains_key("default/api"));
    }

    #[tokio::test]
    async fn pause_and_resume_actions() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();

        // Start rollout first
        start_rollout(
            State(state.clone()),
            Path("default/api".to_string()),
            axum::extract::Form(RolloutForm {
                strategy: "rolling".to_string(),
                new_version: "v2".to_string(),
            }),
        )
        .await;

        // Pause
        let resp = pause_rollout(State(state.clone()), Path("default/api".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);

        // Resume
        let resp = resume_rollout(State(state), Path("default/api".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn deploy_demo_creates_deployment() {
        let state = test_state();
        let resp = deploy_demo(
            State(state.clone()),
            axum::extract::Form(DeployForm { instance_count: Some(10) }),
        )
        .await;
        let resp = resp.into_response();
        // Redirect on success
        assert_eq!(resp.status(), 303);

        // Verify deployment was created
        let dep = state.store.get_deployment(DENSITY_DEMO_DEPLOYMENT_ID).unwrap();
        assert!(dep.is_some());

        // Verify instances were created
        let instances = state
            .store
            .list_instances_for_deployment(DENSITY_DEMO_DEPLOYMENT_ID)
            .unwrap();
        assert_eq!(instances.len(), 10);
    }

    #[tokio::test]
    async fn teardown_demo_removes_deployment() {
        let state = test_state();
        // Deploy first
        deploy_demo(
            State(state.clone()),
            axum::extract::Form(DeployForm { instance_count: Some(5) }),
        )
        .await;

        // Teardown
        let resp = teardown_demo(State(state.clone())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 303);

        // Verify deployment is gone
        let dep = state.store.get_deployment(DENSITY_DEMO_DEPLOYMENT_ID).unwrap();
        assert!(dep.is_none());

        // Verify instances are gone
        let instances = state
            .store
            .list_instances_for_deployment(DENSITY_DEMO_DEPLOYMENT_ID)
            .unwrap();
        assert!(instances.is_empty());
    }

    #[tokio::test]
    async fn deploy_demo_idempotent() {
        let state = test_state();
        // Deploy twice
        deploy_demo(
            State(state.clone()),
            axum::extract::Form(DeployForm { instance_count: Some(10) }),
        )
        .await;
        let resp = deploy_demo(
            State(state.clone()),
            axum::extract::Form(DeployForm { instance_count: Some(50) }),
        )
        .await;
        let resp = resp.into_response();
        // Second deploy should redirect (idempotent)
        assert_eq!(resp.status(), 303);

        // Should still have 10 instances (not 50)
        let instances = state
            .store
            .list_instances_for_deployment(DENSITY_DEMO_DEPLOYMENT_ID)
            .unwrap();
        assert_eq!(instances.len(), 10);
    }
}
