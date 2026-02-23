//! Rollout controller — drives the rollout state machine.
//!
//! The controller progresses through rollout phases, checking health
//! gates between batches. It can pause, resume, or rollback.

use std::time::Instant;

use tracing::{debug, info, warn};

use crate::strategy::{CanaryConfig, RolloutStrategy};

/// Current phase of a rollout.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RolloutPhase {
    /// Rollout not started.
    Pending,
    /// Rolling update: processing batch N of M.
    RollingBatch { current: u32, total: u32 },
    /// Canary: observing canary traffic.
    CanaryObserving,
    /// Canary: promoting to full rollout.
    CanaryPromoting,
    /// Waiting for health gate to pass.
    HealthGate,
    /// Paused by operator.
    Paused,
    /// Completed successfully.
    Completed,
    /// Rolled back due to failure.
    RolledBack { reason: String },
}

/// Health metrics for a rollout health gate.
#[derive(Debug, Clone)]
pub struct HealthMetrics {
    /// Number of healthy instances.
    pub healthy_count: u32,
    /// Total instances (including unhealthy).
    pub total_count: u32,
    /// Error rate as a percentage (0-100).
    pub error_rate: f64,
    /// P99 latency in milliseconds.
    pub p99_latency_ms: u64,
}

/// A rollout in progress.
#[derive(Debug, Clone)]
pub struct Rollout {
    pub deployment_id: String,
    pub strategy: RolloutStrategy,
    pub phase: RolloutPhase,
    pub target_instances: u32,
    pub old_version: String,
    pub new_version: String,
    pub started_at: Option<Instant>,
}

impl Rollout {
    /// Create a new rollout.
    pub fn new(
        deployment_id: &str,
        strategy: RolloutStrategy,
        target_instances: u32,
        old_version: &str,
        new_version: &str,
    ) -> Self {
        Self {
            deployment_id: deployment_id.to_string(),
            strategy,
            phase: RolloutPhase::Pending,
            target_instances,
            old_version: old_version.to_string(),
            new_version: new_version.to_string(),
            started_at: None,
        }
    }

    /// Start the rollout.
    pub fn start(&mut self) {
        self.started_at = Some(Instant::now());
        match &self.strategy {
            RolloutStrategy::Rolling(cfg) => {
                let total_batches = batch_count(self.target_instances, cfg.batch_size);
                self.phase = RolloutPhase::RollingBatch {
                    current: 1,
                    total: total_batches,
                };
                info!(
                    deployment = %self.deployment_id,
                    batches = total_batches,
                    batch_size = cfg.batch_size,
                    "started rolling update"
                );
            }
            RolloutStrategy::Canary(_) => {
                self.phase = RolloutPhase::CanaryObserving;
                info!(
                    deployment = %self.deployment_id,
                    "started canary deployment"
                );
            }
            RolloutStrategy::BlueGreen => {
                self.phase = RolloutPhase::HealthGate;
                info!(
                    deployment = %self.deployment_id,
                    "started blue-green deployment"
                );
            }
        }
    }

