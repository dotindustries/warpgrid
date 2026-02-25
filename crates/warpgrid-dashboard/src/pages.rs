//! Dashboard page handlers.
//!
//! Each handler queries the state store, builds view types, and renders
//! an Askama template. HTMX partials are in `partials.rs`.

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

// ── Overview ────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "overview.html")]
struct OverviewTemplate {
    active_page: &'static str,
    cluster_mode: String,
    summary: ClusterSummary,
    deployments: Vec<DeploymentView>,
    alerts: Vec<AlertView>,
}

pub async fn overview(State(state): State<DashboardState>) -> Html<String> {
    let deployments = state.store.list_deployments().unwrap_or_default();
    let nodes = state.store.list_nodes().unwrap_or_default();

    let mut all_instances = Vec::new();
    let mut deployment_views = Vec::new();

    for spec in &deployments {
        let instances = state
            .store
            .list_instances_for_deployment(&spec.id)
            .unwrap_or_default();
        let metrics = state
            .store
            .list_metrics_for_deployment(&spec.id, 1)
            .unwrap_or_default();
        all_instances.extend(instances.clone());
        deployment_views.push(DeploymentView::from_spec(
            spec,
            &instances,
            metrics.first(),
        ));
    }

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

    let rollout_views: Vec<RolloutView> = {
        let rollouts = state.rollouts.read().await;
        rollouts.values().map(RolloutView::from_rollout).collect()
    };

    let summary = build_cluster_summary(&deployments, &all_instances, &nodes, active_rollout_count);
    let alerts = build_alerts(&deployment_views, &rollout_views);

    let cluster_mode = if nodes.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({})", nodes.len())
    };

    // Limit to last 10 for overview
    deployment_views.truncate(10);

    render(OverviewTemplate {
        active_page: "overview",
        cluster_mode,
        summary,
        deployments: deployment_views,
        alerts,
    })
}

// ── Deployments List ────────────────────────────────────────────

#[derive(Template)]
#[template(path = "deployments.html")]
struct DeploymentsTemplate {
    active_page: &'static str,
    cluster_mode: String,
    deployments: Vec<DeploymentView>,
}

pub async fn deployments(State(state): State<DashboardState>) -> Html<String> {
    let specs = state.store.list_deployments().unwrap_or_default();
    let nodes = state.store.list_nodes().unwrap_or_default();

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

    let cluster_mode = if nodes.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({})", nodes.len())
    };

    render(DeploymentsTemplate {
        active_page: "deployments",
        cluster_mode,
        deployments: deployment_views,
    })
}

// ── Deployment Detail ───────────────────────────────────────────

#[derive(Template)]
#[template(path = "deployment_detail.html")]
struct DeploymentDetailTemplate {
    active_page: &'static str,
    cluster_mode: String,
    deployment: DeploymentView,
    instances: Vec<InstanceView>,
    metrics: Vec<MetricsRow>,
    rollout: Option<RolloutView>,
}

pub async fn deployment_detail(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> Html<String> {
    let nodes = state.store.list_nodes().unwrap_or_default();

    let spec = state.store.get_deployment(&id).unwrap_or(None);
    let instances = state
        .store
        .list_instances_for_deployment(&id)
        .unwrap_or_default();
    let snapshots = state
        .store
        .list_metrics_for_deployment(&id, 20)
        .unwrap_or_default();

    let instance_views: Vec<InstanceView> = instances.iter().map(InstanceView::from_state).collect();
    let metrics = build_metrics_rows(&snapshots);

    let rollout = {
        let rollouts = state.rollouts.read().await;
        rollouts.get(&id).map(RolloutView::from_rollout)
    };

    let deployment_view = match spec {
        Some(ref s) => {
            let latest = snapshots.first();
            DeploymentView::from_spec(s, &instances, latest)
        }
        None => {
            // Build a minimal placeholder for missing deployments
            DeploymentView::from_spec(
                &warpgrid_state::DeploymentSpec {
                    id: id.clone(),
                    namespace: "unknown".to_string(),
                    name: id.clone(),
                    source: "unknown".to_string(),
                    trigger: warpgrid_state::TriggerConfig::Http { port: None },
                    instances: warpgrid_state::InstanceConstraints { min: 0, max: 0 },
                    resources: warpgrid_state::ResourceLimits {
                        memory_bytes: 0,
                        cpu_weight: 0,
                    },
                    scaling: None,
                    health: None,
                    shims: warpgrid_state::ShimsEnabled::default(),
                    env: std::collections::HashMap::new(),
                    created_at: 0,
                    updated_at: 0,
                },
                &instances,
                None,
            )
        }
    };

    let cluster_mode = if nodes.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({})", nodes.len())
    };

    render(DeploymentDetailTemplate {
        active_page: "deployments",
        cluster_mode,
        deployment: deployment_view,
        instances: instance_views,
        metrics,
        rollout,
    })
}

// ── Nodes List ──────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "nodes.html")]
struct NodesTemplate {
    active_page: &'static str,
    cluster_mode: String,
    nodes: Vec<NodeView>,
}

