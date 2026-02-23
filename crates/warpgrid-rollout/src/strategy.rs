//! Rollout strategies — rolling update, canary, blue-green.

/// How to roll out a new version of a deployment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RolloutStrategy {
    /// Replace instances in batches. Default.
    Rolling(RollingConfig),
    /// Route a percentage of traffic to the new version.
    Canary(CanaryConfig),
    /// Spin up a full parallel set, then switch all traffic at once.
    BlueGreen,
}

impl Default for RolloutStrategy {
    fn default() -> Self {
        Self::Rolling(RollingConfig::default())
    }
}

/// Configuration for rolling updates.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RollingConfig {
    /// Number of instances to update per batch.
    pub batch_size: u32,
    /// Seconds to wait between batches.
    pub batch_interval_secs: u64,
    /// Seconds to wait for an instance to become healthy.
    pub health_timeout_secs: u64,
    /// Maximum number of instances that can be unavailable during rollout.
    pub max_unavailable: u32,
}

impl Default for RollingConfig {
    fn default() -> Self {
        Self {
            batch_size: 1,
            batch_interval_secs: 10,
            health_timeout_secs: 30,
            max_unavailable: 1,
        }
    }
}

/// Configuration for canary deployments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanaryConfig {
    /// Percentage of traffic to route to the canary (0-100).
    pub traffic_percent: u32,
    /// Number of canary instances to create.
    pub canary_instances: u32,
    /// Seconds to observe the canary before promoting.
    pub observation_secs: u64,
    /// Error rate threshold (percentage). Rollback if exceeded.
    pub error_rate_threshold: f64,
    /// Latency threshold in milliseconds. Rollback if p99 exceeds this.
    pub latency_threshold_ms: u64,
}

impl Default for CanaryConfig {
    fn default() -> Self {
        Self {
            traffic_percent: 10,
            canary_instances: 1,
            observation_secs: 300,
            error_rate_threshold: 5.0,
            latency_threshold_ms: 1000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rolling() {
        let s = RolloutStrategy::default();
        match s {
            RolloutStrategy::Rolling(cfg) => {
                assert_eq!(cfg.batch_size, 1);
                assert_eq!(cfg.max_unavailable, 1);
            }
            _ => panic!("expected Rolling"),
        }
    }

    #[test]
    fn serializes_roundtrip() {
        let strategy = RolloutStrategy::Canary(CanaryConfig {
            traffic_percent: 20,
            ..Default::default()
        });
        let json = serde_json::to_string(&strategy).unwrap();
        let back: RolloutStrategy = serde_json::from_str(&json).unwrap();
        match back {
            RolloutStrategy::Canary(cfg) => assert_eq!(cfg.traffic_percent, 20),
            _ => panic!("expected Canary"),
        }
    }
}