    /// Advance the rollout by one step, given current health metrics.
    ///
    /// Returns the instances to update in this step, or None if the
    /// rollout is complete/paused/rolled-back.
    pub fn advance(&mut self, health: &HealthMetrics) -> Option<BatchAction> {
        match &self.phase {
            RolloutPhase::Pending => None,
            RolloutPhase::Paused => None,
            RolloutPhase::Completed => None,
            RolloutPhase::RolledBack { .. } => None,

            RolloutPhase::RollingBatch { current, total } => {
                let current = *current;
                let total = *total;

                // Check health gate.
                if !self.check_health_gate(health) {
                    self.phase = RolloutPhase::RolledBack {
                        reason: format!(
                            "health gate failed at batch {}/{}: error_rate={:.1}%",
                            current, total, health.error_rate
                        ),
                    };
                    warn!(
                        deployment = %self.deployment_id,
                        batch = current,
                        "rolling back — health gate failed"
                    );
                    return Some(BatchAction::Rollback);
                }

                let cfg = match &self.strategy {
                    RolloutStrategy::Rolling(c) => c.clone(),
                    _ => unreachable!(),
                };

                let start = (current - 1) * cfg.batch_size;
                let count = cfg.batch_size.min(self.target_instances - start);

                if current >= total {
                    self.phase = RolloutPhase::Completed;
                    info!(deployment = %self.deployment_id, "rolling update completed");
                } else {
                    self.phase = RolloutPhase::RollingBatch {
                        current: current + 1,
                        total,
                    };
                    debug!(
                        deployment = %self.deployment_id,
                        batch = current + 1,
                        total,
                        "advancing to next batch"
                    );
                }

                Some(BatchAction::UpdateBatch {
                    start_index: start,
                    count,
                })
            }

            RolloutPhase::CanaryObserving => {
                let cfg = match &self.strategy {
                    RolloutStrategy::Canary(c) => c.clone(),
                    _ => unreachable!(),
                };

                if !self.check_canary_health(health, &cfg) {
                    self.phase = RolloutPhase::RolledBack {
                        reason: format!(
                            "canary failed: error_rate={:.1}%, p99={}ms",
                            health.error_rate, health.p99_latency_ms
                        ),
                    };
                    warn!(deployment = %self.deployment_id, "canary rolled back");
                    return Some(BatchAction::Rollback);
                }

                self.phase = RolloutPhase::CanaryPromoting;
                info!(deployment = %self.deployment_id, "canary passed, promoting");
                Some(BatchAction::PromoteCanary)
            }

            RolloutPhase::CanaryPromoting => {
                self.phase = RolloutPhase::Completed;
                info!(deployment = %self.deployment_id, "canary promotion completed");
                Some(BatchAction::UpdateBatch {
                    start_index: 0,
                    count: self.target_instances,
                })
            }

            RolloutPhase::HealthGate => {
                if !self.check_health_gate(health) {
                    self.phase = RolloutPhase::RolledBack {
                        reason: "blue-green health gate failed".to_string(),
                    };
                    return Some(BatchAction::Rollback);
                }
                self.phase = RolloutPhase::Completed;
                info!(deployment = %self.deployment_id, "blue-green switch completed");
                Some(BatchAction::SwitchTraffic)
            }
        }
    }

    /// Pause the rollout.
    pub fn pause(&mut self) {
        if self.phase != RolloutPhase::Completed
            && !matches!(self.phase, RolloutPhase::RolledBack { .. })
        {
            info!(deployment = %self.deployment_id, "pausing rollout");
            self.phase = RolloutPhase::Paused;
        }
    }

    /// Resume a paused rollout (restores to health gate).
    pub fn resume(&mut self) {
        if self.phase == RolloutPhase::Paused {
            info!(deployment = %self.deployment_id, "resuming rollout");
            self.phase = RolloutPhase::HealthGate;
        }
    }

    /// Check if the health gate passes.
    fn check_health_gate(&self, health: &HealthMetrics) -> bool {
        if health.total_count == 0 {
            return true; // No instances to check.
        }

        let healthy_ratio = health.healthy_count as f64 / health.total_count as f64;

        // Require at least 80% healthy.
        if healthy_ratio < 0.8 {
            return false;
        }

        // Error rate must be below 10%.
        if health.error_rate > 10.0 {
            return false;
        }

        true
    }

    /// Check canary-specific health thresholds.
    fn check_canary_health(&self, health: &HealthMetrics, cfg: &CanaryConfig) -> bool {
        if health.error_rate > cfg.error_rate_threshold {
            return false;
        }
        if health.p99_latency_ms > cfg.latency_threshold_ms {
            return false;
        }
        true
    }
}

/// Action to take for a rollout batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchAction {
    /// Update instances in range [start_index, start_index + count).
    UpdateBatch { start_index: u32, count: u32 },
    /// Rollback all instances to the old version.
    Rollback,
    /// Promote canary to full rollout.
    PromoteCanary,
    /// Switch all traffic (blue-green).
    SwitchTraffic,
}

