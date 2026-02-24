//! Metrics collector — tracks per-deployment request metrics.
//!
//! Uses a lock-free design with atomics for counters and a mutex-protected
//! histogram for latency tracking.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;
use tracing::{debug, info};

use warpgrid_state::{InstanceStatus, MetricsSnapshot, StateStore};

/// Per-deployment metrics bucket.
struct DeploymentMetrics {
    /// Total requests since last snapshot.
    request_count: AtomicU64,
    /// Total errors since last snapshot.
    error_count: AtomicU64,
    /// Latency samples (microseconds) for histogram computation.
    latencies: tokio::sync::Mutex<Vec<u64>>,
    /// Total memory across instances (set externally).
    total_memory_bytes: AtomicU64,
    /// Active instance count (set externally).
    active_instances: AtomicU64,
}

impl DeploymentMetrics {
    fn new() -> Self {
        Self {
            request_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            latencies: tokio::sync::Mutex::new(Vec::new()),
            total_memory_bytes: AtomicU64::new(0),
            active_instances: AtomicU64::new(0),
        }
    }

    /// Reset counters for a new snapshot window.
    async fn reset(&self) {
        self.request_count.store(0, Ordering::Relaxed);
        self.error_count.store(0, Ordering::Relaxed);
        self.latencies.lock().await.clear();
    }
}

