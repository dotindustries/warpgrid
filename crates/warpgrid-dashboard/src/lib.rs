//! warpgrid-dashboard — server-rendered web UI for WarpGrid.
//!
//! Provides axum route handlers that render Askama HTML templates for the
//! WarpGrid dashboard. Uses Tailwind CSS (CDN) for styling and HTMX (CDN)
//! for live updates via polling.
//!
//! # Routes
//!
//! | Route | Handler |
//! |---|---|
//! | `/dashboard/` | Cluster overview |
//! | `/dashboard/deployments` | Deployment list |
//! | `/dashboard/deployments/:id` | Deployment detail |
//! | `/dashboard/nodes` | Node topology |
//! | `/dashboard/nodes/:id` | Node detail |
//! | `/dashboard/rollouts` | Rollout tracker |
//! | `/dashboard/_overview_stats` | HTMX partial: overview stats |
//! | `/dashboard/_deployments_table` | HTMX partial: deployment rows |
//! | `/dashboard/_deployment_instances/:id` | HTMX partial: instance table |
//! | `/dashboard/_rollout_cards` | HTMX partial: rollout cards |
//! | `/dashboard/_node_cards` | HTMX partial: node cards |

pub mod actions;
pub mod pages;
pub mod partials;
pub mod views;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};
use tokio::sync::RwLock;
use warpgrid_rollout::Rollout;
use warpgrid_state::StateStore;

/// Shared rollout store type (same as in warpgrid-api).
pub type RolloutStore = Arc<RwLock<HashMap<String, Rollout>>>;

/// Shared state for dashboard handlers.
#[derive(Clone)]
pub struct DashboardState {
    pub store: StateStore,
    pub rollouts: RolloutStore,
}

/// Build the dashboard router.
pub fn dashboard_router(state: DashboardState) -> Router {
    Router::new()
        // Page routes
        .route("/", get(pages::overview))
        .route("/deployments", get(pages::deployments))
        .route("/deployments/{id}", get(pages::deployment_detail))
        .route("/nodes", get(pages::nodes))
        .route("/nodes/{id}", get(pages::node_detail))
        .route("/rollouts", get(pages::rollouts))
        .route("/density-demo", get(pages::density_demo))
        // HTMX partial routes
        .route("/_overview_stats", get(partials::overview_stats))
        .route("/_deployments_table", get(partials::deployments_table))
        .route(
            "/_deployment_instances/{id}",
            get(partials::deployment_instances),
        )
        .route("/_rollout_cards", get(partials::rollout_cards))
        .route("/_node_cards", get(partials::node_cards))
        // Action routes
        .route(
            "/deployments/{id}/scale",
            post(actions::scale_deployment),
        )
        .route(
            "/deployments/{id}/rollout",
            post(actions::start_rollout),
        )
        .route(
            "/deployments/{id}",
            delete(actions::delete_deployment),
        )
        .route("/rollouts/{id}/pause", post(actions::pause_rollout))
        .route("/rollouts/{id}/resume", post(actions::resume_rollout))
        .with_state(state)
}
