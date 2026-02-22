//! StateStore — redb-backed state persistence for WarpGrid.
//!
//! Provides typed CRUD operations over deployments, instances, nodes,
//! services, and metrics. All values are JSON-serialized into redb's
//! `&[u8]` value columns. The store supports both on-disk and in-memory
//! backends (the latter for testing).

use std::path::Path;
use std::sync::Arc;

use redb::{Database, ReadableDatabase, ReadableTable};
use tracing::debug;

use crate::error::{StateError, StateResult};
use crate::tables::*;
use crate::types::*;

/// Convert any `Display` error into a `StateError` variant via a closure factory.
macro_rules! map_err {
    ($variant:ident) => {
        |e| StateError::$variant(e.to_string())
    };
}

/// Thread-safe state store backed by redb.
#[derive(Clone)]
pub struct StateStore {
    db: Arc<Database>,
}

impl StateStore {
    /// Open (or create) a persistent state store at the given path.
    pub fn open(path: &Path) -> StateResult<Self> {
        let db = Database::create(path).map_err(map_err!(Open))?;
        let store = Self { db: Arc::new(db) };
        store.ensure_tables()?;
        debug!(?path, "state store opened");
        Ok(store)
    }

    /// Create an ephemeral in-memory state store (for testing).
    pub fn open_in_memory() -> StateResult<Self> {
        let backend = redb::backends::InMemoryBackend::new();
        let db = Database::builder()
            .create_with_backend(backend)
            .map_err(map_err!(Open))?;
        let store = Self { db: Arc::new(db) };
        store.ensure_tables()?;
        debug!("in-memory state store opened");
        Ok(store)
    }

    /// Create all tables if they don't exist yet.
    fn ensure_tables(&self) -> StateResult<()> {
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        // Opening a table in a write transaction creates it if absent.
        txn.open_table(DEPLOYMENTS).map_err(map_err!(Table))?;
        txn.open_table(INSTANCES).map_err(map_err!(Table))?;
        txn.open_table(NODES).map_err(map_err!(Table))?;
        txn.open_table(SERVICES).map_err(map_err!(Table))?;
        txn.open_table(METRICS).map_err(map_err!(Table))?;
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(())
    }

    // ── Deployments ────────────────────────────────────────────────

    /// Insert or update a deployment spec.
    pub fn put_deployment(&self, spec: &DeploymentSpec) -> StateResult<()> {
        let key = spec.table_key();
        let value = serde_json::to_vec(spec).map_err(map_err!(Serialize))?;
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        {
            let mut table = txn.open_table(DEPLOYMENTS).map_err(map_err!(Table))?;
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(map_err!(Write))?;
        }
        txn.commit().map_err(map_err!(Transaction))?;
        debug!(%key, "deployment stored");
        Ok(())
    }

    /// Get a deployment by namespace/name key.
    pub fn get_deployment(&self, key: &str) -> StateResult<Option<DeploymentSpec>> {
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(DEPLOYMENTS).map_err(map_err!(Table))?;
        match table.get(key).map_err(map_err!(Read))? {
            Some(guard) => {
                let spec: DeploymentSpec =
                    serde_json::from_slice(guard.value()).map_err(map_err!(Deserialize))?;
                Ok(Some(spec))
            }
            None => Ok(None),
        }
    }

    /// List all deployments.
    pub fn list_deployments(&self) -> StateResult<Vec<DeploymentSpec>> {
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(DEPLOYMENTS).map_err(map_err!(Table))?;
        let mut results = Vec::new();
        for entry in table.iter().map_err(map_err!(Read))? {
            let (_, value) = entry.map_err(map_err!(Read))?;
            let spec: DeploymentSpec =
                serde_json::from_slice(value.value()).map_err(map_err!(Deserialize))?;
            results.push(spec);
        }
        Ok(results)
    }

