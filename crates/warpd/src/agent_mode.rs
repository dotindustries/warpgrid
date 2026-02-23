//! Agent mode — runs on worker nodes, joins an existing cluster.
//!
//! In this mode, the daemon:
//! 1. Opens a local state store for instance tracking
//! 2. Initializes the Wasm runtime and local scheduler
//! 3. Connects to the control plane and joins the cluster
//! 4. Runs a heartbeat loop, processing commands from the control plane
//! 5. On shutdown, gracefully leaves the cluster

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::info;

use warpgrid_cluster::agent::{AgentConfig, NodeAgent};

/// Run the agent node.
pub async fn run_agent(
    control_plane_addr: String,
    address: String,
    port: u16,
    data_dir: PathBuf,
    capacity_memory_bytes: u64,
    capacity_cpu_weight: u32,
    metrics_interval: u64,
) -> anyhow::Result<()> {
    info!("WarpGrid daemon starting in agent mode");
    std::fs::create_dir_all(&data_dir)?;

    // ── Local state store ────────────────────────────────────────
    let db_path = data_dir.join("warpgrid-agent.redb");
    let state = warpgrid_state::StateStore::open(&db_path)?;
    info!(path = ?db_path, "local state store opened");

    // ── Wasm runtime ─────────────────────────────────────────────
    let runtime = Arc::new(warp_runtime::Runtime::new()?);
    info!("wasm runtime initialized");

    // ── Local scheduler (Standalone mode for executing local work) ─
    let _scheduler = warpgrid_scheduler::Scheduler::new(
        runtime.clone(),
        state.clone(),
        "agent".to_string(),
    );
    info!("local scheduler initialized");

    // ── Health monitor ───────────────────────────────────────────
    let _health_monitor = warpgrid_health::HealthMonitor::new(state.clone());
    info!("health monitor initialized");

    // ── Metrics collector ────────────────────────────────────────
    let metrics = warpgrid_metrics::MetricsCollector::new(
        state.clone(),
        Duration::from_secs(metrics_interval),
    );

    // ── Shutdown signal ──────────────────────────────────────────
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let metrics_shutdown = shutdown_rx.clone();
    let heartbeat_shutdown = shutdown_rx.clone();

    // Start metrics collector.
    let metrics_handle = tokio::spawn(async move {
        metrics.run(metrics_shutdown).await;
    });

    // ── Join cluster ─────────────────────────────────────────────
    let agent_config = AgentConfig {
        control_plane_addr,
        address: address.clone(),
        port,
        labels: HashMap::new(),
        capacity_memory_bytes,
        capacity_cpu_weight,
    };

    let mut agent = NodeAgent::new(agent_config);
    let node_id = agent.join().await?;
    info!(%node_id, "joined cluster");

    // ── Heartbeat loop ───────────────────────────────────────────
    let heartbeat_handle = tokio::spawn(async move {
        if let Err(e) = agent
            .run_heartbeat(0, 0, heartbeat_shutdown)
            .await
        {
            tracing::error!(error = %e, "heartbeat loop error");
        }
    });

    // ── Wait for shutdown ────────────────────────────────────────
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    info!("shutdown signal received");
    let _ = shutdown_tx.send(true);

    // Wait for background tasks.
    let _ = heartbeat_handle.await;
    let _ = metrics_handle.await;

    info!("agent stopped");
    Ok(())
}
