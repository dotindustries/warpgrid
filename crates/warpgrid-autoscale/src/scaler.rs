//! Autoscaler — metrics-driven instance scaling.
//!
//! Reads the latest `MetricsSnapshot` for each deployment from the state
//! store, compares against the `ScalingConfig.target_value`, and emits
//! scaling decisions. The actual scaling is performed by a callback
//! to the scheduler.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::{debug, info, warn};

use warpgrid_state::*;

/// A scaling decision for a single deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScaleDecision {
    /// Scale to the specified instance count.
    ScaleTo(u32),
    /// No change needed.
    NoChange,
}

/// Callback type for performing scaling actions.
///
/// The autoscaler calls this with (deployment_id, target_instances).
pub type ScaleCallback = Box<dyn Fn(&str, u32) -> BoxFuture + Send + Sync>;

type BoxFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>,
>;

/// Per-deployment scaling state.
struct ScaleState {
    /// Last time we scaled up.
    last_scale_up: u64,
    /// Last time we scaled down.
    last_scale_down: u64,
}

impl ScaleState {
    fn new() -> Self {
        Self {
            last_scale_up: 0,
            last_scale_down: 0,
        }
    }
}

/// The autoscaler evaluates metrics and decides whether to scale
/// deployments up or down.
pub struct Autoscaler {
    state: StateStore,
    /// Per-deployment scaling state (cooldown tracking).
    scale_states: HashMap<String, ScaleState>,
    /// Callback to perform scaling.
    scale_fn: Option<ScaleCallback>,
}

impl Autoscaler {
    /// Create a new autoscaler.
    pub fn new(state: StateStore) -> Self {
        Self {
            state,
            scale_states: HashMap::new(),
            scale_fn: None,
        }
    }

    /// Set the callback used to perform scaling.
    pub fn with_scale_fn(mut self, f: ScaleCallback) -> Self {
        self.scale_fn = Some(f);
        self
    }

    /// Evaluate a single deployment and return a scaling decision.
    ///
    /// Compares the latest metrics against the deployment's scaling config.
    pub fn evaluate(
        &mut self,
        spec: &DeploymentSpec,
        snapshot: &MetricsSnapshot,
    ) -> ScaleDecision {
        let scaling = match &spec.scaling {
            Some(s) => s,
            None => return ScaleDecision::NoChange,
        };

        let now = epoch_secs();
        let scale_state = self
            .scale_states
            .entry(spec.id.clone())
            .or_insert_with(ScaleState::new);

        // Parse cooldown windows.
        let scale_up_cooldown = parse_duration_secs(&scaling.scale_up_window);
        let scale_down_cooldown = parse_duration_secs(&scaling.scale_down_window);

        // Get the current metric value.
        let current_value = match scaling.metric.as_str() {
            "rps" => snapshot.rps,
            "latency_p99" => snapshot.latency_p99_ms,
            "error_rate" => snapshot.error_rate,
            "memory" => snapshot.total_memory_bytes as f64,
            _ => {
                warn!(
                    metric = %scaling.metric,
                    deployment = %spec.id,
                    "unknown scaling metric"
                );
                return ScaleDecision::NoChange;
            }
        };

        let target = scaling.target_value;
        let current_instances = snapshot.active_instances;

        // Scale-to-zero check: if RPS is 0 and we have instances.
        if scaling.metric == "rps"
            && snapshot.rps == 0.0
            && current_instances > 0
            && now - scale_state.last_scale_down >= scale_down_cooldown
            && spec.instances.min == 0
        {
            scale_state.last_scale_down = now;
            debug!(deployment = %spec.id, "scale-to-zero: no traffic");
            return ScaleDecision::ScaleTo(0);
        }

        // Scale up: current value exceeds target (10% headroom).
        if current_value > target * 1.1
            && now - scale_state.last_scale_up >= scale_up_cooldown
        {
            let ratio = current_value / target;
            let desired = ((current_instances as f64) * ratio).ceil() as u32;
            let clamped = desired.min(spec.instances.max);

            if clamped > current_instances {
                scale_state.last_scale_up = now;
                debug!(
                    deployment = %spec.id,
                    from = current_instances,
                    to = clamped,
                    metric = %scaling.metric,
                    current = current_value,
                    target,
                    "scaling up"
                );
                return ScaleDecision::ScaleTo(clamped);
            }
        }

        // Scale down: current value is well below target.
        if current_value < target * 0.5
            && current_instances > spec.instances.min
            && now - scale_state.last_scale_down >= scale_down_cooldown
        {
            let ratio = current_value / target;
            let desired = ((current_instances as f64) * ratio).ceil().max(1.0) as u32;
            let clamped = desired.max(spec.instances.min);

            if clamped < current_instances {
                scale_state.last_scale_down = now;
                debug!(
                    deployment = %spec.id,
                    from = current_instances,
                    to = clamped,
                    metric = %scaling.metric,
                    current = current_value,
                    target,
                    "scaling down"
                );
                return ScaleDecision::ScaleTo(clamped);
            }
        }

        ScaleDecision::NoChange
    }

