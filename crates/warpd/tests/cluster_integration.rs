//! Cluster integration tests.
//!
//! Tests multi-node scenarios: cluster formation via MembershipManager,
//! deployment state replication via Raft, health propagation, placement
//! distribution, and proxy synchronization.
//!
//! These tests run entirely in-process with in-memory state stores and
//! Raft storage — no real TCP connections needed for most scenarios.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use openraft::storage::{RaftSnapshotBuilder, RaftStateMachine};
use warpgrid_cluster::MembershipManager;
use warpgrid_placement::convert;
use warpgrid_proxy::{DnsResolver, ProxySync, Router};
use warpgrid_raft::{NodeIdMap, StateMachine};
use warpgrid_state::*;

fn test_store() -> StateStore {
    StateStore::open_in_memory().unwrap()
}

fn test_deployment(ns: &str, name: &str) -> DeploymentSpec {
    DeploymentSpec {
        id: format!("{ns}/{name}"),
        namespace: ns.to_string(),
        name: name.to_string(),
        source: "file://test.wasm".to_string(),
        trigger: TriggerConfig::Http { port: Some(8080) },
        instances: InstanceConstraints { min: 2, max: 10 },
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

fn make_node(store: &StateStore, id: &str, addr: &str, port: u16) -> NodeInfo {
    let node = NodeInfo {
        id: id.to_string(),
        address: addr.to_string(),
        port,
        capacity_memory_bytes: 8_000_000_000,
        capacity_cpu_weight: 1000,
        used_memory_bytes: 0,
        used_cpu_weight: 0,
        labels: HashMap::new(),
        last_heartbeat: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    };
    store.put_node(&node).unwrap();
    node
}

fn make_instance(
    id: &str,
    deployment: &str,
    node: &str,
    status: InstanceStatus,
) -> InstanceState {
    InstanceState {
        id: id.to_string(),
        deployment_id: deployment.to_string(),
        node_id: node.to_string(),
        status,
        health: HealthStatus::Unknown,
        restart_count: 0,
        memory_bytes: 0,
        started_at: 1000,
        updated_at: 1000,
    }
}

// ── Cluster Formation ──────────────────────────────────────────

#[test]
fn cluster_formation_three_nodes() {
    let state = test_store();
    let mgr = MembershipManager::new(state);

    let labels = HashMap::new();
    let id1 = mgr
        .join("10.0.0.1", 8443, labels.clone(), 8_000_000_000, 1000)
        .unwrap();
    let id2 = mgr
        .join("10.0.0.2", 8443, labels.clone(), 8_000_000_000, 1000)
        .unwrap();
    let id3 = mgr
        .join("10.0.0.3", 8443, labels, 8_000_000_000, 1000)
        .unwrap();

    // All three nodes should appear.
    let members = mgr.list_members().unwrap();
    assert_eq!(members.len(), 3);

    // All should be Ready (freshly joined = recent heartbeat).
    assert_eq!(mgr.ready_count().unwrap(), 3);

    // Each has unique ID.
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}

#[test]
fn cluster_node_leave_and_rejoin() {
    let state = test_store();
    let mgr = MembershipManager::new(state);

    let id = mgr
        .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
        .unwrap();

    assert_eq!(mgr.ready_count().unwrap(), 1);

    mgr.leave(&id).unwrap();
    assert_eq!(mgr.ready_count().unwrap(), 0);

    // Rejoin.
    let _new_id = mgr
        .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
        .unwrap();

    assert_eq!(mgr.ready_count().unwrap(), 1);
}

// ── Deployment State Replication (via State Store) ────────────

#[test]
fn deploy_state_persists_and_reads_back() {
    let state = test_store();
    let spec = test_deployment("prod", "api");

    state.put_deployment(&spec).unwrap();

    let back = state.get_deployment("prod/api").unwrap().unwrap();
    assert_eq!(back.name, "api");
    assert_eq!(back.namespace, "prod");
    assert_eq!(back.instances.min, 2);

    let all = state.list_deployments().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn deploy_with_instances_across_nodes() {
    let state = test_store();
    let spec = test_deployment("prod", "web");
    state.put_deployment(&spec).unwrap();

    make_node(&state, "node-1", "10.0.0.1", 8443);
    make_node(&state, "node-2", "10.0.0.2", 8443);

    let i1 = make_instance("i1", "prod/web", "node-1", InstanceStatus::Running);
    let i2 = make_instance("i2", "prod/web", "node-2", InstanceStatus::Running);
    state.put_instance(&i1).unwrap();
    state.put_instance(&i2).unwrap();

    let instances = state.list_instances_for_deployment("prod/web").unwrap();
    assert_eq!(instances.len(), 2);

    // Instances on different nodes.
    let nodes: Vec<&str> = instances.iter().map(|i| i.node_id.as_str()).collect();
    assert!(nodes.contains(&"node-1"));
    assert!(nodes.contains(&"node-2"));
}

// ── Health Propagation ──────────────────────────────────────────

#[test]
fn health_status_propagation() {
    let state = test_store();
    let spec = test_deployment("prod", "api");
    state.put_deployment(&spec).unwrap();

    make_node(&state, "node-1", "10.0.0.1", 8443);

    let mut inst = make_instance("i1", "prod/api", "node-1", InstanceStatus::Running);
    inst.health = HealthStatus::Healthy;
    state.put_instance(&inst).unwrap();

    // Verify health status is stored.
    let instances = state.list_instances_for_deployment("prod/api").unwrap();
    assert_eq!(instances[0].health, HealthStatus::Healthy);

    // Update to unhealthy.
    inst.health = HealthStatus::Unhealthy;
    inst.status = InstanceStatus::Unhealthy;
    state.put_instance(&inst).unwrap();

    let instances = state.list_instances_for_deployment("prod/api").unwrap();
    assert_eq!(instances[0].health, HealthStatus::Unhealthy);
    assert_eq!(instances[0].status, InstanceStatus::Unhealthy);
}

// ── Dead Node Detection ─────────────────────────────────────────

#[test]
fn dead_node_detection_and_reaping() {
    let state = test_store();
    let mgr = MembershipManager::new(state.clone())
        .with_dead_timeout(Duration::from_secs(0));

    let id = mgr
        .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
        .unwrap();

    // Set heartbeat to a very old timestamp to simulate death.
    let mut node = state.get_node(&id).unwrap().unwrap();
    node.last_heartbeat = 1000;
    state.put_node(&node).unwrap();

    // Should detect as dead and reap.
    let reaped = mgr.reap_dead_nodes().unwrap();
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0], id);

    // Node should be gone.
    assert_eq!(mgr.ready_count().unwrap(), 0);
}

// ── Placement Distribution ──────────────────────────────────────

#[test]
fn placement_distributes_across_nodes() {
    let state = test_store();

    // Set up 3 nodes with resources.
    make_node(&state, "node-1", "10.0.0.1", 8443);
    make_node(&state, "node-2", "10.0.0.2", 8443);
    make_node(&state, "node-3", "10.0.0.3", 8443);

    // Set up deployment.
    let spec = test_deployment("prod", "api");
    state.put_deployment(&spec).unwrap();

    // Convert nodes to placement types.
    let nodes = state.list_nodes().unwrap();
    let node_resources: Vec<_> = nodes
        .iter()
        .map(|n| convert::node_info_to_resources(n, false))
        .collect();

    // Convert deployment to requirements.
    let requirements = convert::deployment_to_requirements(&spec, 3);

    // Run placement.
    let weights = warpgrid_placement::ScoringWeights::default();
    let plan = warpgrid_placement::compute_placement(
        &requirements,
        &spec.id,
        &node_resources,
        &weights,
    );

    // Should place across available nodes.
    assert!(!plan.assignments.is_empty());
    assert_eq!(plan.deployment_id, "prod/api");
}

// ── Proxy Sync ──────────────────────────────────────────────────

#[test]
fn proxy_sync_reflects_state_store() {
    let state = test_store();

    let spec = test_deployment("prod", "api");
    state.put_deployment(&spec).unwrap();

    make_node(&state, "node-1", "10.0.0.1", 8443);

    let inst = make_instance("i1", "prod/api", "node-1", InstanceStatus::Running);
    state.put_instance(&inst).unwrap();

    let sync = ProxySync::new(Router::new(), DnsResolver::default());
    let stats = sync.sync(&state).unwrap();

    assert_eq!(stats.services_synced, 1);
    assert_eq!(stats.backends_total, 1);

    // Router has the service.
    let backends = sync.router().get_backends("prod/api");
    assert_eq!(backends.len(), 1);

    // DNS resolves.
    let record = sync.dns().resolve_service("api", "prod").unwrap();
    assert_eq!(record.addresses.len(), 1);
}

#[test]
fn proxy_sync_removes_undeployed_services() {
    let state = test_store();

    let spec = test_deployment("prod", "api");
    state.put_deployment(&spec).unwrap();

    let inst = make_instance("i1", "prod/api", "node-1", InstanceStatus::Running);
    state.put_instance(&inst).unwrap();

    let sync = ProxySync::new(Router::new(), DnsResolver::default());
    sync.sync(&state).unwrap();

    // Undeploy.
    state.delete_deployment("prod/api").unwrap();
    let stats = sync.sync(&state).unwrap();

    assert_eq!(stats.services_removed, 1);
    assert!(sync.router().get_backends("prod/api").is_empty());
}

// ── Raft Node Map ───────────────────────────────────────────────

#[test]
fn raft_node_map_consistent_ids() {
    let db = Arc::new(
        redb::Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .unwrap(),
    );
    let map = NodeIdMap::new(db);

    let id1 = map.get_or_insert("cp-1");
    let id2 = map.get_or_insert("cp-2");
    let id3 = map.get_or_insert("cp-3");

    // Deterministic — same input gives same output.
    assert_eq!(map.get_or_insert("cp-1"), id1);

    // All unique.
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);

    // Bidirectional lookup works.
    assert_eq!(map.get_node_id(id1), Some("cp-1".to_string()));
    assert_eq!(map.get_raft_id("cp-2"), Some(id2));
}

