//! warpd — the WarpGrid daemon.
//!
//! Single binary that assembles all WarpGrid subsystems:
//! - State store (redb)
//! - Wasm runtime
//! - Scheduler + instance pools
//! - Health monitor
//! - Metrics collector
//! - Autoscaler
//! - REST API + dashboard
//!
//! # Usage
//!
//! ```text
//! warpd standalone --port 8443 --data-dir /var/lib/warpgrid
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::sync::watch;
use tracing::info;

#[derive(Parser)]
#[command(name = "warpd", about = "WarpGrid daemon")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run in standalone mode (single-node, all subsystems in one process).
    Standalone {
        /// Port to listen on.
        #[arg(long, default_value = "8443")]
        port: u16,

        /// Data directory for persistent state.
        #[arg(long, default_value = "/var/lib/warpgrid")]
        data_dir: PathBuf,

        /// Metrics snapshot interval in seconds.
        #[arg(long, default_value = "60")]
        metrics_interval: u64,

        /// Autoscaler check interval in seconds.
        #[arg(long, default_value = "30")]
        autoscale_interval: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,warpd=debug,warpgrid=debug".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Standalone {
            port,
            data_dir,
            metrics_interval,
            autoscale_interval,
        } => {
            run_standalone(port, data_dir, metrics_interval, autoscale_interval).await
        }
    }
}

async fn run_standalone(
    port: u16,
    data_dir: PathBuf,
    metrics_interval: u64,
    autoscale_interval: u64,
) -> anyhow::Result<()> {
    info!("WarpGrid daemon starting in standalone mode");

    // Ensure data directory exists.
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("warpgrid.redb");

    // ── Initialize subsystems ──────────────────────────────────

    // State store.
    let state = warpgrid_state::StateStore::open(&db_path)?;
    info!(path = ?db_path, "state store opened");

    // Wasm runtime.
    let runtime = Arc::new(warp_runtime::Runtime::new()?);
    info!("wasm runtime initialized");

    // Scheduler.
    let _scheduler = warpgrid_scheduler::Scheduler::new(
        runtime.clone(),
        state.clone(),
        "standalone".to_string(),
    );
    info!("scheduler initialized");

    // Health monitor.
    let _health_monitor = warpgrid_health::HealthMonitor::new(state.clone());
    info!("health monitor initialized");

    // Metrics collector.
    let metrics = warpgrid_metrics::MetricsCollector::new(
        state.clone(),
        Duration::from_secs(metrics_interval),
    );
    info!(interval = metrics_interval, "metrics collector initialized");

    // Autoscaler.
    let mut autoscaler = warpgrid_autoscale::Autoscaler::new(state.clone());
    info!(interval = autoscale_interval, "autoscaler initialized");

    // ── Shutdown signal ────────────────────────────────────────

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let metrics_shutdown = shutdown_rx.clone();
    let autoscale_shutdown = shutdown_rx.clone();

    // ── Start background tasks ─────────────────────────────────

    // Metrics snapshot loop.
    let metrics_handle = tokio::spawn(async move {
        metrics.run(metrics_shutdown).await;
    });

    // Autoscaler loop.
    let autoscale_handle = tokio::spawn(async move {
        autoscaler
            .run(Duration::from_secs(autoscale_interval), autoscale_shutdown)
            .await;
    });

    // ── Start API server ───────────────────────────────────────

    let router = warpgrid_api::build_router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    info!(%addr, "API server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Graceful shutdown on Ctrl-C.
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C handler");
            info!("shutdown signal received");
            let _ = shutdown_tx.send(true);
        });

    server.await?;

    // Wait for background tasks.
    let _ = metrics_handle.await;
    let _ = autoscale_handle.await;

    info!("WarpGrid daemon stopped");
    Ok(())
}