    /// Evaluate all deployments with scaling configs.
    ///
    /// Reads the latest metrics from the state store and calls `evaluate()`
    /// for each deployment.
    pub async fn evaluate_all(&mut self) -> anyhow::Result<Vec<(String, ScaleDecision)>> {
        let deployments = self.state.list_deployments()?;
        let mut decisions = Vec::new();

        for spec in &deployments {
            if spec.scaling.is_none() {
                continue;
            }

            let snapshots = self
                .state
                .list_metrics_for_deployment(&spec.id, 1)?;

            let snapshot = match snapshots.first() {
                Some(s) => s,
                None => continue, // No metrics yet.
            };

            let decision = self.evaluate(spec, snapshot);

            if let ScaleDecision::ScaleTo(target) = &decision
                && let Some(ref scale_fn) = self.scale_fn
                && let Err(e) = scale_fn(&spec.id, *target).await
            {
                warn!(
                    deployment = %spec.id,
                    target,
                    error = %e,
                    "scaling action failed"
                );
            }

            decisions.push((spec.id.clone(), decision));
        }

        Ok(decisions)
    }

    /// Run the autoscaler loop.
    pub async fn run(
        &mut self,
        interval: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        info!(
            interval_secs = interval.as_secs(),
            "autoscaler started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    if let Err(e) = self.evaluate_all().await {
                        tracing::error!(error = %e, "autoscaler evaluation failed");
                    }
                }
                _ = shutdown.changed() => {
                    info!("autoscaler shutting down");
                    break;
                }
            }
        }
    }
}

