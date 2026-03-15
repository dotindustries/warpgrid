//! Edge-to-Turso runtime state sync.
//!
//! Edge agents maintain hot-path state in local redb (every request),
//! then batch-sync snapshots to Turso on a configurable interval.
//! This gives the global dashboard at app.warpgrid.dev a cross-region
//! view without spamming Turso with per-request writes.
//!
//! ```text
//! HOT PATH (per request):
//!   request → redb: update latency, counter, health
//!
//! WARM PATH (every 30s):
//!   redb → read instances, metrics, node info
//!       → batch INSERT/REPLACE into Turso
//!       → Turso replicates to dashboard
//!
//! DASHBOARD (reads Turso):
//!   app.warpgrid.dev → SELECT * FROM cloud_instances
//!                    → cross-region, at most 30s stale
//! ```

use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Syncs local redb runtime state to the global Turso database.
pub struct RuntimeSync {
    region: String,
    state_store: warpgrid_state::StateStore,
    turso_conn: libsql::Connection,
}

impl RuntimeSync {
    pub fn new(
        region: &str,
        state_store: warpgrid_state::StateStore,
        turso_conn: libsql::Connection,
    ) -> Self {
        Self {
            region: region.to_string(),
            state_store,
            turso_conn,
        }
    }

    /// Run the sync loop at the given interval until shutdown.
    pub async fn run(&self, interval: Duration, mut shutdown: watch::Receiver<bool>) {
        info!(
            region = %self.region,
            interval_secs = interval.as_secs(),
            "runtime sync started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    if let Err(e) = self.sync_once().await {
                        warn!(error = %e, "runtime sync failed");
                    }
                }
                _ = shutdown.changed() => {
                    info!("runtime sync shutting down, final sync...");
                    let _ = self.sync_once().await;
                    break;
                }
            }
        }
    }

    /// Perform a single sync: read from redb, write to Turso.
    pub async fn sync_once(&self) -> anyhow::Result<()> {
        let mut synced = 0u32;

        // Sync instances.
        synced += self.sync_instances().await?;

        // Sync node info (heartbeat).
        synced += self.sync_nodes().await?;

        // Sync metrics snapshots.
        synced += self.sync_metrics().await?;

        debug!(region = %self.region, rows = synced, "runtime sync completed");
        Ok(())
    }

    /// Sync instance states from redb to Turso.
    async fn sync_instances(&self) -> anyhow::Result<u32> {
        let deployments = self.state_store.list_deployments().unwrap_or_default();
        let mut count = 0u32;

        for deployment in &deployments {
            let instances = self
                .state_store
                .list_instances_for_deployment(&deployment.id)
                .unwrap_or_default();

            for inst in &instances {
                self.turso_conn
                    .execute(
                        "INSERT OR REPLACE INTO cloud_instances \
                     (id, deployment_id, node_id, region, status, health, \
                      restart_count, memory_bytes, started_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        libsql::params![
                            inst.id.clone(),
                            inst.deployment_id.clone(),
                            inst.node_id.clone(),
                            self.region.clone(),
                            format!("{:?}", inst.status).to_lowercase(),
                            format!("{:?}", inst.health).to_lowercase(),
                            inst.restart_count as i64,
                            inst.memory_bytes as i64,
                            inst.started_at as i64,
                            inst.updated_at as i64
                        ],
                    )
                    .await?;
                count += 1;
            }
        }

        Ok(count)
    }

    /// Sync node info from redb to Turso.
    async fn sync_nodes(&self) -> anyhow::Result<u32> {
        let nodes = self.state_store.list_nodes().unwrap_or_default();
        let mut count = 0u32;

        for node in &nodes {
            let labels_json = serde_json::to_string(&node.labels).unwrap_or_default();
            self.turso_conn
                .execute(
                    "INSERT OR REPLACE INTO cloud_nodes \
                 (id, region, address, port, capacity_memory_bytes, capacity_cpu_weight, \
                  used_memory_bytes, used_cpu_weight, labels_json, last_heartbeat) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    libsql::params![
                        node.id.clone(),
                        self.region.clone(),
                        node.address.clone(),
                        node.port as i64,
                        node.capacity_memory_bytes as i64,
                        node.capacity_cpu_weight as i64,
                        node.used_memory_bytes as i64,
                        node.used_cpu_weight as i64,
                        labels_json,
                        node.last_heartbeat as i64
                    ],
                )
                .await?;
            count += 1;
        }

        Ok(count)
    }

    /// Sync latest metrics snapshots from redb to Turso.
    async fn sync_metrics(&self) -> anyhow::Result<u32> {
        let deployments = self.state_store.list_deployments().unwrap_or_default();
        let mut count = 0u32;

        for deployment in &deployments {
            let metrics = self
                .state_store
                .list_metrics_for_deployment(&deployment.id, 1)
                .unwrap_or_default();

            // Only sync the latest snapshot (not entire history).
            if let Some(latest) = metrics.last() {
                self.turso_conn
                    .execute(
                        "INSERT OR REPLACE INTO cloud_metrics \
                     (deployment_id, region, epoch, rps, latency_p50_ms, latency_p99_ms, \
                      error_rate, total_memory_bytes, active_instances) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        libsql::params![
                            latest.deployment_id.clone(),
                            self.region.clone(),
                            latest.epoch as i64,
                            latest.rps,
                            latest.latency_p50_ms,
                            latest.latency_p99_ms,
                            latest.error_rate,
                            latest.total_memory_bytes as i64,
                            latest.active_instances as i64
                        ],
                    )
                    .await?;
                count += 1;
            }
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn sync_instances_to_turso() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let state_store = warpgrid_state::StateStore::open_in_memory().unwrap();

        // Create a deployment and instance in redb.
        let spec = warpgrid_state::DeploymentSpec {
            id: "test/app".to_string(),
            namespace: "test".to_string(),
            name: "app".to_string(),
            source: "turso://hash123".to_string(),
            trigger: warpgrid_state::TriggerConfig::Http { port: Some(8080) },
            instances: warpgrid_state::InstanceConstraints { min: 1, max: 5 },
            resources: warpgrid_state::ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: warpgrid_state::ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        };
        state_store.put_deployment(&spec).unwrap();

        let instance = warpgrid_state::InstanceState {
            id: "inst-1".to_string(),
            deployment_id: "test/app".to_string(),
            node_id: "node-iad-1".to_string(),
            status: warpgrid_state::InstanceStatus::Running,
            health: warpgrid_state::HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 2 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1001,
        };
        state_store.put_instance(&instance).unwrap();

        // Sync to Turso.
        let sync = RuntimeSync::new("iad", state_store, conn.clone());
        sync.sync_once().await.unwrap();

        // Verify in Turso.
        let mut rows = conn
            .query("SELECT id, region, status, health FROM cloud_instances", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "inst-1");
        assert_eq!(row.get::<String>(1).unwrap(), "iad");
        assert_eq!(row.get::<String>(2).unwrap(), "running");
        assert_eq!(row.get::<String>(3).unwrap(), "healthy");
    }

    #[tokio::test]
    async fn sync_nodes_to_turso() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let state_store = warpgrid_state::StateStore::open_in_memory().unwrap();

        let node = warpgrid_state::NodeInfo {
            id: "node-iad-1".to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            capacity_memory_bytes: 8_000_000_000,
            capacity_cpu_weight: 1000,
            used_memory_bytes: 500_000_000,
            used_cpu_weight: 200,
            labels: HashMap::from([("region".to_string(), "iad".to_string())]),
            last_heartbeat: 1000,
        };
        state_store.put_node(&node).unwrap();

        let sync = RuntimeSync::new("iad", state_store, conn.clone());
        sync.sync_once().await.unwrap();

        let mut rows = conn
            .query(
                "SELECT id, region, capacity_memory_bytes FROM cloud_nodes",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "node-iad-1");
        assert_eq!(row.get::<String>(1).unwrap(), "iad");
        assert_eq!(row.get::<i64>(2).unwrap(), 8_000_000_000);
    }

    #[tokio::test]
    async fn sync_is_idempotent() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let state_store = warpgrid_state::StateStore::open_in_memory().unwrap();

        let node = warpgrid_state::NodeInfo {
            id: "node-1".to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            capacity_memory_bytes: 4_000_000_000,
            capacity_cpu_weight: 400,
            used_memory_bytes: 0,
            used_cpu_weight: 0,
            labels: HashMap::new(),
            last_heartbeat: 1000,
        };
        state_store.put_node(&node).unwrap();

        let sync = RuntimeSync::new("iad", state_store, conn.clone());
        sync.sync_once().await.unwrap();
        sync.sync_once().await.unwrap(); // second sync should not fail or duplicate

        let mut rows = conn
            .query("SELECT COUNT(*) FROM cloud_nodes", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1); // not 2
    }
}
