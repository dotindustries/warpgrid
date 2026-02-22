//! Health monitor — background task that runs health checks for deployments.
//!
//! The `HealthMonitor` spawns a background task per deployment that
//! periodically probes the health endpoint and updates instance state
//! in the state store.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use warpgrid_state::*;

use crate::checker::{http_probe, HealthTracker};

/// Callback invoked when a deployment's health status changes.
///
/// The scheduler can use this to trigger instance replacement.
pub type HealthCallback =
    Arc<dyn Fn(String, HealthStatus) -> BoxFuture + Send + Sync>;

type BoxFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = ()> + Send>,
>;

/// Per-deployment monitor state.
struct MonitorSlot {
    /// Handle to the background check task.
    handle: JoinHandle<()>,
    /// Shutdown signal for this monitor.
    shutdown_tx: watch::Sender<bool>,
}

/// Manages health check monitors for all scheduled deployments.
pub struct HealthMonitor {
    state: StateStore,
    /// Active monitors: deployment_id → slot.
    monitors: Arc<RwLock<HashMap<String, MonitorSlot>>>,
    /// Optional callback when health status changes.
    on_status_change: Option<HealthCallback>,
}

impl HealthMonitor {
    /// Create a new health monitor.
    pub fn new(state: StateStore) -> Self {
        Self {
            state,
            monitors: Arc::new(RwLock::new(HashMap::new())),
            on_status_change: None,
        }
    }

    /// Set a callback for health status changes.
    pub fn with_callback(mut self, callback: HealthCallback) -> Self {
        self.on_status_change = Some(callback);
        self
    }

    /// Start monitoring a deployment's health.
    ///
    /// The deployment must have a `health` config in its spec.
    /// The `address` is the instance's listen address (ip:port).
    pub async fn start_monitor(
        &self,
        deployment_id: &str,
        health_config: &HealthConfig,
        address: &str,
    ) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let deployment_id_owned = deployment_id.to_string();
        let config = health_config.clone();
        let address = address.to_string();
        let state = self.state.clone();
        let callback = self.on_status_change.clone();

        let handle = tokio::spawn(async move {
            run_health_loop(
                &deployment_id_owned,
                &config,
                &address,
                state,
                callback,
                shutdown_rx,
            )
            .await;
        });

        let mut monitors = self.monitors.write().await;
        if let Some(old) = monitors.insert(
            deployment_id.to_string(),
            MonitorSlot {
                handle,
                shutdown_tx,
            },
        ) {
            // Stop the old monitor if one was running.
            let _ = old.shutdown_tx.send(true);
            old.handle.abort();
        }

        info!(%deployment_id, endpoint = %health_config.endpoint, "health monitor started");
    }

    /// Stop monitoring a deployment.
    pub async fn stop_monitor(&self, deployment_id: &str) {
        let mut monitors = self.monitors.write().await;
        if let Some(slot) = monitors.remove(deployment_id) {
            let _ = slot.shutdown_tx.send(true);
            slot.handle.abort();
            info!(%deployment_id, "health monitor stopped");
        }
    }

    /// Stop all monitors (for graceful shutdown).
    pub async fn stop_all(&self) {
        let mut monitors = self.monitors.write().await;
        for (id, slot) in monitors.drain() {
            let _ = slot.shutdown_tx.send(true);
            slot.handle.abort();
            debug!(deployment_id = %id, "health monitor stopped");
        }
        info!("all health monitors stopped");
    }

    /// List deployment IDs with active monitors.
    pub async fn active_monitors(&self) -> Vec<String> {
        let monitors = self.monitors.read().await;
        monitors.keys().cloned().collect()
    }

    /// Check if a deployment has an active monitor.
    pub async fn is_monitoring(&self, deployment_id: &str) -> bool {
        let monitors = self.monitors.read().await;
        monitors.contains_key(deployment_id)
    }
}

/// The health check loop for a single deployment.
async fn run_health_loop(
    deployment_id: &str,
    config: &HealthConfig,
    address: &str,
    state: StateStore,
    callback: Option<HealthCallback>,
    mut shutdown: watch::Receiver<bool>,
) {
    let timeout = parse_timeout(&config.timeout);
    let mut tracker = HealthTracker::new(config);

    debug!(%deployment_id, endpoint = %config.endpoint, "health loop starting");

    loop {
        let interval = tracker.next_interval();

        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                let result = http_probe(address, &config.endpoint, timeout).await;
                let prev_status = tracker.status();
                let new_status = tracker.record(result);

                // Update instance states in the store if status changed.
                if new_status != prev_status {
                    if let Err(e) = update_deployment_health(&state, deployment_id, new_status) {
                        error!(%deployment_id, error = %e, "failed to update health status in store");
                    }

                    if let Some(ref cb) = callback {
                        cb(deployment_id.to_string(), new_status).await;
                    }
                }
            }
            _ = shutdown.changed() => {
                debug!(%deployment_id, "health loop shutting down");
                break;
            }
        }
    }
}