/// Parse a duration string like "30s", "5m" into seconds.
fn parse_duration_secs(s: &str) -> u64 {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        secs.parse::<u64>().unwrap_or(30)
    } else if let Some(mins) = s.strip_suffix('m') {
        mins.parse::<u64>().unwrap_or(5) * 60
    } else {
        s.parse::<u64>().unwrap_or(30)
    }
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
    use std::collections::HashMap;

    fn test_spec_with_scaling(metric: &str, target: f64) -> DeploymentSpec {
        DeploymentSpec {
            id: "default/api".to_string(),
            namespace: "default".to_string(),
            name: "api".to_string(),
            source: "file://test.wasm".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 1, max: 10 },
            resources: ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: Some(ScalingConfig {
                metric: metric.to_string(),
                target_value: target,
                scale_up_window: "0s".to_string(),   // No cooldown for tests.
                scale_down_window: "0s".to_string(),
            }),
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    fn test_snapshot(rps: f64, active: u32) -> MetricsSnapshot {
        MetricsSnapshot {
            deployment_id: "default/api".to_string(),
            epoch: 1000,
            rps,
            latency_p50_ms: 5.0,
            latency_p99_ms: 50.0,
            error_rate: 0.01,
            total_memory_bytes: 64 * 1024 * 1024,
            active_instances: active,
        }
    }

    #[test]
    fn no_scaling_config_returns_no_change() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let mut spec = test_spec_with_scaling("rps", 100.0);
        spec.scaling = None;
        let snap = test_snapshot(200.0, 2);

        assert_eq!(scaler.evaluate(&spec, &snap), ScaleDecision::NoChange);
    }

    #[test]
    fn scale_up_when_above_target() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let spec = test_spec_with_scaling("rps", 100.0);
        // RPS is 200, target is 100 → need ~2x instances.
        let snap = test_snapshot(200.0, 2);

        let decision = scaler.evaluate(&spec, &snap);
        assert!(matches!(decision, ScaleDecision::ScaleTo(n) if n > 2));
    }

    #[test]
    fn scale_down_when_below_target() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let spec = test_spec_with_scaling("rps", 100.0);
        // RPS is 20 (below 50% of target) with 4 instances.
        let snap = test_snapshot(20.0, 4);

        let decision = scaler.evaluate(&spec, &snap);
        assert!(matches!(decision, ScaleDecision::ScaleTo(n) if n < 4));
    }

    #[test]
    fn no_change_when_near_target() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let spec = test_spec_with_scaling("rps", 100.0);
        // RPS is 95 — within 10% headroom, no scale-up.
        // And not below 50% threshold, no scale-down.
        let snap = test_snapshot(95.0, 2);

        assert_eq!(scaler.evaluate(&spec, &snap), ScaleDecision::NoChange);
    }

    #[test]
    fn respects_max_instances() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let mut spec = test_spec_with_scaling("rps", 10.0);
        spec.instances.max = 5;
        // RPS is 1000 with 1 instance → wants 100, capped at 5.
        let snap = test_snapshot(1000.0, 1);

        let decision = scaler.evaluate(&spec, &snap);
        assert_eq!(decision, ScaleDecision::ScaleTo(5));
    }

    #[test]
    fn respects_min_instances() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let mut spec = test_spec_with_scaling("rps", 100.0);
        spec.instances.min = 2;
        // RPS is 5, wants to scale down, but min is 2.
        let snap = test_snapshot(5.0, 4);

        let decision = scaler.evaluate(&spec, &snap);
        assert!(matches!(decision, ScaleDecision::ScaleTo(n) if n >= 2));
    }

    #[test]
    fn scale_to_zero_when_no_traffic() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let mut spec = test_spec_with_scaling("rps", 100.0);
        spec.instances.min = 0; // Allow scale-to-zero.
        let snap = test_snapshot(0.0, 2);

        let decision = scaler.evaluate(&spec, &snap);
        assert_eq!(decision, ScaleDecision::ScaleTo(0));
    }

    #[test]
    fn no_scale_to_zero_when_min_is_positive() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let spec = test_spec_with_scaling("rps", 100.0);
        // min = 1 (default), so scale-to-zero is blocked.
        let snap = test_snapshot(0.0, 2);

        let decision = scaler.evaluate(&spec, &snap);
        // Should scale down but not to zero.
        assert!(matches!(decision, ScaleDecision::ScaleTo(n) if n >= 1));
    }

    #[test]
    fn latency_based_scaling() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let spec = test_spec_with_scaling("latency_p99", 50.0);
        let mut snap = test_snapshot(100.0, 2);
        snap.latency_p99_ms = 120.0; // P99 is 120ms, target is 50ms.

        let decision = scaler.evaluate(&spec, &snap);
        assert!(matches!(decision, ScaleDecision::ScaleTo(n) if n > 2));
    }

    #[test]
    fn unknown_metric_returns_no_change() {
        let state = StateStore::open_in_memory().unwrap();
        let mut scaler = Autoscaler::new(state);

        let spec = test_spec_with_scaling("cpu_usage", 50.0);
        let snap = test_snapshot(100.0, 2);

        assert_eq!(scaler.evaluate(&spec, &snap), ScaleDecision::NoChange);
    }

    #[test]
    fn parse_duration_secs_values() {
        assert_eq!(parse_duration_secs("30s"), 30);
        assert_eq!(parse_duration_secs("5m"), 300);
        assert_eq!(parse_duration_secs("0s"), 0);
        assert_eq!(parse_duration_secs("invalid"), 30);
    }

    #[tokio::test]
    async fn evaluate_all_reads_from_state() {
        let state = StateStore::open_in_memory().unwrap();

        let spec = test_spec_with_scaling("rps", 100.0);
        state.put_deployment(&spec).unwrap();
        state.put_metrics(&test_snapshot(200.0, 2)).unwrap();

        let mut scaler = Autoscaler::new(state);
        let decisions = scaler.evaluate_all().await.unwrap();

        assert_eq!(decisions.len(), 1);
        assert!(matches!(
            decisions[0].1,
            ScaleDecision::ScaleTo(n) if n > 2
        ));
    }
}