    /// Delete a deployment by key. Returns true if it existed.
    pub fn delete_deployment(&self, key: &str) -> StateResult<bool> {
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        let existed;
        {
            let mut table = txn.open_table(DEPLOYMENTS).map_err(map_err!(Table))?;
            existed = table.remove(key).map_err(map_err!(Write))?.is_some();
        }
        txn.commit().map_err(map_err!(Transaction))?;
        debug!(%key, existed, "deployment deleted");
        Ok(existed)
    }

    // ── Instances ──────────────────────────────────────────────────

    /// Insert or update an instance state.
    pub fn put_instance(&self, state: &InstanceState) -> StateResult<()> {
        let key = state.table_key();
        let value = serde_json::to_vec(state).map_err(map_err!(Serialize))?;
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        {
            let mut table = txn.open_table(INSTANCES).map_err(map_err!(Table))?;
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(map_err!(Write))?;
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(())
    }

    /// Get an instance by its composite key.
    pub fn get_instance(&self, key: &str) -> StateResult<Option<InstanceState>> {
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(INSTANCES).map_err(map_err!(Table))?;
        match table.get(key).map_err(map_err!(Read))? {
            Some(guard) => {
                let state: InstanceState =
                    serde_json::from_slice(guard.value()).map_err(map_err!(Deserialize))?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// List all instances for a given deployment ID.
    pub fn list_instances_for_deployment(
        &self,
        deployment_id: &str,
    ) -> StateResult<Vec<InstanceState>> {
        let prefix = format!("{deployment_id}:");
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(INSTANCES).map_err(map_err!(Table))?;
        let mut results = Vec::new();
        for entry in table.iter().map_err(map_err!(Read))? {
            let (key, value) = entry.map_err(map_err!(Read))?;
            if key.value().starts_with(&prefix) {
                let state: InstanceState =
                    serde_json::from_slice(value.value()).map_err(map_err!(Deserialize))?;
                results.push(state);
            }
        }
        Ok(results)
    }

    /// Delete an instance by key. Returns true if it existed.
    pub fn delete_instance(&self, key: &str) -> StateResult<bool> {
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        let existed;
        {
            let mut table = txn.open_table(INSTANCES).map_err(map_err!(Table))?;
            existed = table.remove(key).map_err(map_err!(Write))?.is_some();
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(existed)
    }

    /// Delete all instances for a deployment. Returns number deleted.
    pub fn delete_instances_for_deployment(&self, deployment_id: &str) -> StateResult<u32> {
        let prefix = format!("{deployment_id}:");
        // Collect keys in a read transaction first.
        let keys: Vec<String> = {
            let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
            let table = txn.open_table(INSTANCES).map_err(map_err!(Table))?;
            table
                .iter()
                .map_err(map_err!(Read))?
                .filter_map(|entry| {
                    let (key, _) = entry.ok()?;
                    let k = key.value().to_string();
                    k.starts_with(&prefix).then_some(k)
                })
                .collect()
        };
        // Delete in a write transaction.
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        let count = keys.len() as u32;
        {
            let mut table = txn.open_table(INSTANCES).map_err(map_err!(Table))?;
            for key in &keys {
                table.remove(key.as_str()).map_err(map_err!(Write))?;
            }
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(count)
    }

    // ── Nodes ──────────────────────────────────────────────────────

    /// Insert or update a node info.
    pub fn put_node(&self, node: &NodeInfo) -> StateResult<()> {
        let value = serde_json::to_vec(node).map_err(map_err!(Serialize))?;
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        {
            let mut table = txn.open_table(NODES).map_err(map_err!(Table))?;
            table
                .insert(node.id.as_str(), value.as_slice())
                .map_err(map_err!(Write))?;
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(())
    }

    /// Get a node by ID.
    pub fn get_node(&self, node_id: &str) -> StateResult<Option<NodeInfo>> {
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(NODES).map_err(map_err!(Table))?;
        match table.get(node_id).map_err(map_err!(Read))? {
            Some(guard) => {
                let node: NodeInfo =
                    serde_json::from_slice(guard.value()).map_err(map_err!(Deserialize))?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    /// List all nodes.
    pub fn list_nodes(&self) -> StateResult<Vec<NodeInfo>> {
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(NODES).map_err(map_err!(Table))?;
        let mut results = Vec::new();
        for entry in table.iter().map_err(map_err!(Read))? {
            let (_, value) = entry.map_err(map_err!(Read))?;
            let node: NodeInfo =
                serde_json::from_slice(value.value()).map_err(map_err!(Deserialize))?;
            results.push(node);
        }
        Ok(results)
    }

    /// Delete a node by ID. Returns true if it existed.
    pub fn delete_node(&self, node_id: &str) -> StateResult<bool> {
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        let existed;
        {
            let mut table = txn.open_table(NODES).map_err(map_err!(Table))?;
            existed = table.remove(node_id).map_err(map_err!(Write))?.is_some();
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(existed)
    }

    // ── Services ───────────────────────────────────────────────────

    /// Insert or update a service endpoint entry.
    pub fn put_service(&self, svc: &ServiceEndpoints) -> StateResult<()> {
        let key = svc.table_key();
        let value = serde_json::to_vec(svc).map_err(map_err!(Serialize))?;
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        {
            let mut table = txn.open_table(SERVICES).map_err(map_err!(Table))?;
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(map_err!(Write))?;
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(())
    }

    /// Get a service by namespace/name key.
    pub fn get_service(&self, key: &str) -> StateResult<Option<ServiceEndpoints>> {
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(SERVICES).map_err(map_err!(Table))?;
        match table.get(key).map_err(map_err!(Read))? {
            Some(guard) => {
                let svc: ServiceEndpoints =
                    serde_json::from_slice(guard.value()).map_err(map_err!(Deserialize))?;
                Ok(Some(svc))
            }
            None => Ok(None),
        }
    }

    // ── Metrics ────────────────────────────────────────────────────

    /// Insert a metrics snapshot.
    pub fn put_metrics(&self, snapshot: &MetricsSnapshot) -> StateResult<()> {
        let key = snapshot.table_key();
        let value = serde_json::to_vec(snapshot).map_err(map_err!(Serialize))?;
        let txn = self.db.begin_write().map_err(map_err!(Transaction))?;
        {
            let mut table = txn.open_table(METRICS).map_err(map_err!(Table))?;
            table
                .insert(key.as_str(), value.as_slice())
                .map_err(map_err!(Write))?;
        }
        txn.commit().map_err(map_err!(Transaction))?;
        Ok(())
    }

    /// Get recent metrics snapshots for a deployment (by key prefix scan).
    pub fn list_metrics_for_deployment(
        &self,
        deployment_id: &str,
        limit: usize,
    ) -> StateResult<Vec<MetricsSnapshot>> {
        let prefix = format!("{deployment_id}:");
        let txn = self.db.begin_read().map_err(map_err!(Transaction))?;
        let table = txn.open_table(METRICS).map_err(map_err!(Table))?;
        let mut results = Vec::new();
        for entry in table.iter().map_err(map_err!(Read))? {
            let (key, value) = entry.map_err(map_err!(Read))?;
            if key.value().starts_with(&prefix) {
                let snapshot: MetricsSnapshot =
                    serde_json::from_slice(value.value()).map_err(map_err!(Deserialize))?;
                results.push(snapshot);
                if results.len() >= limit {
                    break;
                }
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_deployment(namespace: &str, name: &str) -> DeploymentSpec {
        DeploymentSpec {
            id: format!("{namespace}-{name}"),
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
            health: Some(HealthConfig {
                endpoint: "/healthz".to_string(),
                interval: "5s".to_string(),
                timeout: "2s".to_string(),
                unhealthy_threshold: 3,
            }),
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
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
            memory_bytes: 32 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        }
    }

    fn test_node(id: &str) -> NodeInfo {
        NodeInfo {
            id: id.to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            capacity_memory_bytes: 8 * 1024 * 1024 * 1024,
            capacity_cpu_weight: 1000,
            used_memory_bytes: 0,
            used_cpu_weight: 0,
            labels: HashMap::new(),
            last_heartbeat: 1000,
        }
    }

    // ── Deployment CRUD ────────────────────────────────────────────

    #[test]
    fn deployment_put_and_get() {
        let store = StateStore::open_in_memory().unwrap();
        let spec = test_deployment("default", "my-api");

        store.put_deployment(&spec).unwrap();
        let retrieved = store.get_deployment("default/my-api").unwrap();

        assert_eq!(retrieved, Some(spec));
    }

    #[test]
    fn deployment_get_nonexistent_returns_none() {
        let store = StateStore::open_in_memory().unwrap();
        let result = store.get_deployment("nope/nothing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn deployment_list_all() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_deployment(&test_deployment("ns1", "a")).unwrap();
        store.put_deployment(&test_deployment("ns1", "b")).unwrap();
        store.put_deployment(&test_deployment("ns2", "c")).unwrap();

        let all = store.list_deployments().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn deployment_update_in_place() {
        let store = StateStore::open_in_memory().unwrap();
        let mut spec = test_deployment("default", "api");
        store.put_deployment(&spec).unwrap();

        spec.updated_at = 2000;
        spec.instances.max = 20;
        store.put_deployment(&spec).unwrap();

        let retrieved = store.get_deployment("default/api").unwrap().unwrap();
        assert_eq!(retrieved.instances.max, 20);
        assert_eq!(retrieved.updated_at, 2000);
    }

    #[test]
    fn deployment_delete() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_deployment(&test_deployment("default", "api")).unwrap();

        assert!(store.delete_deployment("default/api").unwrap());
        assert!(!store.delete_deployment("default/api").unwrap());
        assert!(store.get_deployment("default/api").unwrap().is_none());
    }

    // ── Instance CRUD ──────────────────────────────────────────────

    #[test]
    fn instance_put_and_get() {
        let store = StateStore::open_in_memory().unwrap();
        let inst = test_instance("deploy-1", 0);

        store.put_instance(&inst).unwrap();
        let retrieved = store.get_instance("deploy-1:inst-0").unwrap();

        assert_eq!(retrieved, Some(inst));
    }

    #[test]
    fn instance_list_for_deployment() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_instance(&test_instance("deploy-1", 0)).unwrap();
        store.put_instance(&test_instance("deploy-1", 1)).unwrap();
        store.put_instance(&test_instance("deploy-2", 0)).unwrap();

        let deploy1 = store.list_instances_for_deployment("deploy-1").unwrap();
        assert_eq!(deploy1.len(), 2);

        let deploy2 = store.list_instances_for_deployment("deploy-2").unwrap();
        assert_eq!(deploy2.len(), 1);
    }

    #[test]
    fn instance_delete_single() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_instance(&test_instance("deploy-1", 0)).unwrap();

        assert!(store.delete_instance("deploy-1:inst-0").unwrap());
        assert!(store.get_instance("deploy-1:inst-0").unwrap().is_none());
    }

    #[test]
    fn instance_delete_all_for_deployment() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_instance(&test_instance("deploy-1", 0)).unwrap();
        store.put_instance(&test_instance("deploy-1", 1)).unwrap();
        store.put_instance(&test_instance("deploy-2", 0)).unwrap();

        let deleted = store.delete_instances_for_deployment("deploy-1").unwrap();
        assert_eq!(deleted, 2);
        assert!(store.list_instances_for_deployment("deploy-1").unwrap().is_empty());
        // deploy-2 untouched
        assert_eq!(store.list_instances_for_deployment("deploy-2").unwrap().len(), 1);
    }

    // ── Node CRUD ──────────────────────────────────────────────────

    #[test]
    fn node_put_and_get() {
        let store = StateStore::open_in_memory().unwrap();
        let node = test_node("node-1");

        store.put_node(&node).unwrap();
        let retrieved = store.get_node("node-1").unwrap();

        assert_eq!(retrieved, Some(node));
    }

    #[test]
    fn node_list_all() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_node(&test_node("node-1")).unwrap();
        store.put_node(&test_node("node-2")).unwrap();

        let all = store.list_nodes().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn node_delete() {
        let store = StateStore::open_in_memory().unwrap();
        store.put_node(&test_node("node-1")).unwrap();

        assert!(store.delete_node("node-1").unwrap());
        assert!(store.get_node("node-1").unwrap().is_none());
    }

    // ── Service CRUD ───────────────────────────────────────────────

    #[test]
    fn service_put_and_get() {
        let store = StateStore::open_in_memory().unwrap();
        let svc = ServiceEndpoints {
            namespace: "default".to_string(),
            service: "my-api".to_string(),
            endpoints: vec!["10.0.0.1:8080".to_string(), "10.0.0.2:8080".to_string()],
            updated_at: 1000,
        };

        store.put_service(&svc).unwrap();
        let retrieved = store.get_service("default/my-api").unwrap();

        assert_eq!(retrieved, Some(svc));
    }

    // ── Metrics CRUD ───────────────────────────────────────────────

    #[test]
    fn metrics_put_and_list() {
        let store = StateStore::open_in_memory().unwrap();

        for epoch in [1000u64, 1060, 1120] {
            let snap = MetricsSnapshot {
                deployment_id: "deploy-1".to_string(),
                epoch,
                rps: 100.0,
                latency_p50_ms: 5.0,
                latency_p99_ms: 50.0,
                error_rate: 0.01,
                total_memory_bytes: 64 * 1024 * 1024,
                active_instances: 3,
            };
            store.put_metrics(&snap).unwrap();
        }

        let all = store.list_metrics_for_deployment("deploy-1", 10).unwrap();
        assert_eq!(all.len(), 3);

        let limited = store.list_metrics_for_deployment("deploy-1", 2).unwrap();
        assert_eq!(limited.len(), 2);
    }

    // ── Persistence (on-disk) ──────────────────────────────────────

    #[test]
    fn persistence_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.redb");

        {
            let store = StateStore::open(&db_path).unwrap();
            store.put_deployment(&test_deployment("prod", "api")).unwrap();
        }

        // Reopen the same database file.
        let store = StateStore::open(&db_path).unwrap();
        let spec = store.get_deployment("prod/api").unwrap();
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().name, "api");
    }

    // ── Edge cases ─────────────────────────────────────────────────

    #[test]
    fn empty_store_operations() {
        let store = StateStore::open_in_memory().unwrap();

        assert!(store.list_deployments().unwrap().is_empty());
        assert!(store.list_nodes().unwrap().is_empty());
        assert!(store.list_instances_for_deployment("any").unwrap().is_empty());
        assert!(store.list_metrics_for_deployment("any", 10).unwrap().is_empty());
        assert!(!store.delete_deployment("nope").unwrap());
        assert!(!store.delete_instance("nope").unwrap());
        assert!(!store.delete_node("nope").unwrap());
    }

    #[test]
    fn deployment_with_all_trigger_types() {
        let store = StateStore::open_in_memory().unwrap();

        let mut spec = test_deployment("ns", "http-svc");
        store.put_deployment(&spec).unwrap();

        spec.id = "ns-cron-job".to_string();
        spec.name = "cron-job".to_string();
        spec.trigger = TriggerConfig::Cron {
            schedule: "*/5 * * * *".to_string(),
        };
        store.put_deployment(&spec).unwrap();

        spec.id = "ns-queue-worker".to_string();
        spec.name = "queue-worker".to_string();
        spec.trigger = TriggerConfig::Queue {
            topic: "orders".to_string(),
        };
        store.put_deployment(&spec).unwrap();

        let all = store.list_deployments().unwrap();
        assert_eq!(all.len(), 3);
    }
}
