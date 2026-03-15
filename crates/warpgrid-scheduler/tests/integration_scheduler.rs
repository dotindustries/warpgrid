//! Integration tests for the warpgrid-scheduler crate.
//!
//! Tests cover the round-robin load balancer, placement modes,
//! placement executor, and scheduler lifecycle error paths.

use std::collections::HashMap;
use std::sync::Arc;

use warpgrid_placement::placer::PlacementPlan;
use warpgrid_scheduler::{
    PlacementMode, RoundRobinBalancer, SchedulePayload, Scheduler,
    SchedulerError, execute_placement,
};
use warpgrid_state::*;

// ── Helpers ──────────────────────────────────────────────────────────

fn in_memory_state() -> StateStore {
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

fn make_plan(deployment_id: &str, assignments: Vec<(&str, u32)>) -> PlacementPlan {
    PlacementPlan {
        deployment_id: deployment_id.to_string(),
        assignments: assignments
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        preemptions: Vec::new(),
    }
}

fn make_runtime() -> Arc<warp_runtime::Runtime> {
    Arc::new(
        warp_runtime::Runtime::new(warpgrid_host::config::ShimConfig::default()).unwrap(),
    )
}

// ── 1. Round-robin load balancer distribution ────────────────────────

#[test]
fn round_robin_distributes_evenly_across_indices() {
    let lb = RoundRobinBalancer::new();
    let count = 4;
    let rounds = 100;
    let mut distribution = vec![0u32; count];

    for _ in 0..count * rounds {
        let idx = lb.next(count).unwrap();
        distribution[idx] += 1;
    }

    // Each index should have been selected exactly `rounds` times.
    for &hits in &distribution {
        assert_eq!(hits, rounds as u32);
    }
}

#[test]
fn round_robin_wraps_around_multiple_cycles() {
    let lb = RoundRobinBalancer::new();

    let first_cycle: Vec<usize> = (0..5).map(|_| lb.next(5).unwrap()).collect();
    let second_cycle: Vec<usize> = (0..5).map(|_| lb.next(5).unwrap()).collect();

    assert_eq!(first_cycle, vec![0, 1, 2, 3, 4]);
    assert_eq!(second_cycle, vec![0, 1, 2, 3, 4]);
}

#[test]
fn round_robin_returns_none_for_zero_count() {
    let lb = RoundRobinBalancer::new();
    assert_eq!(lb.next(0), None);
}

#[test]
fn round_robin_concurrent_distribution() {
    let lb = Arc::new(RoundRobinBalancer::new());
    let mut handles = vec![];
    let per_thread = 250;
    let threads = 4;

    for _ in 0..threads {
        let lb = lb.clone();
        handles.push(std::thread::spawn(move || {
            let mut indices = Vec::with_capacity(per_thread);
            for _ in 0..per_thread {
                indices.push(lb.next(4).unwrap());
            }
            indices
        }));
    }

    let mut all_indices = Vec::new();
    for h in handles {
        all_indices.extend(h.join().unwrap());
    }

    assert_eq!(all_indices.len(), threads * per_thread);
    assert!(all_indices.iter().all(|&i| i < 4));
    assert_eq!(lb.current(), threads * per_thread);
}

// ── 2. Standalone rejects distributed placement ──────────────────────

#[test]
fn standalone_mode_rejects_compute_distributed_placement() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let spec = test_deployment("default", "api");
    state.put_deployment(&spec).unwrap();

    let scheduler = Scheduler::new(runtime, state, "node-1".to_string());
    assert_eq!(scheduler.placement_mode(), PlacementMode::Standalone);

    let result = scheduler.compute_distributed_placement("default/api");
    assert!(matches!(result, Err(SchedulerError::Placement(_))));
}

#[test]
fn distributed_mode_allows_placement_computation() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let spec = test_deployment("default", "api");
    state.put_deployment(&spec).unwrap();

    let node = test_node("node-1", 8 * 1024 * 1024 * 1024, 0);
    state.put_node(&node).unwrap();

    let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());
    assert_eq!(scheduler.placement_mode(), PlacementMode::Distributed);

    let plan = scheduler
        .compute_distributed_placement("default/api")
        .unwrap();

    let total: u32 = plan.assignments.values().sum();
    assert_eq!(total, spec.instances.min);
}

// ── 3. Placement executor with local/remote assignments ──────────────

#[test]
fn executor_all_local_produces_no_remote_commands() {
    let plan = make_plan("deploy/svc", vec![("local-node", 3)]);
    let state = in_memory_state();

    let result = execute_placement(&plan, "local-node", &state).unwrap();

    assert!(result.remote_commands.is_empty());
    assert_eq!(result.local_instances, 3);

    // Verify instances written to state.
    let instances = state.list_instances_for_deployment("deploy/svc").unwrap();
    assert_eq!(instances.len(), 3);
    assert!(instances.iter().all(|i| i.node_id == "local-node"));
}