/// Calculate number of batches for a rolling update.
fn batch_count(total_instances: u32, batch_size: u32) -> u32 {
    if batch_size == 0 {
        return 1;
    }
    total_instances.div_ceil(batch_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{CanaryConfig, RollingConfig};

    fn healthy_metrics() -> HealthMetrics {
        HealthMetrics {
            healthy_count: 10,
            total_count: 10,
            error_rate: 0.5,
            p99_latency_ms: 50,
        }
    }

    fn unhealthy_metrics() -> HealthMetrics {
        HealthMetrics {
            healthy_count: 2,
            total_count: 10,
            error_rate: 25.0,
            p99_latency_ms: 5000,
        }
    }

    #[test]
    fn rolling_update_completes_in_batches() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::Rolling(RollingConfig {
                batch_size: 2,
                ..Default::default()
            }),
            4, // 4 instances → 2 batches.
            "v1",
            "v2",
        );

        rollout.start();
        assert_eq!(
            rollout.phase,
            RolloutPhase::RollingBatch {
                current: 1,
                total: 2
            }
        );

        // Batch 1.
        let action = rollout.advance(&healthy_metrics()).unwrap();
        assert_eq!(
            action,
            BatchAction::UpdateBatch {
                start_index: 0,
                count: 2
            }
        );

        // Batch 2 (final).
        let action = rollout.advance(&healthy_metrics()).unwrap();
        assert_eq!(
            action,
            BatchAction::UpdateBatch {
                start_index: 2,
                count: 2
            }
        );
        assert_eq!(rollout.phase, RolloutPhase::Completed);
    }

    #[test]
    fn rolling_rollback_on_unhealthy() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::Rolling(RollingConfig::default()),
            3,
            "v1",
            "v2",
        );

        rollout.start();
        let action = rollout.advance(&unhealthy_metrics()).unwrap();
        assert_eq!(action, BatchAction::Rollback);
        assert!(matches!(rollout.phase, RolloutPhase::RolledBack { .. }));
    }

    #[test]
    fn canary_promotes_on_healthy() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::Canary(CanaryConfig::default()),
            5,
            "v1",
            "v2",
        );

        rollout.start();
        assert_eq!(rollout.phase, RolloutPhase::CanaryObserving);

        let action = rollout.advance(&healthy_metrics()).unwrap();
        assert_eq!(action, BatchAction::PromoteCanary);
        assert_eq!(rollout.phase, RolloutPhase::CanaryPromoting);

        let action = rollout.advance(&healthy_metrics()).unwrap();
        assert_eq!(
            action,
            BatchAction::UpdateBatch {
                start_index: 0,
                count: 5
            }
        );
        assert_eq!(rollout.phase, RolloutPhase::Completed);
    }

    #[test]
    fn canary_rollback_on_high_error_rate() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::Canary(CanaryConfig {
                error_rate_threshold: 2.0,
                ..Default::default()
            }),
            5,
            "v1",
            "v2",
        );

        rollout.start();
        let metrics = HealthMetrics {
            error_rate: 3.0, // Exceeds 2.0 threshold.
            ..healthy_metrics()
        };
        let action = rollout.advance(&metrics).unwrap();
        assert_eq!(action, BatchAction::Rollback);
    }

    #[test]
    fn canary_rollback_on_high_latency() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::Canary(CanaryConfig {
                latency_threshold_ms: 100,
                ..Default::default()
            }),
            5,
            "v1",
            "v2",
        );

        rollout.start();
        let metrics = HealthMetrics {
            p99_latency_ms: 200, // Exceeds 100ms threshold.
            ..healthy_metrics()
        };
        let action = rollout.advance(&metrics).unwrap();
        assert_eq!(action, BatchAction::Rollback);
    }

    #[test]
    fn blue_green_switches_on_healthy() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::BlueGreen,
            5,
            "v1",
            "v2",
        );

        rollout.start();
        assert_eq!(rollout.phase, RolloutPhase::HealthGate);

        let action = rollout.advance(&healthy_metrics()).unwrap();
        assert_eq!(action, BatchAction::SwitchTraffic);
        assert_eq!(rollout.phase, RolloutPhase::Completed);
    }

    #[test]
    fn pause_and_resume() {
        let mut rollout = Rollout::new(
            "deploy/a",
            RolloutStrategy::Rolling(RollingConfig::default()),
            3,
            "v1",
            "v2",
        );

        rollout.start();
        rollout.pause();
        assert_eq!(rollout.phase, RolloutPhase::Paused);

        assert!(rollout.advance(&healthy_metrics()).is_none());

        rollout.resume();
        assert_eq!(rollout.phase, RolloutPhase::HealthGate);
    }

    #[test]
    fn batch_count_calculation() {
        assert_eq!(batch_count(4, 2), 2);
        assert_eq!(batch_count(5, 2), 3);
        assert_eq!(batch_count(1, 1), 1);
        assert_eq!(batch_count(10, 3), 4);
        assert_eq!(batch_count(0, 5), 0);
    }
}