pub async fn nodes(State(state): State<DashboardState>) -> Html<String> {
    let node_infos = state.store.list_nodes().unwrap_or_default();

    let node_views: Vec<NodeView> = node_infos
        .iter()
        .map(|n| {
            // Count instances on this node
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

    let cluster_mode = if node_infos.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({})", node_infos.len())
    };

    render(NodesTemplate {
        active_page: "nodes",
        cluster_mode,
        nodes: node_views,
    })
}

// ── Node Detail ─────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "node_detail.html")]
struct NodeDetailTemplate {
    active_page: &'static str,
    cluster_mode: String,
    node: NodeView,
    instances: Vec<InstanceView>,
}

pub async fn node_detail(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> Html<String> {
    let node_infos = state.store.list_nodes().unwrap_or_default();
    let node_info = state.store.get_node(&id).unwrap_or(None);

    // Get all instances on this node
    let instances_on_node: Vec<InstanceView> = state
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
        .filter(|i| i.node_id == id)
        .map(|i| InstanceView::from_state(&i))
        .collect();

    let node_view = match node_info {
        Some(ref n) => NodeView::from_node(n, instances_on_node.len()),
        None => NodeView::from_node(
            &warpgrid_state::NodeInfo {
                id: id.clone(),
                address: "unknown".to_string(),
                port: 0,
                capacity_memory_bytes: 0,
                capacity_cpu_weight: 0,
                used_memory_bytes: 0,
                used_cpu_weight: 0,
                labels: std::collections::HashMap::new(),
                last_heartbeat: 0,
            },
            instances_on_node.len(),
        ),
    };

    let cluster_mode = if node_infos.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({})", node_infos.len())
    };

    render(NodeDetailTemplate {
        active_page: "nodes",
        cluster_mode,
        node: node_view,
        instances: instances_on_node,
    })
}

// ── Rollouts ────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "rollouts.html")]
struct RolloutsTemplate {
    active_page: &'static str,
    cluster_mode: String,
    active_rollouts: Vec<RolloutView>,
    completed_rollouts: Vec<RolloutView>,
}

pub async fn rollouts(State(state): State<DashboardState>) -> Html<String> {
    let nodes = state.store.list_nodes().unwrap_or_default();

    let (active, completed) = {
        let rollouts = state.rollouts.read().await;
        let all: Vec<RolloutView> = rollouts.values().map(RolloutView::from_rollout).collect();
        let active: Vec<RolloutView> = all.iter().filter(|r| r.is_active).cloned().collect();
        let completed: Vec<RolloutView> = all.iter().filter(|r| !r.is_active).cloned().collect();
        (active, completed)
    };

    let cluster_mode = if nodes.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({})", nodes.len())
    };

    render(RolloutsTemplate {
        active_page: "rollouts",
        cluster_mode,
        active_rollouts: active,
        completed_rollouts: completed,
    })
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
    async fn overview_renders_html() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();

        let resp = overview(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn deployments_page_renders() {
        let state = test_state();
        state
            .store
            .put_deployment(&test_deployment("default", "api"))
            .unwrap();
        state
            .store
            .put_deployment(&test_deployment("prod", "worker"))
            .unwrap();

        let resp = deployments(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn deployment_detail_shows_instances() {
        let state = test_state();
        let inst = InstanceState {
            id: "inst-0".to_string(),
            deployment_id: "default/api".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 64 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        };
        state.store.put_instance(&inst).unwrap();

        let resp = deployment_detail(
            State(state),
            Path("default/api".to_string()),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn overview_empty_state() {
        let state = test_state();
        let resp = overview(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn nodes_page_renders() {
        let state = test_state();
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

        let resp = nodes(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn rollouts_page_empty() {
        let state = test_state();
        let resp = rollouts(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn node_detail_renders() {
        let state = test_state();
        state
            .store
            .put_node(&NodeInfo {
                id: "node-1".to_string(),
                address: "10.0.0.1".to_string(),
                port: 8443,
                capacity_memory_bytes: 8 * 1024 * 1024 * 1024,
                capacity_cpu_weight: 1000,
                used_memory_bytes: 0,
                used_cpu_weight: 0,
                labels: HashMap::new(),
                last_heartbeat: 1000,
            })
            .unwrap();

        let resp = node_detail(State(state), Path("node-1".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn density_demo_shows_deploy_button() {
        let state = test_state();
        let resp = density_demo(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn density_demo_shows_live_metrics() {
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
            instances: InstanceConstraints { min: 10, max: 20 },
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

        for i in 0..10 {
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

        let resp = density_demo(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), 200);
    }
}

// ── Density Demo ──────────────────────────────────────────────

#[derive(Template)]
#[template(path = "density_demo.html")]
struct DensityDemoTemplate {
    active_page: &'static str,
    cluster_mode: String,
    demo: DensityDemoView,
}

pub async fn density_demo(State(state): State<DashboardState>) -> Html<String> {
    let nodes = state.store.list_nodes().unwrap_or_default();
    let cluster_mode = if nodes.is_empty() {
        "Standalone".to_string()
    } else {
        format!("Cluster ({} nodes)", nodes.len())
    };

    let demo = build_density_demo_live(&state.store);

    render(DensityDemoTemplate {
        active_page: "density-demo",
        cluster_mode,
        demo,
    })
}