#[test]
fn executor_all_remote_produces_commands() {
    let plan = make_plan("deploy/svc", vec![("remote-1", 2), ("remote-2", 1)]);
    let state = in_memory_state();

    let result = execute_placement(&plan, "local-node", &state).unwrap();

    assert_eq!(result.local_instances, 0);
    assert_eq!(result.remote_commands.len(), 2);

    for cmd in &result.remote_commands {
        assert_eq!(cmd.command_type, "schedule");
        let payload: SchedulePayload = serde_json::from_str(&cmd.payload).unwrap();
        assert_eq!(payload.deployment_id, "deploy/svc");
    }
}

#[test]
fn executor_mixed_local_and_remote() {
    let plan = make_plan("deploy/svc", vec![("local-node", 2), ("remote-1", 3)]);
    let state = in_memory_state();

    let result = execute_placement(&plan, "local-node", &state).unwrap();

    assert_eq!(result.local_instances, 2);
    assert_eq!(result.remote_commands.len(), 1);
    assert_eq!(result.remote_commands[0].node_id, "remote-1");

    let payload: SchedulePayload =
        serde_json::from_str(&result.remote_commands[0].payload).unwrap();
    assert_eq!(payload.instance_count, 3);

    // Total instances in state should be 5.
    let instances = state.list_instances_for_deployment("deploy/svc").unwrap();
    assert_eq!(instances.len(), 5);
}

#[test]
fn executor_writes_instances_with_starting_status() {
    let plan = make_plan("deploy/svc", vec![("node-1", 2)]);
    let state = in_memory_state();

    execute_placement(&plan, "node-1", &state).unwrap();

    let instances = state.list_instances_for_deployment("deploy/svc").unwrap();
    for inst in &instances {
        assert_eq!(inst.status, InstanceStatus::Starting);
        assert_eq!(inst.health, HealthStatus::Unknown);
    }
}

// ── 4. Empty plan is noop ────────────────────────────────────────────

#[test]
fn empty_plan_produces_no_commands_and_no_instances() {
    let plan = make_plan("deploy/svc", vec![]);
    let state = in_memory_state();

    let result = execute_placement(&plan, "node-1", &state).unwrap();

    assert!(result.remote_commands.is_empty());
    assert_eq!(result.local_instances, 0);

    let instances = state.list_instances_for_deployment("deploy/svc").unwrap();
    assert!(instances.is_empty());
}

// ── 5. Scheduler lifecycle error paths ───────────────────────────────

#[tokio::test]
async fn schedule_fails_when_deployment_not_in_state() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

    let result = scheduler.schedule("nonexistent/deploy").await;
    assert!(matches!(result, Err(SchedulerError::DeploymentNotFound(_))));
}

#[tokio::test]
async fn schedule_fails_when_module_not_loaded() {
    let runtime = make_runtime();
    let state = in_memory_state();

    let spec = test_deployment("default", "api");
    state.put_deployment(&spec).unwrap();

    let scheduler = Scheduler::new(runtime, state, "node-1".to_string());
    let result = scheduler.schedule("default/api").await;
    assert!(matches!(result, Err(SchedulerError::ModuleNotLoaded(_))));
}

#[tokio::test]
async fn unschedule_nonexistent_is_noop() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

    let result = scheduler.unschedule("nonexistent/deploy").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn scheduler_starts_with_no_scheduled_deployments() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

    assert!(scheduler.scheduled_deployments().await.is_empty());
    assert!(!scheduler.is_scheduled("any/deploy").await);
    assert_eq!(scheduler.instance_count("any/deploy").await, None);
}

#[tokio::test]
async fn next_instance_index_fails_for_unscheduled_deployment() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let scheduler = Scheduler::new(runtime, state, "node-1".to_string());

    let result = scheduler.next_instance_index("missing/deploy").await;
    assert!(matches!(result, Err(SchedulerError::DeploymentNotFound(_))));
}

#[test]
fn distributed_placement_requires_at_least_one_node() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let spec = test_deployment("default", "api");
    state.put_deployment(&spec).unwrap();

    let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());
    let result = scheduler.compute_distributed_placement("default/api");
    assert!(matches!(result, Err(SchedulerError::Placement(_))));
}

#[test]
fn distributed_placement_requires_deployment_in_state() {
    let runtime = make_runtime();
    let state = in_memory_state();
    let scheduler = Scheduler::new_distributed(runtime, state, "node-1".to_string());

    let result = scheduler.compute_distributed_placement("missing/deploy");
    assert!(matches!(result, Err(SchedulerError::DeploymentNotFound(_))));
}
