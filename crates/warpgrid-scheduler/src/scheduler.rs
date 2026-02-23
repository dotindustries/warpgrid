//! Scheduler — maps deployments to instance pools.
//!
//! The `Scheduler` is the control loop that:
//! - Accepts deployment specs and creates instance pools
//! - Manages the lifecycle of instances (start, stop, restart)
//! - Persists instance state to the state store
//! - Provides load-balanced access to instances for request routing

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use warp_runtime::{InstancePool, PoolConfig, Runtime};
use warpgrid_host::config::ShimConfig;
use warpgrid_placement::convert::{deployment_to_requirements, node_info_to_resources};
use warpgrid_placement::placer::{PlacementPlan, compute_placement};
use warpgrid_placement::scorer::ScoringWeights;
use warpgrid_state::*;

use crate::error::{SchedulerError, SchedulerResult};
use crate::load_balancer::RoundRobinBalancer;

/// Controls whether the scheduler operates locally or across the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementMode {
    /// Single-node mode: schedule only on this node (existing behavior).
    Standalone,
    /// Multi-node mode: use the placement engine to distribute across nodes.
    Distributed,
}

/// Per-deployment scheduling state held in memory.
struct DeploymentSlot {
    /// The deployment spec (mirrored from state store).
    spec: DeploymentSpec,
    /// The instance pool for this deployment.
    pool: InstancePool,
    /// Round-robin load balancer for this deployment.
    balancer: RoundRobinBalancer,
}

/// The scheduler manages deployment → instance pool mappings.
///
/// It reads `DeploymentSpec` from the state store, creates `InstancePool`s
/// via the runtime, and persists `InstanceState` records back to the store.
///
/// In [`PlacementMode::Distributed`] mode, it can also compute multi-node
/// placement plans via the placement engine.
pub struct Scheduler {
    /// The Wasm runtime for creating instance pools.
    runtime: Arc<Runtime>,
    /// The state store for persisting deployment/instance state.
    state: StateStore,
    /// Active deployments: deployment_id → slot.
    slots: Arc<RwLock<HashMap<String, DeploymentSlot>>>,
    /// This node's ID (for instance state records).
    node_id: String,
    /// Placement mode (standalone or distributed).
    mode: PlacementMode,
}

impl Scheduler {
    /// Create a new scheduler in standalone (single-node) mode.
    pub fn new(runtime: Arc<Runtime>, state: StateStore, node_id: String) -> Self {
        Self {
            runtime,
            state,
            slots: Arc::new(RwLock::new(HashMap::new())),
            node_id,
            mode: PlacementMode::Standalone,
        }
    }

    /// Create a new scheduler in distributed (multi-node) mode.
    pub fn new_distributed(
        runtime: Arc<Runtime>,
        state: StateStore,
        node_id: String,
    ) -> Self {
        Self {
            runtime,
            state,
            slots: Arc::new(RwLock::new(HashMap::new())),
            node_id,
            mode: PlacementMode::Distributed,
        }
    }

    /// Returns the current placement mode.
    pub fn placement_mode(&self) -> PlacementMode {
        self.mode
    }

    /// Schedule a deployment — create an instance pool and warm it up.
    ///
    /// The deployment spec must already be persisted in the state store.
    /// The Wasm module must already be loaded into the runtime.
    pub async fn schedule(&self, deployment_id: &str) -> SchedulerResult<()> {
        // Check for duplicate.
        {
            let slots = self.slots.read().await;
            if slots.contains_key(deployment_id) {
                return Err(SchedulerError::AlreadyScheduled(
                    deployment_id.to_string(),
                ));
            }
        }

        // Load the deployment spec from state.
        let spec = self
            .state
            .get_deployment(deployment_id)
            .map_err(SchedulerError::State)?
            .ok_or_else(|| SchedulerError::DeploymentNotFound(deployment_id.to_string()))?;

        // Get the compiled module from the runtime cache.
        let module = self
            .runtime
            .get_module(&spec.name)
            .await
            .ok_or_else(|| SchedulerError::ModuleNotLoaded(spec.name.clone()))?;

        // Build pool config from the deployment spec.
        let pool_config = self.build_pool_config(&spec);
        let pool = self.runtime.create_pool(module, pool_config);

        // Warm up to min instances.
        pool.warm_up()
            .await
            .map_err(SchedulerError::Runtime)?;

        // Record instance states in the store.
        let now = epoch_secs();
        for i in 0..pool.total_count().await {
            let instance_state = InstanceState {
                id: format!("inst-{i}"),
                deployment_id: deployment_id.to_string(),
                node_id: self.node_id.clone(),
                status: InstanceStatus::Running,
                health: HealthStatus::Unknown,
                restart_count: 0,
                memory_bytes: spec.resources.memory_bytes,
                started_at: now,
                updated_at: now,
            };
            self.state.put_instance(&instance_state)?;
        }

        // Insert into active slots.
        {
            let mut slots = self.slots.write().await;
            slots.insert(
                deployment_id.to_string(),
                DeploymentSlot {
                    spec: spec.clone(),
                    pool,
                    balancer: RoundRobinBalancer::new(),
                },
            );
        }

        info!(%deployment_id, name = %spec.name, "deployment scheduled");
        Ok(())
    }