/// Update all instance health statuses for a deployment.
fn update_deployment_health(
    state: &StateStore,
    deployment_id: &str,
    status: HealthStatus,
) -> Result<(), warpgrid_state::StateError> {
    let instances = state.list_instances_for_deployment(deployment_id)?;
    for mut inst in instances {
        inst.health = status;
        inst.updated_at = epoch_secs();
        if status == HealthStatus::Unhealthy {
            inst.status = InstanceStatus::Unhealthy;
        } else if inst.status == InstanceStatus::Unhealthy && status == HealthStatus::Healthy {
            inst.status = InstanceStatus::Running;
        }
        state.put_instance(&inst)?;
    }
    Ok(())
}

fn parse_timeout(s: &str) -> Duration {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        if let Some(ms) = secs.strip_suffix('m') {
            ms.parse::<u64>()
                .map(Duration::from_millis)
                .unwrap_or(Duration::from_secs(2))
        } else {
            secs.parse::<u64>()
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(2))
        }
    } else {
        Duration::from_secs(2)
    }
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::ProbeResult;

    fn test_health_config() -> HealthConfig {
        HealthConfig {
            endpoint: "/healthz".to_string(),
            interval: "1s".to_string(),
            timeout: "1s".to_string(),
            unhealthy_threshold: 2,
        }
    }

    fn test_instance(deployment_id: &str, index: u32) -> InstanceState {
        InstanceState {
            id: format!("inst-{index}"),
            deployment_id: deployment_id.to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 64 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        }
    }

    #[tokio::test]
    async fn monitor_starts_and_stops() {
        let state = StateStore::open_in_memory().unwrap();
        let monitor = HealthMonitor::new(state);

        assert!(monitor.active_monitors().await.is_empty());

        // Start — will fail to connect but that's fine for lifecycle test.
        monitor
            .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:0")
            .await;
        assert!(monitor.is_monitoring("deploy-1").await);

        monitor.stop_monitor("deploy-1").await;
        assert!(!monitor.is_monitoring("deploy-1").await);
    }

    #[tokio::test]
    async fn monitor_stop_all() {
        let state = StateStore::open_in_memory().unwrap();
        let monitor = HealthMonitor::new(state);

        monitor
            .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:0")
            .await;
        monitor
            .start_monitor("deploy-2", &test_health_config(), "127.0.0.1:0")
            .await;

        assert_eq!(monitor.active_monitors().await.len(), 2);

        monitor.stop_all().await;
        assert!(monitor.active_monitors().await.is_empty());
    }

    #[test]
    fn update_deployment_health_marks_instances() {
        let state = StateStore::open_in_memory().unwrap();
        state.put_instance(&test_instance("deploy-1", 0)).unwrap();
        state.put_instance(&test_instance("deploy-1", 1)).unwrap();

        update_deployment_health(&state, "deploy-1", HealthStatus::Unhealthy).unwrap();

        let instances = state.list_instances_for_deployment("deploy-1").unwrap();
        for inst in &instances {
            assert_eq!(inst.health, HealthStatus::Unhealthy);
            assert_eq!(inst.status, InstanceStatus::Unhealthy);
        }

        // Recovery.
        update_deployment_health(&state, "deploy-1", HealthStatus::Healthy).unwrap();
        let instances = state.list_instances_for_deployment("deploy-1").unwrap();
        for inst in &instances {
            assert_eq!(inst.health, HealthStatus::Healthy);
            assert_eq!(inst.status, InstanceStatus::Running);
        }
    }

    #[test]
    fn parse_timeout_values() {
        assert_eq!(parse_timeout("2s"), Duration::from_secs(2));
        assert_eq!(parse_timeout("500ms"), Duration::from_millis(500));
        assert_eq!(parse_timeout("invalid"), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn http_probe_to_closed_port_returns_failed() {
        // Port 0 won't be listening.
        let result = http_probe("127.0.0.1:1", "/healthz", Duration::from_millis(100)).await;
        assert_eq!(result, ProbeResult::Failed);
    }

    #[tokio::test]
    async fn monitor_replaces_existing_monitor() {
        let state = StateStore::open_in_memory().unwrap();
        let monitor = HealthMonitor::new(state);

        monitor
            .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:0")
            .await;
        // Starting again replaces the old one.
        monitor
            .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:1")
            .await;

        assert_eq!(monitor.active_monitors().await.len(), 1);
        monitor.stop_all().await;
    }
}