// ── Raft State Machine ─────────────────────────────────────────

#[tokio::test]
async fn raft_state_machine_apply_and_snapshot() {
    use openraft::{CommittedLeaderId, Entry, EntryPayload, LogId};
    use warpgrid_raft::{Request, Response, TypeConfig};

    let db = Arc::new(
        redb::Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .unwrap(),
    );
    let mut sm = StateMachine::new(db);

    // Apply a put.
    let entry = Entry::<TypeConfig> {
        log_id: LogId::new(CommittedLeaderId::new(1, 1), 1),
        payload: EntryPayload::Normal(Request::PutDeployment {
            key: "ns/app".to_string(),
            value: r#"{"name":"app"}"#.to_string(),
        }),
    };
    let responses: Vec<Response> =
        RaftStateMachine::apply(&mut sm, [entry]).await.unwrap();
    assert!(responses[0].success);

    // Build and restore snapshot.
    let mut builder: <StateMachine as RaftStateMachine<TypeConfig>>::SnapshotBuilder =
        RaftStateMachine::get_snapshot_builder(&mut sm).await;
    let snapshot = builder.build_snapshot().await.unwrap();
    assert_eq!(snapshot.meta.snapshot_id, "snap-1");
}

// ── Rollout Integration ─────────────────────────────────────────

