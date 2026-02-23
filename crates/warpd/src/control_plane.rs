//! Control plane mode — runs the Raft consensus leader, cluster
//! membership gRPC server, and the REST API.
//!
//! In this mode, the daemon:
//! 1. Opens a state store for the Raft state machine
//! 2. Bootstraps (or rejoins) a Raft cluster
//! 3. Serves both Raft RPCs and cluster membership RPCs over gRPC
//! 4. Serves the REST API over HTTP (separate port)
//! 5. Runs background tasks (metrics, autoscaler, dead node reaper)

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use openraft::BasicNode;
use tokio::sync::watch;
use tracing::info;

use warpgrid_cluster::MembershipManager;
use warpgrid_raft::{LogStore, NetworkFactory, NodeIdMap, RaftGrpcServer, StateMachine};

/// Run the control plane node.
pub async fn run_control_plane(
    api_port: u16,
    grpc_port: u16,
    data_dir: PathBuf,
    raft_node_id: String,
    metrics_interval: u64,
    autoscale_interval: u64,
) -> anyhow::Result<()> {
    info!("WarpGrid daemon starting in control-plane mode");
    std::fs::create_dir_all(&data_dir)?;

    // ── State store (application data) ───────────────────────────
    let app_db_path = data_dir.join("warpgrid.redb");
    let state = warpgrid_state::StateStore::open(&app_db_path)?;
    info!(path = ?app_db_path, "application state store opened");

    // ── Raft storage (separate redb for Raft log + state machine) ─
    let raft_db_path = data_dir.join("raft.redb");
    let raft_db = Arc::new(
        redb::Database::create(&raft_db_path)
            .map_err(|e| anyhow::anyhow!("open raft db: {e}"))?,
    );
    info!(path = ?raft_db_path, "raft storage opened");

    // ── Node ID mapping ──────────────────────────────────────────
    let node_map = Arc::new(NodeIdMap::new(Arc::clone(&raft_db)));
    let my_raft_id = node_map.get_or_insert(&raft_node_id);
    info!(raft_node_id = %raft_node_id, raft_id = my_raft_id, "node ID mapped");

    // ── Raft subsystem ───────────────────────────────────────────
    let log_store = LogStore::new(Arc::clone(&raft_db));
    let state_machine = StateMachine::new(Arc::clone(&raft_db));
    let network_factory = NetworkFactory;

    let raft_config = openraft::Config {
        heartbeat_interval: 500,
        election_timeout_min: 1500,
        election_timeout_max: 3000,
        ..Default::default()
    };
    let raft_config = Arc::new(raft_config);

    let raft = openraft::Raft::new(
        my_raft_id,
        raft_config,
        network_factory,
        log_store,
        state_machine,
    )
    .await?;
    let raft = Arc::new(raft);
    info!("raft instance created");

    // Bootstrap as single-node cluster if fresh.
    let grpc_addr = format!("0.0.0.0:{grpc_port}");
    let mut members = BTreeMap::new();
    members.insert(my_raft_id, BasicNode::new(&grpc_addr));

    if let Err(e) = raft.initialize(members).await {
        // NotAllowed means already initialized — expected on restart.
        info!(error = %e, "raft initialize (may already be bootstrapped)");
    }

    // ── Cluster membership ───────────────────────────────────────
    let membership = Arc::new(MembershipManager::new(state.clone()));
    info!("membership manager initialized");

    // ── gRPC server (Raft + Cluster) ─────────────────────────────
    let raft_grpc = RaftGrpcServer::new(Arc::clone(&raft));
    let cluster_grpc = warpgrid_cluster::ClusterServer::new(Arc::clone(&membership));

    let grpc_addr_parsed: SocketAddr = grpc_addr.parse()?;
    info!(%grpc_addr_parsed, "gRPC server starting (raft + cluster)");

    let grpc_handle = tokio::spawn(async move {
        if let Err(e) = tonic::transport::Server::builder()
            .add_service(raft_grpc.into_service())
            .add_service(cluster_grpc.into_service())
            .serve(grpc_addr_parsed)
            .await
        {
            tracing::error!(error = %e, "gRPC server error");
        }
    });

    // ── Background tasks ─────────────────────────────────────────
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let metrics_shutdown = shutdown_rx.clone();
    let autoscale_shutdown = shutdown_rx.clone();
    let reaper_shutdown = shutdown_rx.clone();

    // Metrics collector.
    let metrics = warpgrid_metrics::MetricsCollector::new(
        state.clone(),
        Duration::from_secs(metrics_interval),
    );
    let metrics_handle = tokio::spawn(async move {
        metrics.run(metrics_shutdown).await;
    });

    // Autoscaler.
    let mut autoscaler = warpgrid_autoscale::Autoscaler::new(state.clone());
    let autoscale_handle = tokio::spawn(async move {
        autoscaler
            .run(Duration::from_secs(autoscale_interval), autoscale_shutdown)
            .await;
    });

    // Dead node reaper (periodic check for unresponsive nodes).
    let reaper_membership = Arc::clone(&membership);
    let reaper_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        let mut shutdown = reaper_shutdown;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match reaper_membership.reap_dead_nodes() {
                        Ok(reaped) if !reaped.is_empty() => {
                            info!(count = reaped.len(), "reaped dead nodes");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "dead node reaper error");
                        }
                        _ => {}
                    }
                }
                _ = shutdown.changed() => break,
            }
        }
    });

    // ── REST API server ──────────────────────────────────────────
    let router = warpgrid_api::build_router(state);
    let api_addr = SocketAddr::from(([0, 0, 0, 0], api_port));

    info!(%api_addr, "API server starting");
    let listener = tokio::net::TcpListener::bind(api_addr).await?;

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        info!("shutdown signal received");
        let _ = shutdown_tx.send(true);
    });

    server.await?;

    // Clean up.
    grpc_handle.abort();
    let _ = metrics_handle.await;
    let _ = autoscale_handle.await;
    let _ = reaper_handle.await;

    info!("control plane stopped");
    Ok(())
}
