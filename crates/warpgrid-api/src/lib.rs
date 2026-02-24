//! warpgrid-api — REST API for WarpGrid.
//!
//! Provides axum route handlers for managing deployments, instances,
//! metrics, nodes, and rollouts. Mounts the dashboard under `/dashboard/`.
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
//! | POST | `/api/v1/deployments/:id/rollout` | Start rollout |
//! | GET | `/api/v1/rollouts` | List active rollouts |
//! | GET | `/api/v1/rollouts/:id` | Get rollout status |
//! | POST | `/api/v1/rollouts/:id/pause` | Pause rollout |
//! | POST | `/api/v1/rollouts/:id/resume` | Resume rollout |
//! | GET | `/api/v1/nodes` | List nodes |
//! | GET | `/metrics` | Prometheus exposition |

pub mod handlers;
pub mod rollout_handlers;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tokio::sync::RwLock;
use warpgrid_state::StateStore;

pub use rollout_handlers::{RolloutApiState, RolloutStore};

/// Shared state for API handlers.
#[derive(Clone)]
pub struct ApiState {
    pub store: StateStore,
}

/// Build the complete API router (REST + dashboard + metrics + rollouts).
pub fn build_router(store: StateStore) -> Router {
    let rollout_store: RolloutStore = Arc::new(RwLock::new(HashMap::new()));
    build_router_with_rollouts(store, rollout_store)
}

/// Build the API router with an externally provided rollout store.
pub fn build_router_with_rollouts(store: StateStore, rollouts: RolloutStore) -> Router {
    let api_state = ApiState {
        store: store.clone(),
    };

    let dashboard_state = warpgrid_dashboard::DashboardState {
        store: store.clone(),
        rollouts: rollouts.clone(),
    };

    let rollout_state = RolloutApiState {
        store: store.clone(),
        rollouts,
    };

    let api_routes = Router::new()
        .route("/deployments", get(handlers::list_deployments).post(handlers::create_deployment))
        .route("/deployments/{id}", get(handlers::get_deployment).delete(handlers::delete_deployment))
        .route("/deployments/{id}/scale", post(handlers::scale_deployment))
        .route("/deployments/{id}/instances", get(handlers::list_instances))
        .route("/deployments/{id}/metrics", get(handlers::get_metrics))
        .route("/nodes", get(handlers::list_nodes))
        .with_state(api_state.clone());

    let rollout_routes = Router::new()
        .route("/deployments/{id}/rollout", post(rollout_handlers::start_rollout))
        .route("/rollouts", get(rollout_handlers::list_rollouts))
        .route("/rollouts/{id}", get(rollout_handlers::get_rollout))
        .route("/rollouts/{id}/pause", post(rollout_handlers::pause_rollout))
        .route("/rollouts/{id}/resume", post(rollout_handlers::resume_rollout))
        .with_state(rollout_state);

    Router::new()
        .nest("/api/v1", api_routes)
        .nest("/api/v1", rollout_routes)
        .nest("/dashboard", warpgrid_dashboard::dashboard_router(dashboard_state))
        .route("/metrics", get(handlers::prometheus_metrics).with_state(api_state))
}