#[test]
fn rollout_lifecycle() {
    use warpgrid_rollout::{Rollout, RolloutPhase, RolloutStrategy};

    let mut rollout = Rollout::new(
        "prod/api",
        RolloutStrategy::default(),
        3,
        "v1",
        "v2",
    );
    assert_eq!(rollout.phase, RolloutPhase::Pending);

    rollout.start();
    // Rolling strategy starts with batch 1.
    assert!(matches!(
        rollout.phase,
        RolloutPhase::RollingBatch { current: 1, .. }
    ));

    rollout.pause();
    assert_eq!(rollout.phase, RolloutPhase::Paused);

    rollout.resume();
    // Resume always restores to HealthGate.
    assert_eq!(rollout.phase, RolloutPhase::HealthGate);
}

// ── End-to-End: Deploy, Place, Sync ─────────────────────────────

#[test]
fn e2e_deploy_place_and_sync() {
    let state = test_store();

    // 1. Register 2 nodes via MembershipManager.
    let mgr = MembershipManager::new(state.clone());
    let id1 = mgr
        .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
        .unwrap();
    let id2 = mgr
        .join("10.0.0.2", 8443, HashMap::new(), 8_000_000_000, 1000)
        .unwrap();
    assert_eq!(mgr.ready_count().unwrap(), 2);

    // 2. Create a deployment.
    let spec = test_deployment("prod", "api");
    state.put_deployment(&spec).unwrap();

    // 3. Simulate instances on each node.
    let i1 = make_instance("i1", "prod/api", &id1, InstanceStatus::Running);
    let i2 = make_instance("i2", "prod/api", &id2, InstanceStatus::Running);
    state.put_instance(&i1).unwrap();
    state.put_instance(&i2).unwrap();

    // 4. Sync to proxy.
    let sync = ProxySync::new(Router::new(), DnsResolver::default());
    let stats = sync.sync(&state).unwrap();

    assert_eq!(stats.services_synced, 1);
    assert_eq!(stats.backends_total, 2);

    // 5. Router has both backends.
    let backends = sync.router().get_backends("prod/api");
    assert_eq!(backends.len(), 2);

    // 6. DNS resolves.
    let record = sync.dns().resolve_service("api", "prod").unwrap();
    assert_eq!(record.addresses.len(), 2);
}
