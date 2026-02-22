//! Dashboard page handlers.
//!
//! Returns server-rendered HTML for the dashboard pages.
//! Phase 1 uses plain HTML; Leptos SSR + hydration comes in Phase 1.1.

use axum::extract::State;
use axum::response::Html;

use crate::DashboardState;

/// Dashboard overview page.
pub async fn overview(State(state): State<DashboardState>) -> Html<String> {
    let deployments = state
        .store
        .list_deployments()
        .unwrap_or_default();
    let nodes = state.store.list_nodes().unwrap_or_default();

    let deployment_rows: String = deployments
        .iter()
        .map(|d| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}-{}</td></tr>",
                d.name, d.namespace, d.instances.min, d.instances.max
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        r#"<!DOCTYPE html>
<html>
<head><title>WarpGrid Dashboard</title>
<style>
  body {{ font-family: system-ui; max-width: 960px; margin: 2rem auto; padding: 0 1rem; }}
  table {{ width: 100%; border-collapse: collapse; }}
  th, td {{ padding: 0.5rem; text-align: left; border-bottom: 1px solid #ddd; }}
  nav a {{ margin-right: 1rem; }}
  .badge {{ display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 12px; }}
</style>
</head>
<body>
<h1>WarpGrid Dashboard</h1>
<nav>
  <a href="/dashboard/">Overview</a>
  <a href="/dashboard/deployments">Deployments</a>
</nav>
<hr>
<h2>Cluster Overview</h2>
<p>Nodes: {node_count} | Deployments: {deploy_count}</p>
<table>
<tr><th>Name</th><th>Namespace</th><th>Instances (min-max)</th></tr>
{deployment_rows}
</table>
</body>
</html>"#,
        node_count = nodes.len(),
        deploy_count = deployments.len(),
    ))
}

/// Deployments list page.
pub async fn deployments(State(state): State<DashboardState>) -> Html<String> {
    let deployments = state
        .store
        .list_deployments()
        .unwrap_or_default();

    let rows: String = deployments
        .iter()
        .map(|d| {
            let trigger = match &d.trigger {
                warpgrid_state::TriggerConfig::Http { port } => {
                    format!("HTTP (:{port})", port = port.unwrap_or(8080))
                }
                warpgrid_state::TriggerConfig::Cron { schedule } => {
                    format!("Cron ({schedule})")
                }
                warpgrid_state::TriggerConfig::Queue { topic } => {
                    format!("Queue ({topic})")
                }
            };
            format!(
                "<tr><td><a href=\"/dashboard/deployments/{id}\">{name}</a></td>\
                 <td>{ns}</td><td>{trigger}</td>\
                 <td>{min}-{max}</td><td>{source}</td></tr>",
                id = d.id,
                name = d.name,
                ns = d.namespace,
                min = d.instances.min,
                max = d.instances.max,
                source = d.source,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Deployments — WarpGrid</title>
<style>
  body {{ font-family: system-ui; max-width: 960px; margin: 2rem auto; padding: 0 1rem; }}
  table {{ width: 100%; border-collapse: collapse; }}
  th, td {{ padding: 0.5rem; text-align: left; border-bottom: 1px solid #ddd; }}
  nav a {{ margin-right: 1rem; }}
</style>
</head>
<body>
<h1>Deployments</h1>
<nav>
  <a href="/dashboard/">Overview</a>
  <a href="/dashboard/deployments">Deployments</a>
</nav>
<hr>
<table>
<tr><th>Name</th><th>Namespace</th><th>Trigger</th><th>Instances</th><th>Source</th></tr>
{rows}
</table>
</body>
</html>"#,
    ))
}

/// Single deployment detail page.
pub async fn deployment_detail(
    State(state): State<DashboardState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Html<String> {
    let instances = state
        .store
        .list_instances_for_deployment(&id)
        .unwrap_or_default();

    let instance_rows: String = instances
        .iter()
        .map(|i| {
            format!(
                "<tr><td>{}</td><td>{:?}</td><td>{:?}</td><td>{}</td><td>{}MB</td></tr>",
                i.id,
                i.status,
                i.health,
                i.restart_count,
                i.memory_bytes / (1024 * 1024),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        r#"<!DOCTYPE html>
<html>
<head><title>{id} — WarpGrid</title>
<style>
  body {{ font-family: system-ui; max-width: 960px; margin: 2rem auto; padding: 0 1rem; }}
  table {{ width: 100%; border-collapse: collapse; }}
  th, td {{ padding: 0.5rem; text-align: left; border-bottom: 1px solid #ddd; }}
  nav a {{ margin-right: 1rem; }}
</style>
</head>
<body>
<h1>Deployment: {id}</h1>
<nav>
  <a href="/dashboard/">Overview</a>
  <a href="/dashboard/deployments">Deployments</a>
</nav>
<hr>
<h2>Instances ({count})</h2>
<table>
<tr><th>ID</th><th>Status</th><th>Health</th><th>Restarts</th><th>Memory</th></tr>
{instance_rows}
</table>
</body>
</html>"#,
        count = instances.len(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpgrid_state::*;
    use std::collections::HashMap;

    fn test_state() -> DashboardState {
        let store = StateStore::open_in_memory().unwrap();
        DashboardState { store }
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
        state.store.put_deployment(&test_deployment("default", "api")).unwrap();

        let html = overview(State(state)).await;
        assert!(html.0.contains("WarpGrid Dashboard"));
        assert!(html.0.contains("api"));
        assert!(html.0.contains("Deployments: 1"));
    }

    #[tokio::test]
    async fn deployments_page_renders() {
        let state = test_state();
        state.store.put_deployment(&test_deployment("default", "api")).unwrap();
        state.store.put_deployment(&test_deployment("prod", "worker")).unwrap();

        let html = deployments(State(state)).await;
        assert!(html.0.contains("api"));
        assert!(html.0.contains("worker"));
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

        let html = deployment_detail(
            State(state),
            axum::extract::Path("default/api".to_string()),
        ).await;
        assert!(html.0.contains("inst-0"));
        assert!(html.0.contains("Running"));
    }

    #[tokio::test]
    async fn overview_empty_state() {
        let state = test_state();
        let html = overview(State(state)).await;
        assert!(html.0.contains("Deployments: 0"));
        assert!(html.0.contains("Nodes: 0"));
    }
}