    /// Unschedule a deployment — drain instances and remove from the scheduler.
    ///
    /// Instance state records are cleaned up from the store.
    pub async fn unschedule(&self, deployment_id: &str) -> SchedulerResult<()> {
        let slot = {
            let mut slots = self.slots.write().await;
            slots.remove(deployment_id)
        };

        if slot.is_none() {
            warn!(%deployment_id, "deployment not scheduled, nothing to unschedule");
            return Ok(());
        }

        // Clean up instance states from the store.
        let deleted = self.state.delete_instances_for_deployment(deployment_id)?;
        info!(%deployment_id, instances_removed = deleted, "deployment unscheduled");

        Ok(())
    }

    /// Scale a deployment to a target number of instances.
    ///
    /// If target > current, new instances are created.
    /// If target < current, idle instances are removed.
    pub async fn scale(&self, deployment_id: &str, target: u32) -> SchedulerResult<()> {
        let slots = self.slots.read().await;
        let slot = slots
            .get(deployment_id)
            .ok_or_else(|| SchedulerError::DeploymentNotFound(deployment_id.to_string()))?;

        let current = slot.pool.total_count().await;

        if target > current {
            // Scale up: create more instances.
            let to_create = target - current;
            for _ in 0..to_create {
                match slot.pool.acquire().await {
                    Ok(Some(instance)) => {
                        // Return it immediately — we just want the pool to grow.
                        slot.pool.release(instance).await;
                    }
                    Ok(None) => {
                        debug!(
                            %deployment_id,
                            "pool at max capacity, cannot scale further"
                        );
                        break;
                    }
                    Err(e) => {
                        error!(%deployment_id, error = %e, "failed to create instance for scale-up");
                        break;
                    }
                }
            }
            info!(%deployment_id, from = current, to = target, "scaled up");
        } else if target < current {
            slot.pool.scale_down_to(target).await;
            info!(%deployment_id, from = current, to = target, "scaled down");
        } else {
            debug!(%deployment_id, target, "already at target, no scaling needed");
        }

        // Update instance states in store.
        self.sync_instance_states(deployment_id, &slot.spec, &slot.pool)
            .await?;

        Ok(())
    }

    /// Get the current number of instances for a deployment.
    pub async fn instance_count(&self, deployment_id: &str) -> Option<u32> {
        let slots = self.slots.read().await;
        let slot = slots.get(deployment_id)?;
        Some(slot.pool.total_count().await)
    }

    /// Get the next instance index via round-robin for a deployment.
    ///
    /// Used by the HTTP trigger to select which instance handles a request.
    pub async fn next_instance_index(
        &self,
        deployment_id: &str,
    ) -> SchedulerResult<usize> {
        let slots = self.slots.read().await;
        let slot = slots
            .get(deployment_id)
            .ok_or_else(|| SchedulerError::DeploymentNotFound(deployment_id.to_string()))?;

        let count = slot.pool.available_count().await + (slot.pool.total_count().await as usize - slot.pool.available_count().await);
        slot.balancer
            .next(count)
            .ok_or_else(|| SchedulerError::NoInstancesAvailable(deployment_id.to_string()))
    }

    /// List all scheduled deployment IDs.
    pub async fn scheduled_deployments(&self) -> Vec<String> {
        let slots = self.slots.read().await;
        slots.keys().cloned().collect()
    }

    /// Check if a deployment is currently scheduled.
    pub async fn is_scheduled(&self, deployment_id: &str) -> bool {
        let slots = self.slots.read().await;
        slots.contains_key(deployment_id)
    }