/// Collects metrics across all deployments and periodically snapshots
/// them to the state store.
pub struct MetricsCollector {
    /// Per-deployment metrics: deployment_id → metrics.
    metrics: Arc<RwLock<HashMap<String, Arc<DeploymentMetrics>>>>,
    /// The state store for persisting snapshots.
    state: StateStore,
    /// Snapshot interval.
    interval: Duration,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new(state: StateStore, interval: Duration) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            state,
            interval,
        }
    }

    /// Register a deployment for metrics collection.
    pub async fn register(&self, deployment_id: &str) {
        let mut metrics = self.metrics.write().await;
        metrics
            .entry(deployment_id.to_string())
            .or_insert_with(|| Arc::new(DeploymentMetrics::new()));
        debug!(%deployment_id, "registered for metrics collection");
    }

    /// Unregister a deployment.
    pub async fn unregister(&self, deployment_id: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.remove(deployment_id);
        debug!(%deployment_id, "unregistered from metrics collection");
    }

    /// Record a request for a deployment.
    pub async fn record_request(
        &self,
        deployment_id: &str,
        latency_us: u64,
        is_error: bool,
    ) {
        let metrics = self.metrics.read().await;
        if let Some(m) = metrics.get(deployment_id) {
            m.request_count.fetch_add(1, Ordering::Relaxed);
            if is_error {
                m.error_count.fetch_add(1, Ordering::Relaxed);
            }
            m.latencies.lock().await.push(latency_us);
        }
    }

    /// Update memory and instance counts for a deployment.
    pub async fn update_resource_usage(
        &self,
        deployment_id: &str,
        total_memory_bytes: u64,
        active_instances: u32,
    ) {
        let metrics = self.metrics.read().await;
        if let Some(m) = metrics.get(deployment_id) {
            m.total_memory_bytes
                .store(total_memory_bytes, Ordering::Relaxed);
            m.active_instances
                .store(active_instances as u64, Ordering::Relaxed);
        }
    }

    /// Scan the state store for deployments and register any not already tracked.
    pub async fn auto_discover(&self) -> anyhow::Result<()> {
        let deployments = self.state.list_deployments()?;
        let metrics = self.metrics.read().await;
        let known: HashSet<String> = metrics.keys().cloned().collect();
        drop(metrics);

        for d in deployments {
            if !known.contains(&d.id) {
                self.register(&d.id).await;
            }
        }
        Ok(())
    }

    /// Refresh per-deployment memory and instance counts from actual instance data.
    pub async fn refresh_resource_usage(&self) -> anyhow::Result<()> {
        let metrics = self.metrics.read().await;
        for (deployment_id, m) in metrics.iter() {
            let instances = self.state.list_instances_for_deployment(deployment_id)?;
            let running = instances
                .iter()
                .filter(|i| i.status == InstanceStatus::Running)
                .count();
            let total_mem: u64 = instances.iter().map(|i| i.memory_bytes).sum();
            m.active_instances
                .store(running as u64, Ordering::Relaxed);
            m.total_memory_bytes
                .store(total_mem, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Take a snapshot of all deployments and persist to state store.
    pub async fn snapshot(&self) -> anyhow::Result<Vec<MetricsSnapshot>> {
        let metrics = self.metrics.read().await;
        let epoch = epoch_secs();
        let mut snapshots = Vec::new();

        for (deployment_id, m) in metrics.iter() {
            let request_count = m.request_count.load(Ordering::Relaxed);
            let error_count = m.error_count.load(Ordering::Relaxed);
            let total_memory = m.total_memory_bytes.load(Ordering::Relaxed);
            let active = m.active_instances.load(Ordering::Relaxed) as u32;

            let latencies = m.latencies.lock().await;

            // Compute RPS (requests per snapshot interval).
            let rps = request_count as f64 / self.interval.as_secs_f64();

            // Compute error rate.
            let error_rate = if request_count > 0 {
                error_count as f64 / request_count as f64
            } else {
                0.0
            };

            // Compute latency percentiles (microseconds → milliseconds).
            let (p50, p99) = compute_percentiles(&latencies);

            let snapshot = MetricsSnapshot {
                deployment_id: deployment_id.clone(),
                epoch,
                rps,
                latency_p50_ms: p50,
                latency_p99_ms: p99,
                error_rate,
                total_memory_bytes: total_memory,
                active_instances: active,
            };

            self.state.put_metrics(&snapshot)?;
            snapshots.push(snapshot);

            drop(latencies);
            m.reset().await;
        }

        debug!(
            deployments = snapshots.len(),
            epoch, "metrics snapshot persisted"
        );
        Ok(snapshots)
    }

    /// Run the snapshot loop until shutdown signal.
    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        info!(
            interval_secs = self.interval.as_secs(),
            "metrics collector started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {
                    if let Err(e) = self.auto_discover().await {
                        tracing::warn!(error = %e, "metrics auto-discover failed");
                    }
                    if let Err(e) = self.refresh_resource_usage().await {
                        tracing::warn!(error = %e, "metrics resource refresh failed");
                    }
                    if let Err(e) = self.snapshot().await {
                        tracing::error!(error = %e, "metrics snapshot failed");
                    }
                }
                _ = shutdown.changed() => {
                    info!("metrics collector shutting down");
                    // Final snapshot before exit.
                    let _ = self.snapshot().await;
                    break;
                }
            }
        }
    }

    /// Get the current request count for a deployment (without resetting).
    pub async fn current_request_count(&self, deployment_id: &str) -> u64 {
        let metrics = self.metrics.read().await;
        metrics
            .get(deployment_id)
            .map(|m| m.request_count.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// List all registered deployment IDs.
    pub async fn registered_deployments(&self) -> Vec<String> {
        let metrics = self.metrics.read().await;
        metrics.keys().cloned().collect()
    }
}

/// Compute P50 and P99 latency from a sorted list of samples.
///
/// Returns (p50_ms, p99_ms). If empty, returns (0.0, 0.0).
fn compute_percentiles(latencies: &[u64]) -> (f64, f64) {
    if latencies.is_empty() {
        return (0.0, 0.0);
    }

    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();

    let p50_idx = (sorted.len() as f64 * 0.50) as usize;
    let p99_idx = (sorted.len() as f64 * 0.99) as usize;

    let p50 = sorted[p50_idx.min(sorted.len() - 1)] as f64 / 1000.0;
    let p99 = sorted[p99_idx.min(sorted.len() - 1)] as f64 / 1000.0;

    (p50, p99)
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpgrid_state::{
        DeploymentSpec, HealthStatus, InstanceConstraints, InstanceState, ResourceLimits,
        ShimsEnabled, TriggerConfig,
    };

    fn test_state() -> StateStore {
        StateStore::open_in_memory().unwrap()
    }

    fn make_deployment(id: &str) -> DeploymentSpec {
        DeploymentSpec {
            id: id.to_string(),
            namespace: "default".to_string(),
            name: id.to_string(),
            source: "file://test.wasm".to_string(),
            trigger: TriggerConfig::Http { port: None },
            instances: InstanceConstraints { min: 1, max: 3 },
            resources: ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn make_instance(
        id: &str,
        deployment_id: &str,
        status: InstanceStatus,
        memory_bytes: u64,
    ) -> InstanceState {
        InstanceState {
            id: id.to_string(),
            deployment_id: deployment_id.to_string(),
            node_id: "standalone".to_string(),
            status,
            health: HealthStatus::Unknown,
            restart_count: 0,
            memory_bytes,
            started_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn register_and_unregister() {
        let collector = MetricsCollector::new(test_state(), Duration::from_secs(60));

        collector.register("deploy-1").await;
        assert_eq!(collector.registered_deployments().await.len(), 1);

        collector.unregister("deploy-1").await;
        assert!(collector.registered_deployments().await.is_empty());
    }

    #[tokio::test]
    async fn record_and_count_requests() {
        let collector = MetricsCollector::new(test_state(), Duration::from_secs(60));
        collector.register("deploy-1").await;

        collector.record_request("deploy-1", 5000, false).await;
        collector.record_request("deploy-1", 10000, false).await;
        collector.record_request("deploy-1", 3000, true).await;

        assert_eq!(collector.current_request_count("deploy-1").await, 3);
    }

    #[tokio::test]
    async fn unregistered_deployment_ignored() {
        let collector = MetricsCollector::new(test_state(), Duration::from_secs(60));
        // Recording to unknown deployment is a no-op.
        collector.record_request("unknown", 5000, false).await;
        assert_eq!(collector.current_request_count("unknown").await, 0);
    }

    #[tokio::test]
    async fn snapshot_persists_to_state() {
        let state = test_state();
        let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));
        collector.register("deploy-1").await;

        collector.record_request("deploy-1", 5000, false).await;
        collector.record_request("deploy-1", 10000, false).await;
        collector.record_request("deploy-1", 50000, true).await;
        collector
            .update_resource_usage("deploy-1", 128_000_000, 3)
            .await;

        let snapshots = collector.snapshot().await.unwrap();
        assert_eq!(snapshots.len(), 1);

        let snap = &snapshots[0];
        assert_eq!(snap.deployment_id, "deploy-1");
        assert!(snap.rps > 0.0);
        assert!(snap.error_rate > 0.0 && snap.error_rate < 1.0);
        assert_eq!(snap.total_memory_bytes, 128_000_000);
        assert_eq!(snap.active_instances, 3);

        // Verify persisted in state store.
        let stored = state.list_metrics_for_deployment("deploy-1", 10).unwrap();
        assert_eq!(stored.len(), 1);
    }

    #[tokio::test]
    async fn snapshot_resets_counters() {
        let collector = MetricsCollector::new(test_state(), Duration::from_secs(60));
        collector.register("deploy-1").await;

        collector.record_request("deploy-1", 5000, false).await;
        collector.snapshot().await.unwrap();

        // Counter should be reset after snapshot.
        assert_eq!(collector.current_request_count("deploy-1").await, 0);
    }

    #[test]
    fn percentiles_empty() {
        let (p50, p99) = compute_percentiles(&[]);
        assert_eq!(p50, 0.0);
        assert_eq!(p99, 0.0);
    }

    #[test]
    fn percentiles_single_value() {
        let (p50, p99) = compute_percentiles(&[5000]);
        assert_eq!(p50, 5.0);
        assert_eq!(p99, 5.0);
    }

    #[test]
    fn percentiles_distribution() {
        // 100 samples: 1ms to 100ms.
        let latencies: Vec<u64> = (1..=100).map(|i| i * 1000).collect();
        let (p50, p99) = compute_percentiles(&latencies);

        // P50 should be around 50ms.
        assert!(p50 >= 49.0 && p50 <= 51.0, "p50 was {p50}");
        // P99 should be around 99ms.
        assert!(p99 >= 98.0 && p99 <= 100.0, "p99 was {p99}");
    }

    #[tokio::test]
    async fn multiple_deployments() {
        let collector = MetricsCollector::new(test_state(), Duration::from_secs(60));
        collector.register("deploy-1").await;
        collector.register("deploy-2").await;

        collector.record_request("deploy-1", 5000, false).await;
        collector.record_request("deploy-2", 10000, true).await;

        assert_eq!(collector.current_request_count("deploy-1").await, 1);
        assert_eq!(collector.current_request_count("deploy-2").await, 1);

        let snapshots = collector.snapshot().await.unwrap();
        assert_eq!(snapshots.len(), 2);
    }

    #[tokio::test]
    async fn auto_discover_finds_new_deployments() {
        let state = test_state();
        let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));

        // No deployments yet — auto_discover is a no-op.
        collector.auto_discover().await.unwrap();
        assert!(collector.registered_deployments().await.is_empty());

        // Add a deployment to the state store (simulating the API handler).
        state.put_deployment(&make_deployment("deploy-1")).unwrap();

        // auto_discover should pick it up.
        collector.auto_discover().await.unwrap();
        assert_eq!(collector.registered_deployments().await, vec!["deploy-1"]);

        // Calling again should not duplicate.
        collector.auto_discover().await.unwrap();
        assert_eq!(collector.registered_deployments().await.len(), 1);
    }

    #[tokio::test]
    async fn auto_discover_skips_already_registered() {
        let state = test_state();
        let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));

        // Manually register and record a request.
        collector.register("deploy-1").await;
        collector.record_request("deploy-1", 5000, false).await;

        // Add the same deployment to state store.
        state.put_deployment(&make_deployment("deploy-1")).unwrap();

        // auto_discover should not reset the existing registration.
        collector.auto_discover().await.unwrap();
        assert_eq!(collector.current_request_count("deploy-1").await, 1);
    }

    #[tokio::test]
    async fn refresh_resource_usage_updates_from_instances() {
        let state = test_state();
        let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));
        collector.register("deploy-1").await;

        // Add running and stopped instances.
        state
            .put_instance(&make_instance("i-1", "deploy-1", InstanceStatus::Running, 32_000_000))
            .unwrap();
        state
            .put_instance(&make_instance("i-2", "deploy-1", InstanceStatus::Running, 48_000_000))
            .unwrap();
        state
            .put_instance(&make_instance("i-3", "deploy-1", InstanceStatus::Stopped, 16_000_000))
            .unwrap();

        collector.refresh_resource_usage().await.unwrap();

        // Take a snapshot to read the values.
        let snapshots = collector.snapshot().await.unwrap();
        assert_eq!(snapshots.len(), 1);
        let snap = &snapshots[0];
        // Only running instances count.
        assert_eq!(snap.active_instances, 2);
        // Memory sums all instances (running + stopped).
        assert_eq!(snap.total_memory_bytes, 32_000_000 + 48_000_000 + 16_000_000);
    }
}
