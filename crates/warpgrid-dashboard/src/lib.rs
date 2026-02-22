//! warpgrid-dashboard — server-rendered web UI for WarpGrid.
//!
//! Provides axum route handlers that render HTML pages for the
//! WarpGrid dashboard. Phase 1 uses plain HTML templates;
//! Leptos SSR + hydration will be added in a future milestone.
//!
//! # Routes
//!
//! | Route | Handler |
//! |---|---|
//! | `/dashboard/` | Cluster overview |
//! | `/dashboard/deployments` | Deployment list |
//! | `/dashboard/deployments/:id` | Deployment detail |

pub mod pages;

use axum::Router;
use axum::routing::get;
use warpgrid_state::StateStore;

/// Shared state for dashboard handlers.
#[derive(Clone)]
pub struct DashboardState {
    pub store: StateStore,
}

/// Build the dashboard router.
pub fn dashboard_router(state: DashboardState) -> Router {
    Router::new()
        .route("/", get(pages::overview))
        .route("/deployments", get(pages::deployments))
        .route("/deployments/{id}", get(pages::deployment_detail))
        .with_state(state)
}