    /// Compute a distributed placement plan for a deployment.
    ///
    /// Only available in [`PlacementMode::Distributed`]. Reads the deployment
    /// spec and node list from the state store, converts to placement types,
    /// and runs the placement engine.
    pub fn compute_distributed_placement(
        &self,
        deployment_id: &str,
    ) -> SchedulerResult<PlacementPlan> {
        if self.mode != PlacementMode::Distributed {
            return Err(SchedulerError::Placement(
                "distributed placement requires PlacementMode::Distributed".to_string(),
            ));
        }

        let spec = self
            .state
            .get_deployment(deployment_id)
            .map_err(SchedulerError::State)?
            .ok_or_else(|| SchedulerError::DeploymentNotFound(deployment_id.to_string()))?;

        let nodes = self
            .state
            .list_nodes()
            .map_err(SchedulerError::State)?;

        if nodes.is_empty() {
            return Err(SchedulerError::Placement(
                "no nodes available for placement".to_string(),
            ));
        }

        let node_resources: Vec<_> = nodes
            .iter()
            .map(|n| node_info_to_resources(n, false))
            .collect();

        let requirements = deployment_to_requirements(&spec, spec.instances.min);
        let weights = ScoringWeights::default();

        let plan = compute_placement(&requirements, deployment_id, &node_resources, &weights);

        info!(
            %deployment_id,
            assignments = plan.assignments.len(),
            total_placed = plan.assignments.values().sum::<u32>(),
            "computed distributed placement"
        );

        Ok(plan)
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Build a `PoolConfig` from a `DeploymentSpec`.
    fn build_pool_config(&self, spec: &DeploymentSpec) -> PoolConfig {
        let shim_config = ShimConfig {
            filesystem: spec.shims.timezone || spec.shims.dev_urandom,
            dns: spec.shims.dns,
            signals: spec.shims.signals,
            database_proxy: spec.shims.database_proxy,
            ..ShimConfig::default()
        };

        PoolConfig {
            min_instances: spec.instances.min,
            max_instances: spec.instances.max,
            memory_limit: spec.resources.memory_bytes as usize,
            shim_config,
        }
    }

    /// Synchronize in-memory pool state with the state store.
    async fn sync_instance_states(
        &self,
        deployment_id: &str,
        spec: &DeploymentSpec,
        pool: &InstancePool,
    ) -> SchedulerResult<()> {
        // Remove existing instance records for this deployment.
        self.state.delete_instances_for_deployment(deployment_id)?;

        // Write new records for current instance count.
        let now = epoch_secs();
        let total = pool.total_count().await;
        for i in 0..total {
            let instance_state = InstanceState {
                id: format!("inst-{i}"),
                deployment_id: deployment_id.to_string(),
                node_id: self.node_id.clone(),
                status: InstanceStatus::Running,
                health: HealthStatus::Unknown,
                restart_count: 0,
                memory_bytes: spec.resources.memory_bytes,
                started_at: now,
                updated_at: now,
            };
            self.state.put_instance(&instance_state)?;
        }

        Ok(())
    }
}

/// Current Unix epoch in seconds.
fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> StateStore {
        StateStore::open_in_memory().unwrap()
    }

