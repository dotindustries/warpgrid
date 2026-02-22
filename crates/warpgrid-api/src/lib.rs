//! warpgrid-api — REST API for WarpGrid.
//!
//! Provides axum route handlers for managing deployments, instances,
//! metrics, and nodes. Mounts the dashboard under `/dashboard/`.
//!
//! # API Routes
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | GET | `/api/v1/deployments` | List all deployments |
//! | POST | `/api/v1/deployments` | Create a deployment |
//! | GET | `/api/v1/deployments/:id` | Get deployment details |
//! | DELETE | `/api/v1/deployments/:id` | Delete a deployment |
//! | POST | `/api/v1/deployments/:id/scale` | Scale a deployment |
//! | GET | `/api/v1/deployments/:id/instances` | List instances |
//! | GET | `/api/v1/deployments/:id/metrics` | Get metrics |
//! | GET | `/api/v1/nodes` | List nodes |
//! | GET | `/metrics` | Prometheus exposition |

pub mod handlers;

use axum::Router;
use axum::routing::{get, post};
use warpgrid_state::StateStore;

/// Shared state for API handlers.
#[derive(Clone)]
pub struct ApiState {
    pub store: StateStore,
}

/// Build the complete API router (REST + dashboard + metrics).
pub fn build_router(store: StateStore) -> Router {
    let api_state = ApiState {
        store: store.clone(),
    };

    let dashboard_state = warpgrid_dashboard::DashboardState {
        store: store.clone(),
    };

    let api_routes = Router::new()
        .route("/deployments", get(handlers::list_deployments).post(handlers::create_deployment))
        .route("/deployments/{id}", get(handlers::get_deployment).delete(handlers::delete_deployment))
        .route("/deployments/{id}/scale", post(handlers::scale_deployment))
        .route("/deployments/{id}/instances", get(handlers::list_instances))
        .route("/deployments/{id}/metrics", get(handlers::get_metrics))
        .route("/nodes", get(handlers::list_nodes))
        .with_state(api_state.clone());

    Router::new()
        .nest("/api/v1", api_routes)
        .nest("/dashboard", warpgrid_dashboard::dashboard_router(dashboard_state))
        .route("/metrics", get(handlers::prometheus_metrics).with_state(api_state))
}
