//! Cloud mode — hosted WarpGrid platform control plane.
//!
//! Runs as a multi-tenant control plane that:
//! - Authenticates users via API keys
//! - Manages deployment lifecycle (create, scale, delete)
//! - Stores Wasm components in a registry
//! - Provisions edge `warpd agent` nodes via Fly Machines API
//! - Serves the web console and cloud API
//!
//! ```text
//! warpd cloud --api-port 8443 --data-dir /var/lib/warpgrid
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::cloud::analytics::AnalyticsService;
use crate::cloud::auth::AuthStore;
use crate::cloud::console::console_router;
use crate::cloud::domains::DomainStore;
use crate::cloud::provisioner::FlyProvisioner;
use crate::cloud::registry::WasmRegistry;
use crate::cloud::routes::{cloud_router, CloudState};
use crate::cloud::teams::TeamStore;

pub async fn run_cloud(
    api_port: u16,
    data_dir: PathBuf,
    _postgres_url: Option<String>,
    fly_api_token: Option<String>,
    _registry_bucket: String,
    edge_regions: String,
    metrics_interval: u64,
    posthog_api_key: Option<String>,
) -> anyhow::Result<()> {
    info!("WarpGrid daemon starting in cloud mode");

    // ── Initialize data directory ───────────────────────────────
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("warpgrid-cloud.redb");

    // ── Initialize subsystems ───────────────────────────────────

    // State store (redb for beta, Postgres for production).
    let state = warpgrid_state::StateStore::open(&db_path)?;
    info!(path = ?db_path, "cloud state store opened");

    // Auth store (in-memory for beta, Postgres for production).
    let auth = AuthStore::new();
    info!("auth store initialized");

    // Wasm component registry (local filesystem for beta, Tigris S3 for production).
    let registry_dir = data_dir.join("registry");
    let registry = WasmRegistry::local(&registry_dir);
    info!(path = ?registry_dir, "wasm registry initialized");

    // Parse edge regions.
    let regions: Vec<String> = edge_regions
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    info!(?regions, "edge regions configured");

    // Fly Machines provisioner (optional — only if FLY_API_TOKEN is set).
    if let Some(ref token) = fly_api_token {
        let provisioner = FlyProvisioner::new(token, "warpgrid-edge", "registry.fly.io/warpgrid:latest");
        let control_plane_url = format!("http://localhost:{api_port}");
        info!("Fly provisioner initialized, checking edge regions...");

        match provisioner.provision_regions(&regions, &control_plane_url).await {
            Ok(machines) => {
                for m in &machines {
                    info!(id = %m.id, region = %m.region, state = %m.state, "edge machine ready");
                }
                if machines.is_empty() {
                    info!("all edge regions already provisioned");
                }
            }
            Err(e) => {
                warn!(error = %e, "edge provisioning failed (will retry on next deploy)");
            }
        }
    } else {
        info!("no FLY_API_TOKEN set — running in local-only mode (no edge provisioning)");
    }

    // Metrics collector.
    let metrics = warpgrid_metrics::MetricsCollector::new(
        state.clone(),
        std::time::Duration::from_secs(metrics_interval),
    );
    info!(interval = metrics_interval, "metrics collector initialized");

    // ── Shutdown signal ─────────────────────────────────────────

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let metrics_shutdown = shutdown_rx.clone();

    // ── Background tasks ────────────────────────────────────────

    let metrics_handle = tokio::spawn(async move {
        metrics.run(metrics_shutdown).await;
    });

    // ── Analytics ────────────────────────────────────────────────

    let analytics = AnalyticsService::from_env(posthog_api_key.as_deref());
    match &analytics {
        AnalyticsService::Active(_) => info!("PostHog analytics enabled"),
        AnalyticsService::Noop => info!("PostHog analytics disabled (no POSTHOG_API_KEY)"),
    }

    // ── Build API router ────────────────────────────────────────

    // Merge existing warpgrid-api routes (dashboard, deployments, metrics)
    // with cloud-specific routes (auth, deploy, domains).
    let base_router = warpgrid_api::build_router(state.clone());
    let cloud_state = CloudState {
        auth,
        registry,
        state_store: state,
        teams: TeamStore::new(),
        analytics,
        domains: DomainStore::new(),
    };
    let console_routes = console_router(cloud_state.clone());
    let cloud_routes = cloud_router(cloud_state);

    let router = base_router.merge(cloud_routes).merge(console_routes);

    // ── Start server ────────────────────────────────────────────

    let addr = SocketAddr::from(([0, 0, 0, 0], api_port));
    info!(%addr, mode = "cloud", "API server starting");
    info!(
        "Dashboard: http://localhost:{}/dashboard",
        api_port
    );
    info!(
        "Cloud API: http://localhost:{}/api/v1/status",
        api_port
    );
    info!(
        "Console:   http://localhost:{}/console/",
        api_port
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        info!("shutdown signal received");
        let _ = shutdown_tx.send(true);
    });

    server.await?;

    // Wait for background tasks.
    let _ = metrics_handle.await;

    info!("WarpGrid cloud daemon stopped");
    Ok(())
}