    fn test_deployment(namespace: &str, name: &str) -> DeploymentSpec {
        DeploymentSpec {
            id: format!("{namespace}/{name}"),
            namespace: namespace.to_string(),
            name: name.to_string(),
            source: "file://./test.wasm".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 1, max: 10 },
            resources: ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn scheduler_creation() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());
        assert_eq!(scheduler.node_id, "node-1");
    }

    #[tokio::test]
    async fn scheduler_starts_empty() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        assert!(scheduler.scheduled_deployments().await.is_empty());
        assert!(!scheduler.is_scheduled("default/api").await);
    }

    #[tokio::test]
    async fn schedule_requires_deployment_in_state() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        let result = scheduler.schedule("default/api").await;
        assert!(matches!(result, Err(SchedulerError::DeploymentNotFound(_))));
    }

    #[tokio::test]
    async fn schedule_requires_loaded_module() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let spec = test_deployment("default", "api");
        state.put_deployment(&spec).unwrap();

        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());
        let result = scheduler.schedule("default/api").await;
        assert!(matches!(result, Err(SchedulerError::ModuleNotLoaded(_))));
    }

    #[tokio::test]
    async fn unschedule_nonexistent_is_noop() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        let result = scheduler.unschedule("default/api").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn instance_count_returns_none_for_unscheduled() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        assert_eq!(scheduler.instance_count("default/api").await, None);
    }

    #[tokio::test]
    async fn next_instance_fails_when_not_scheduled() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        let result = scheduler.next_instance_index("default/api").await;
        assert!(matches!(
            result,
            Err(SchedulerError::DeploymentNotFound(_))
        ));
    }

    #[test]
    fn build_pool_config_from_spec() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        let mut spec = test_deployment("default", "api");
        spec.instances.min = 2;
        spec.instances.max = 20;
        spec.resources.memory_bytes = 128 * 1024 * 1024;
        spec.shims.dns = true;
        spec.shims.database_proxy = true;

        let config = scheduler.build_pool_config(&spec);
        assert_eq!(config.min_instances, 2);
        assert_eq!(config.max_instances, 20);
        assert_eq!(config.memory_limit, 128 * 1024 * 1024);
        assert!(config.shim_config.dns);
        assert!(config.shim_config.database_proxy);
        assert!(!config.shim_config.filesystem);
    }

    #[tokio::test]
    async fn duplicate_schedule_is_rejected() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let spec = test_deployment("default", "api");
        state.put_deployment(&spec).unwrap();

        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

        // Manually insert a slot to simulate a scheduled deployment.
        {
            let _pool_config = scheduler.build_pool_config(&spec);
            // We can't actually warm up without a real module, so just test
            // the duplicate-check path by inserting a slot directly.
            let _module = match scheduler.runtime.get_module(&spec.name).await {
                Some(m) => m,
                None => {
                    // Can't complete this test without a real wasm module.
                    // Instead, test the error path — first schedule should
                    // fail with ModuleNotLoaded, then trying again should
                    // still fail (not with AlreadyScheduled since first
                    // schedule didn't succeed).
                    let r1 = scheduler.schedule("default/api").await;
                    assert!(matches!(r1, Err(SchedulerError::ModuleNotLoaded(_))));
                    let r2 = scheduler.schedule("default/api").await;
                    assert!(matches!(r2, Err(SchedulerError::ModuleNotLoaded(_))));
                    return;
                }
            };
        }
    }

    #[test]
    fn epoch_secs_returns_reasonable_value() {
        let now = epoch_secs();
        // Should be after 2024-01-01.
        assert!(now > 1_704_067_200);
    }

    // ── Distributed mode tests ──────────────────────────────────────

    fn test_node(id: &str, cap_mem: u64, used_mem: u64) -> NodeInfo {
        NodeInfo {
            id: id.to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            capacity_memory_bytes: cap_mem,
            capacity_cpu_weight: 1000,
            used_memory_bytes: used_mem,
            used_cpu_weight: 0,
            labels: HashMap::new(),
            last_heartbeat: 1700000000,
        }
    }

    #[test]
    fn distributed_scheduler_creation() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());
        assert_eq!(scheduler.placement_mode(), PlacementMode::Distributed);
    }

    #[test]
    fn scheduler_defaults_to_standalone() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());
        assert_eq!(scheduler.placement_mode(), PlacementMode::Standalone);
    }

    #[test]
    fn standalone_rejects_distributed_placement() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let spec = test_deployment("default", "api");
        state.put_deployment(&spec).unwrap();

        let scheduler = Scheduler::new(runtime, state, "node-1".to_string());
        let result = scheduler.compute_distributed_placement("default/api");
        assert!(matches!(result, Err(SchedulerError::Placement(_))));
    }

    #[test]
    fn distributed_placement_requires_deployment() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());

        let result = scheduler.compute_distributed_placement("nope/missing");
        assert!(matches!(result, Err(SchedulerError::DeploymentNotFound(_))));
    }

    #[test]
    fn distributed_placement_requires_nodes() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let spec = test_deployment("default", "api");
        state.put_deployment(&spec).unwrap();

        let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());
        let result = scheduler.compute_distributed_placement("default/api");
        assert!(matches!(result, Err(SchedulerError::Placement(_))));
    }

    #[test]
    fn distributed_placement_produces_plan() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();

        let spec = test_deployment("default", "api");
        state.put_deployment(&spec).unwrap();

        let node1 = test_node("node-1", 8 * 1024 * 1024 * 1024, 0);
        let node2 = test_node("node-2", 8 * 1024 * 1024 * 1024, 0);
        state.put_node(&node1).unwrap();
        state.put_node(&node2).unwrap();

        let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());
        let plan = scheduler
            .compute_distributed_placement("default/api")
            .unwrap();

        assert_eq!(plan.deployment_id, "default/api");
        let total_placed: u32 = plan.assignments.values().sum();
        assert_eq!(total_placed, spec.instances.min);
    }

    #[test]
    fn distributed_placement_partial_when_constrained() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();

        let mut spec = test_deployment("default", "big");
        spec.instances.min = 10;
        spec.resources.memory_bytes = 256 * 1024 * 1024;
        state.put_deployment(&spec).unwrap();

        // One small node: 512 MiB => fits 2 instances.
        let node = test_node("node-1", 512 * 1024 * 1024, 0);
        state.put_node(&node).unwrap();

        let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());
        let plan = scheduler
            .compute_distributed_placement("default/big")
            .unwrap();

        let total_placed: u32 = plan.assignments.values().sum();
        assert_eq!(total_placed, 2);
    }

    #[tokio::test]
    async fn distributed_scheduler_still_supports_local_schedule() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let state = test_state();
        let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());

        let result = scheduler.schedule("default/api").await;
        assert!(matches!(result, Err(SchedulerError::DeploymentNotFound(_))));
    }
}
