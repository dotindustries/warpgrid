//! HTMX partial endpoints.
//!
//! These return HTML fragments (not full pages) for HTMX to swap
//! into specific DOM sections, enabling live updates without full reloads.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Html;

use warpgrid_rollout::RolloutPhase;

use crate::DashboardState;
use crate::views::*;

fn render<T: Template>(tmpl: T) -> Html<String> {
    Html(tmpl.render().unwrap_or_else(|e| {
        format!("<pre>Template error: {e}</pre>")
    }))
}

// ── Overview Stats ──────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/stats.html")]
struct StatsPartial {
    summary: ClusterSummary,
}

pub async fn overview_stats(State(state): State<DashboardState>) -> Html<String> {
    let deployments = state.store.list_deployments().unwrap_or_default();
    let nodes = state.store.list_nodes().unwrap_or_default();

    let all_instances: Vec<warpgrid_state::InstanceState> = deployments
        .iter()
        .flat_map(|d| {
            state
                .store
                .list_instances_for_deployment(&d.id)
                .unwrap_or_default()
        })
        .collect();

    let active_rollout_count = {
        let rollouts = state.rollouts.read().await;
        rollouts
            .values()
            .filter(|r| {
                !matches!(
                    r.phase,
                    RolloutPhase::Completed | RolloutPhase::RolledBack { .. }
                )
            })
            .count()
    };

    let summary = build_cluster_summary(&deployments, &all_instances, &nodes, active_rollout_count);

    render(StatsPartial { summary })
}

// ── Deployments Table ───────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/deployment_rows.html")]
struct DeploymentRowsPartial {
    deployments: Vec<DeploymentView>,
}

pub async fn deployments_table(State(state): State<DashboardState>) -> Html<String> {
    let specs = state.store.list_deployments().unwrap_or_default();

    let deployment_views: Vec<DeploymentView> = specs
        .iter()
        .map(|spec| {
            let instances = state
                .store
                .list_instances_for_deployment(&spec.id)
                .unwrap_or_default();
            let metrics = state
                .store
                .list_metrics_for_deployment(&spec.id, 1)
                .unwrap_or_default();
            DeploymentView::from_spec(spec, &instances, metrics.first())
        })
        .collect();

    render(DeploymentRowsPartial {
        deployments: deployment_views,
    })
}

// ── Instance Table ──────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/instance_table.html")]
struct InstanceTablePartial {
    instances: Vec<InstanceView>,
}

pub async fn deployment_instances(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> Html<String> {
    let instances = state
        .store
        .list_instances_for_deployment(&id)
        .unwrap_or_default();

    let instance_views: Vec<InstanceView> = instances.iter().map(InstanceView::from_state).collect();

    render(InstanceTablePartial {
        instances: instance_views,
    })
}

// ── Rollout Cards ───────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/rollout_cards.html")]
struct RolloutCardsPartial {
    active_rollouts: Vec<RolloutView>,
}

pub async fn rollout_cards(State(state): State<DashboardState>) -> Html<String> {
    let active: Vec<RolloutView> = {
        let rollouts = state.rollouts.read().await;
        rollouts
            .values()
            .map(RolloutView::from_rollout)
            .filter(|r| r.is_active)
            .collect()
    };

    render(RolloutCardsPartial {
        active_rollouts: active,
    })
}

// ── Node Cards ──────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/node_cards.html")]
struct NodeCardsPartial {
    nodes: Vec<NodeView>,
}

pub async fn node_cards(State(state): State<DashboardState>) -> Html<String> {
    let node_infos = state.store.list_nodes().unwrap_or_default();

    let node_views: Vec<NodeView> = node_infos
        .iter()
        .map(|n| {
            let count = state
                .store
                .list_deployments()
                .unwrap_or_default()
                .iter()
                .flat_map(|d| {
                    state
                        .store
                        .list_instances_for_deployment(&d.id)
                        .unwrap_or_default()
                })
                .filter(|i| i.node_id == n.id)
                .count();
            NodeView::from_node(n, count)
        })
        .collect();

    render(NodeCardsPartial { nodes: node_views })
}

// ── Density Stats ───────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/density_stats.html")]
struct DensityStatsPartial {
    demo: crate::views::DensityDemoView,
}

pub async fn density_stats(State(state): State<DashboardState>) -> Html<String> {
    let demo = crate::views::build_density_demo_live(&state.store);
    render(DensityStatsPartial { demo })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
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

    #[tokio::test]
    async fn overview_stats_partial_renders() {
        let state = test_state();
        let resp = overview_stats(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn deployments_table_partial_renders() {
        let state = test_state();
        let resp = deployments_table(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn instance_table_partial_renders() {
        let state = test_state();
        let resp = deployment_instances(
            State(state),
            Path("default/api".to_string()),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn rollout_cards_partial_renders() {
        let state = test_state();
        let resp = rollout_cards(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn node_cards_partial_renders() {
        let state = test_state();
        let resp = node_cards(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn density_stats_partial_renders() {
        let state = test_state();
        let resp = density_stats(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn density_stats_with_deployment() {
        let state = test_state();

        // Create the demo deployment
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let spec = DeploymentSpec {
            id: crate::views::DENSITY_DEMO_DEPLOYMENT_ID.to_string(),
            namespace: "demo".to_string(),
            name: "wastebin-density".to_string(),
            source: "file://demos/wastebin/wastebin-demo.wasm".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 5, max: 10 },
            resources: ResourceLimits {
                memory_bytes: 16 * 1024 * 1024,
                cpu_weight: 50,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: now,
            updated_at: now,
        };
        state.store.put_deployment(&spec).unwrap();

        // Add some instances
        for i in 0..5 {
            state
                .store
                .put_instance(&InstanceState {
                    id: format!("demo-wb-{i:04}"),
                    deployment_id: crate::views::DENSITY_DEMO_DEPLOYMENT_ID.to_string(),
                    node_id: "standalone".to_string(),
                    status: InstanceStatus::Running,
                    health: HealthStatus::Healthy,
                    restart_count: 0,
                    memory_bytes: 3 * 1024 * 1024,
                    started_at: now,
                    updated_at: now,
                })
                .unwrap();
        }

        let resp = density_stats(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }
}
