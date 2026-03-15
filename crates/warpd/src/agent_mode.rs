//! Agent mode — runs on worker nodes, joins an existing cluster.
//!
//! In this mode, the daemon:
//! 1. Opens a local redb state store for instance tracking (hot path)
//! 2. Connects to Turso as a read replica (for deployment discovery)
//! 3. Initializes the Wasm runtime and local scheduler
//! 4. Connects to the control plane and joins the cluster
//! 5. Starts the deployment watcher (polls Turso replica for new deployments)
//! 6. Starts the runtime sync (batch-pushes local state to Turso every 30s)
//! 7. Runs a heartbeat loop, processing commands from the control plane
//! 8. On shutdown, gracefully leaves the cluster
//!
//! ```text
//! Turso replica ──watcher──→ detect new deployment
//!                                   ↓
//!                            load Wasm BLOB from replica
//!                                   ↓
//!                            log deployment (runtime loading is TODO)
//!
//! redb (hot) ──sync (30s)──→ Turso (global dashboard)
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use warpgrid_cluster::agent::{AgentConfig, NodeAgent};

use crate::cloud::sync::RuntimeSync;
use crate::cloud::watcher::DeploymentWatcher;

/// Run the agent node.
pub async fn run_agent(
    control_plane_addr: String,
    address: String,
    port: u16,
    data_dir: PathBuf,
    capacity_memory_bytes: u64,
    capacity_cpu_weight: u32,
    metrics_interval: u64,
    region: String,
    turso_url: Option<String>,
    turso_auth_token: Option<String>,
    sync_interval: u64,
) -> anyhow::Result<()> {
    info!(region = %region, "WarpGrid daemon starting in agent mode");
    std::fs::create_dir_all(&data_dir)?;

    // ── Local state store (redb — hot path) ───────────────────────
    let db_path = data_dir.join("warpgrid-agent.redb");
    let state = warpgrid_state::StateStore::open(&db_path)?;
    info!(path = ?db_path, "local state store opened (redb)");

    // ── Turso replica (for deployment discovery + global sync) ────
    let cloud_db_path = data_dir
        .join("cloud-replica.db")
        .to_string_lossy()
        .to_string();
    let cloud_db = match (&turso_url, &turso_auth_token) {
        (Some(url), Some(token)) => {
            info!(url = %url, "connecting to Turso Cloud (embedded replica)");
            crate::cloud::db::open_replica(&cloud_db_path, url, token).await?
        }
        (Some(_), None) => {
            anyhow::bail!("TURSO_DATABASE_URL is set but TURSO_AUTH_TOKEN is missing");
        }
        _ => {
            info!("no Turso credentials — using local-only cloud database");
            crate::cloud::db::open_local(&cloud_db_path).await?
        }
    };
    let cloud_conn = cloud_db.connect()?;
    crate::cloud::db::migrate(&cloud_conn).await?;
    let mode_label = if turso_url.is_some() {
        "turso replica"
    } else {
        "local"
    };
    info!(path = %cloud_db_path, mode = mode_label, "cloud metadata store opened");

    // ── Wasm runtime ──────────────────────────────────────────────
    let runtime = Arc::new(warp_runtime::Runtime::new(
        warp_runtime::ShimConfig::default(),
    )?);
    info!("wasm runtime initialized");

    // ── Local scheduler ───────────────────────────────────────────
    let _scheduler =
        warpgrid_scheduler::Scheduler::new(runtime.clone(), state.clone(), "agent".to_string());
    info!("local scheduler initialized");

    // ── Health monitor ────────────────────────────────────────────
    let _health_monitor = warpgrid_health::HealthMonitor::new(state.clone());
    info!("health monitor initialized");

    // ── Metrics collector ─────────────────────────────────────────
    let metrics = warpgrid_metrics::MetricsCollector::new(
        state.clone(),
        Duration::from_secs(metrics_interval),
    );

    // ── Shutdown signal ───────────────────────────────────────────
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let metrics_shutdown = shutdown_rx.clone();
    let heartbeat_shutdown = shutdown_rx.clone();
    let watcher_shutdown = shutdown_rx.clone();
    let sync_shutdown = shutdown_rx.clone();

    // ── Start metrics collector ───────────────────────────────────
    let metrics_handle = tokio::spawn(async move {
        metrics.run(metrics_shutdown).await;
    });

    // ── Start deployment watcher ──────────────────────────────────
    // Polls Turso replica for new deployments assigned to this region.
    // When a new deployment is found, the Wasm blob is read from the
    // local replica and logged. Actual runtime loading is the next step.
    let watcher_conn = cloud_db.connect()?;
    let watcher_state = state.clone();
    let watcher_region = region.clone();
    let watcher_runtime = runtime.clone();
    let watcher_handle = tokio::spawn(async move {
        let mut watcher =
            DeploymentWatcher::new(watcher_conn, watcher_state.clone(), watcher_region.clone());
        info!(region = %watcher_region, "deployment watcher started");

        let poll_interval = Duration::from_secs(5);
        let mut shutdown = watcher_shutdown;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {
                    match watcher.poll_once().await {
                        Ok(new_deployments) => {
                            for dep in &new_deployments {
                                info!(
                                    deployment_id = %dep.deployment_id,
                                    wasm_size = dep.wasm_bytes.len(),
                                    region = %dep.region,
                                    "new deployment detected — loading into runtime"
                                );

                                // Compile and cache the Wasm module.
                                match watcher_runtime.load_module(&dep.deployment_id, &dep.wasm_bytes).await {
                                    Ok(module) => {
                                        info!(
                                            deployment_id = %dep.deployment_id,
                                            module = module.name(),
                                            "Wasm module compiled and cached"
                                        );

                                        // Create an instance record in local redb.
                                        let instance = warpgrid_state::InstanceState {
                                            id: format!("{}-0", dep.deployment_id),
                                            deployment_id: dep.deployment_id.clone(),
                                            node_id: watcher_region.clone(),
                                            status: warpgrid_state::InstanceStatus::Running,
                                            health: warpgrid_state::HealthStatus::Healthy,
                                            restart_count: 0,
                                            memory_bytes: dep.wasm_bytes.len() as u64,
                                            started_at: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs(),
                                            updated_at: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs(),
                                        };
                                        if let Err(e) = watcher_state.put_instance(&instance) {
                                            warn!(error = %e, "failed to record instance state");
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            deployment_id = %dep.deployment_id,
                                            error = %e,
                                            "failed to compile Wasm module"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "deployment watcher poll failed");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("deployment watcher shutting down");
                    break;
                }
            }
        }
    });

    // ── Start runtime sync (local redb → Turso, every 30s) ───────
    let sync_conn = cloud_db.connect()?;
    let sync_state = state.clone();
    let sync_region = region.clone();
    let sync_handle = tokio::spawn(async move {
        let sync = RuntimeSync::new(&sync_region, sync_state, sync_conn);
        sync.run(Duration::from_secs(sync_interval), sync_shutdown)
            .await;
    });
    info!(
        interval = sync_interval,
        "runtime sync started (redb → Turso)"
    );

    // ── Join cluster ──────────────────────────────────────────────
    let agent_config = AgentConfig {
        control_plane_addr,
        address: address.clone(),
        port,
        labels: HashMap::from([("region".to_string(), region.clone())]),
        capacity_memory_bytes,
        capacity_cpu_weight,
    };

    let mut agent = NodeAgent::new(agent_config);
    let node_id = agent.join().await?;
    info!(%node_id, region = %region, "joined cluster");

    // ── Heartbeat loop ────────────────────────────────────────────
    let heartbeat_handle = tokio::spawn(async move {
        if let Err(e) = agent.run_heartbeat(0, 0, heartbeat_shutdown).await {
            tracing::error!(error = %e, "heartbeat loop error");
        }
    });

    // ── Wait for shutdown ─────────────────────────────────────────
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    info!("shutdown signal received");
    let _ = shutdown_tx.send(true);

    // Wait for all background tasks.
    let _ = heartbeat_handle.await;
    let _ = metrics_handle.await;
    let _ = watcher_handle.await;
    let _ = sync_handle.await;

    info!("agent stopped");
    Ok(())
}
