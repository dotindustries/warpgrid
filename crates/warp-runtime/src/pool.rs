//! Instance pool — manages a set of Wasm instances for a deployment.
//!
//! Supports min/max instance scaling, round-robin dispatch, and
//! instance lifecycle management (create, recycle, destroy).

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::instance::{InstanceFactory, WasmInstance};
use warpgrid_host::config::ShimConfig;

/// Configuration for an instance pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Minimum number of warm instances to keep.
    pub min_instances: u32,
    /// Maximum number of instances allowed.
    pub max_instances: u32,
    /// Memory limit per instance (bytes).
    pub memory_limit: usize,
    /// Shim configuration for instances in this pool.
    pub shim_config: ShimConfig,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_instances: 1,
            max_instances: 10,
            memory_limit: 64 * 1024 * 1024,
            shim_config: ShimConfig::default(),
        }
    }
}

/// Manages a pool of Wasm instances for a single deployment.
///
/// Instances are created on-demand up to `max_instances` and recycled
/// back to the pool after use. The pool maintains at least `min_instances`
/// warm instances when possible.
pub struct InstancePool {
    factory: InstanceFactory,
    config: PoolConfig,
    /// Available (idle) instances ready for dispatch.
    available: Arc<Mutex<VecDeque<WasmInstance>>>,
    /// Total number of instances (available + checked out).
    total_count: Arc<Mutex<u32>>,
}

impl InstancePool {
    /// Create a new instance pool.
    pub fn new(factory: InstanceFactory, config: PoolConfig) -> Self {
        Self {
            factory,
            config,
            available: Arc::new(Mutex::new(VecDeque::new())),
            total_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Pre-warm the pool to `min_instances`.
    pub async fn warm_up(&self) -> anyhow::Result<()> {
        let current = *self.total_count.lock().await;
        let needed = self.config.min_instances.saturating_sub(current);

        for _ in 0..needed {
            let instance = self
                .factory
                .create_instance(&self.config.shim_config, self.config.memory_limit)
                .await?;
            self.available.lock().await.push_back(instance);
            *self.total_count.lock().await += 1;
        }

        info!(
            min = self.config.min_instances,
            warmed = needed,
            "instance pool warmed"
        );
        Ok(())
    }

    /// Acquire an instance from the pool.
    ///
    /// Returns an idle instance if available, or creates a new one if
    /// under the max limit. Returns `None` if at capacity.
    pub async fn acquire(&self) -> anyhow::Result<Option<WasmInstance>> {
        // Try to get an idle instance first.
        if let Some(instance) = self.available.lock().await.pop_front() {
            debug!("acquired idle instance from pool");
            return Ok(Some(instance));
        }

        // No idle instances — can we create a new one?
        let mut count = self.total_count.lock().await;
        if *count < self.config.max_instances {
            *count += 1;
            drop(count); // Release lock before async work.

            let instance = self
                .factory
                .create_instance(&self.config.shim_config, self.config.memory_limit)
                .await?;
            debug!("created new instance for pool");
            Ok(Some(instance))
        } else {
            debug!(
                max = self.config.max_instances,
                "pool at capacity, no instance available"
            );
            Ok(None)
        }
    }

    /// Return an instance to the pool for reuse.
    pub async fn release(&self, instance: WasmInstance) {
        self.available.lock().await.push_back(instance);
        debug!("instance returned to pool");
    }

    /// Current number of available (idle) instances.
    pub async fn available_count(&self) -> usize {
        self.available.lock().await.len()
    }

    /// Total instance count (idle + checked out).
    pub async fn total_count(&self) -> u32 {
        *self.total_count.lock().await
    }

    /// Maximum instances allowed.
    pub fn max_instances(&self) -> u32 {
        self.config.max_instances
    }

    /// Minimum instances to keep warm.
    pub fn min_instances(&self) -> u32 {
        self.config.min_instances
    }

    /// Scale down to a target instance count.
    ///
    /// Removes idle instances from the pool until total count reaches
    /// the target (but never below `min_instances`).
    pub async fn scale_down_to(&self, target: u32) {
        let target = target.max(self.config.min_instances);
        let mut available = self.available.lock().await;
        let mut count = self.total_count.lock().await;

        while *count > target && !available.is_empty() {
            available.pop_back(); // Drop the instance.
            *count -= 1;
        }

        debug!(target, actual = *count, "scaled down instance pool");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full pool tests require a real .wasm component to instantiate.
    // These tests verify the configuration and structural aspects.

    #[test]
    fn pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.min_instances, 1);
        assert_eq!(config.max_instances, 10);
        assert_eq!(config.memory_limit, 64 * 1024 * 1024);
    }

    #[test]
    fn pool_config_custom() {
        let config = PoolConfig {
            min_instances: 2,
            max_instances: 50,
            memory_limit: 128 * 1024 * 1024,
            shim_config: ShimConfig::default(),
        };
        assert_eq!(config.min_instances, 2);
        assert_eq!(config.max_instances, 50);
    }
}
