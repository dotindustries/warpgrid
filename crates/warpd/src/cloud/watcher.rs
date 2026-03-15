//! Deployment watcher — monitors the Turso replica for new cloud deployments.
//!
//! Edge agents run a local Turso read replica that syncs from the cloud primary.
//! This module polls the replica for new `cloud_deployments` rows and loads the
//! corresponding Wasm blobs so the agent can instantiate them in warp-runtime.
//!
//! Two detection strategies are supported:
//!
//! 1. **Fallback polling** (primary) — queries `cloud_deployments` for rows not
//!    yet loaded by this agent, filtered by region. Reliable across all libSQL
//!    backends including in-memory databases.
//!
//! 2. **CDC-based detection** (optional optimization) — uses Turso's unstable
//!    `PRAGMA unstable_capture_data_changes_conn` to track inserts via the
//!    `turso_cdc` table. Only available on real Turso replicas, not in-memory.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use libsql::Connection;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// A newly detected deployment ready for runtime instantiation.
#[derive(Debug, Clone)]
pub struct NewDeployment {
    pub deployment_id: String,
    pub namespace: String,
    pub name: String,
    pub wasm_hash: String,
    pub wasm_bytes: Vec<u8>,
    pub region: String,
}

/// A deployment that was removed (status changed or row deleted).
#[derive(Debug, Clone)]
pub struct RemovedDeployment {
    pub deployment_id: String,
}

/// Watches the Turso replica for new cloud deployments and loads their Wasm blobs.
pub struct DeploymentWatcher {
    conn: Connection,
    /// Agent's local state store. Held for future use when the watcher feeds
    /// deployments directly into the local scheduler.
    #[allow(dead_code)]
    state: warpgrid_state::StateStore,
    region: String,
    loaded_ids: HashSet<String>,
    /// Last processed CDC change_id. `None` means CDC has not been initialized.
    last_change_id: Option<i64>,
    /// Whether CDC is available on this connection.
    cdc_available: bool,
}

impl DeploymentWatcher {
    /// Create a new watcher for the given region.
    ///
    /// The connection should point to a Turso read replica (or an in-memory
    /// database for testing). The `state` store is the agent's local redb.
    pub fn new(conn: Connection, state: warpgrid_state::StateStore, region: String) -> Self {
        Self {
            conn,
            state,
            region,
            loaded_ids: HashSet::new(),
            last_change_id: None,
            cdc_available: false,
        }
    }

    // ── CDC setup (unstable / optional) ─────────────────────────────
    //
    // Turso's CDC pragma is experimental and may not be available on all
    // replica types or libSQL versions. We attempt to enable it once and
    // fall back to polling if it fails.

    /// Attempt to enable CDC capture on this connection.
    /// Returns `true` if CDC was successfully enabled.
    async fn try_enable_cdc(&mut self) -> bool {
        let result = self
            .conn
            .execute("PRAGMA unstable_capture_data_changes_conn('after')", ())
            .await;

        match result {
            Ok(_) => {
                info!("CDC capture enabled on deployment watcher connection");
                self.cdc_available = true;
                self.last_change_id = Some(0);
                true
            }
            Err(e) => {
                debug!(error = %e, "CDC not available, using fallback polling");
                self.cdc_available = false;
                false
            }
        }
    }

    /// Poll for new deployments via CDC table.
    /// Returns new deployments found since `last_change_id`.
    async fn poll_cdc(&mut self) -> Result<Vec<NewDeployment>> {
        let last_id = self.last_change_id.unwrap_or(0);

        let mut rows = self
            .conn
            .query(
                "SELECT change_id, id, namespace, name, wasm_hash, region, status \
                 FROM cloud_deployments \
                 INNER JOIN ( \
                     SELECT change_id, json_extract(after, '$.id') AS cdc_id \
                     FROM turso_cdc \
                     WHERE table_name = 'cloud_deployments' \
                       AND change_type = 1 \
                       AND change_id > ? \
                     ORDER BY change_id ASC \
                 ) cdc ON cloud_deployments.id = cdc.cdc_id",
                libsql::params![last_id],
            )
            .await
            .context("CDC query failed")?;

        let mut deployments = Vec::new();
        let mut max_change_id = last_id;

        while let Ok(Some(row)) = rows.next().await {
            let change_id = row.get::<i64>(0).unwrap_or(0);
            let id = row.get::<String>(1).unwrap_or_default();
            let namespace = row.get::<String>(2).unwrap_or_default();
            let name = row.get::<String>(3).unwrap_or_default();
            let wasm_hash = row.get::<String>(4).unwrap_or_default();
            let region = row.get::<String>(5).unwrap_or_default();
            let status = row.get::<String>(6).unwrap_or_default();

            if change_id > max_change_id {
                max_change_id = change_id;
            }

            if region != self.region || status != "active" {
                continue;
            }

            if self.loaded_ids.contains(&id) {
                continue;
            }

            match self.load_wasm_blob(&wasm_hash).await {
                Ok(wasm_bytes) => {
                    self.loaded_ids.insert(id.clone());
                    deployments.push(NewDeployment {
                        deployment_id: id,
                        namespace,
                        name,
                        wasm_hash,
                        wasm_bytes,
                        region,
                    });
                }
                Err(e) => {
                    warn!(
                        deployment_id = %id,
                        wasm_hash = %wasm_hash,
                        error = %e,
                        "failed to load wasm blob for deployment, will retry next poll"
                    );
                }
            }
        }

        self.last_change_id = Some(max_change_id);
        Ok(deployments)
    }

    // ── Fallback polling (primary mechanism) ────────────────────────

    /// Poll for new deployments by querying `cloud_deployments` directly.
    /// Filters by region and skips already-loaded deployment IDs.
    async fn poll_fallback(&mut self) -> Result<Vec<NewDeployment>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, namespace, name, wasm_hash, region, status \
                 FROM cloud_deployments \
                 WHERE region = ? AND status = 'active'",
                libsql::params![self.region.clone()],
            )
            .await
            .context("fallback polling query failed")?;

        let mut deployments = Vec::new();

        while let Ok(Some(row)) = rows.next().await {
            let id = row.get::<String>(0).unwrap_or_default();
            let namespace = row.get::<String>(1).unwrap_or_default();
            let name = row.get::<String>(2).unwrap_or_default();
            let wasm_hash = row.get::<String>(3).unwrap_or_default();
            let region = row.get::<String>(4).unwrap_or_default();

            if self.loaded_ids.contains(&id) {
                continue;
            }

            match self.load_wasm_blob(&wasm_hash).await {
                Ok(wasm_bytes) => {
                    self.loaded_ids.insert(id.clone());
                    deployments.push(NewDeployment {
                        deployment_id: id,
                        namespace,
                        name,
                        wasm_hash,
                        wasm_bytes,
                        region,
                    });
                }
                Err(e) => {
                    warn!(
                        deployment_id = %id,
                        wasm_hash = %wasm_hash,
                        error = %e,
                        "failed to load wasm blob for deployment, will retry next poll"
                    );
                }
            }
        }

        Ok(deployments)
    }

    /// Detect deployments that were removed (deleted or deactivated).
    /// Returns IDs of deployments that were previously loaded but are no longer
    /// present as active in the database.
    async fn detect_removed(&mut self) -> Result<Vec<RemovedDeployment>> {
        if self.loaded_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Query all active deployment IDs for this region.
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM cloud_deployments \
                 WHERE region = ? AND status = 'active'",
                libsql::params![self.region.clone()],
            )
            .await
            .context("removal detection query failed")?;

        let mut active_ids = HashSet::new();
        while let Ok(Some(row)) = rows.next().await {
            let id = row.get::<String>(0).unwrap_or_default();
            active_ids.insert(id);
        }

        let removed: Vec<RemovedDeployment> = self
            .loaded_ids
            .iter()
            .filter(|id| !active_ids.contains(*id))
            .map(|id| RemovedDeployment {
                deployment_id: id.clone(),
            })
            .collect();

        // Remove from loaded set so we don't report them again.
        for r in &removed {
            self.loaded_ids.remove(&r.deployment_id);
        }

        Ok(removed)
    }

    // ── Wasm blob loading ───────────────────────────────────────────

    /// Load a Wasm blob from `cloud_wasm_blobs` by content hash.
    async fn load_wasm_blob(&self, hash: &str) -> Result<Vec<u8>> {
        let mut rows = self
            .conn
            .query(
                "SELECT wasm FROM cloud_wasm_blobs WHERE hash = ?",
                libsql::params![hash.to_string()],
            )
            .await
            .context("wasm blob query failed")?;

        let row = rows
            .next()
            .await
            .context("failed to read wasm blob row")?
            .ok_or_else(|| anyhow::anyhow!("wasm blob not found for hash {hash}"))?;

        let value = row
            .get::<libsql::Value>(0)
            .context("failed to read wasm column")?;

        let wasm_bytes = extract_blob(value)
            .ok_or_else(|| anyhow::anyhow!("wasm column is not a BLOB for hash {hash}"))?;

        Ok(wasm_bytes)
    }

    // ── Public API ──────────────────────────────────────────────────

    /// Check for new deployments once.
    ///
    /// Uses CDC if available, otherwise falls back to direct polling.
    /// Both paths are idempotent — calling `poll_once` multiple times will
    /// not return already-loaded deployments.
    pub async fn poll_once(&mut self) -> Result<Vec<NewDeployment>> {
        if self.cdc_available {
            match self.poll_cdc().await {
                Ok(deployments) => return Ok(deployments),
                Err(e) => {
                    warn!(error = %e, "CDC poll failed, falling back to direct query");
                    self.cdc_available = false;
                }
            }
        }

        self.poll_fallback().await
    }

    /// Check for removed deployments once.
    pub async fn poll_removed(&mut self) -> Result<Vec<RemovedDeployment>> {
        self.detect_removed().await
    }

    /// Run the deployment watcher loop.
    ///
    /// Polls at the given interval until the shutdown signal fires.
    /// New deployments are logged; the caller can extend this to feed
    /// them into warp-runtime instantiation.
    pub async fn run(&mut self, interval: Duration, mut shutdown: watch::Receiver<bool>) {
        info!(
            region = %self.region,
            interval_secs = interval.as_secs(),
            "deployment watcher starting"
        );

        // Attempt CDC setup once at startup.
        self.try_enable_cdc().await;

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    match self.poll_once().await {
                        Ok(new_deployments) => {
                            for dep in &new_deployments {
                                info!(
                                    deployment_id = %dep.deployment_id,
                                    namespace = %dep.namespace,
                                    name = %dep.name,
                                    wasm_hash = %dep.wasm_hash,
                                    wasm_size = dep.wasm_bytes.len(),
                                    "new deployment detected"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "deployment poll failed");
                        }
                    }

                    match self.poll_removed().await {
                        Ok(removed) => {
                            for dep in &removed {
                                info!(
                                    deployment_id = %dep.deployment_id,
                                    "deployment removed"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "removal detection failed");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("deployment watcher shutting down");
                    break;
                }
            }
        }
    }

    /// Return the set of currently loaded deployment IDs.
    pub fn loaded_deployment_ids(&self) -> &HashSet<String> {
        &self.loaded_ids
    }

    /// Borrow the underlying database connection.
    /// Useful for tests that need to insert or modify rows after constructing the watcher.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Extract raw bytes from a `libsql::Value::Blob`.
/// Returns `None` if the value is not a blob.
fn extract_blob(value: libsql::Value) -> Option<Vec<u8>> {
    match value {
        libsql::Value::Blob(bytes) => Some(bytes),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::db;

    /// Helper: create an in-memory database with cloud schema.
    async fn setup_test_db() -> (libsql::Database, Connection) {
        let database = db::open_memory().await.unwrap();
        let conn = database.connect().unwrap();
        db::migrate(&conn).await.unwrap();
        (database, conn)
    }

    /// Helper: insert a deployment row.
    async fn insert_deployment(
        conn: &Connection,
        id: &str,
        namespace: &str,
        name: &str,
        wasm_hash: &str,
        region: &str,
        status: &str,
    ) {
        conn.execute(
            "INSERT INTO cloud_deployments \
             (id, namespace, name, wasm_hash, region, status, spec_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, '{}', 1000, 1000)",
            libsql::params![
                id.to_string(),
                namespace.to_string(),
                name.to_string(),
                wasm_hash.to_string(),
                region.to_string(),
                status.to_string()
            ],
        )
        .await
        .unwrap();
    }

    /// Helper: insert a wasm blob row.
    async fn insert_wasm_blob(conn: &Connection, hash: &str, wasm: &[u8]) {
        conn.execute(
            "INSERT INTO cloud_wasm_blobs (hash, wasm, size_bytes, uploaded_at) \
             VALUES (?, ?, ?, 1000)",
            libsql::params![hash.to_string(), wasm.to_vec(), wasm.len() as i64],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn detects_new_deployment() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-abc", b"fake-wasm-module").await;
        insert_deployment(&conn, "dep-1", "ns1", "my-app", "hash-abc", "iad", "active").await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());
        let new = watcher.poll_once().await.unwrap();

        assert_eq!(new.len(), 1);
        assert_eq!(new[0].deployment_id, "dep-1");
        assert_eq!(new[0].namespace, "ns1");
        assert_eq!(new[0].name, "my-app");
        assert_eq!(new[0].wasm_hash, "hash-abc");
        assert_eq!(new[0].wasm_bytes, b"fake-wasm-module");
        assert_eq!(new[0].region, "iad");
    }

    #[tokio::test]
    async fn no_duplicates_on_second_poll() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-abc", b"fake-wasm").await;
        insert_deployment(&conn, "dep-1", "ns1", "app", "hash-abc", "iad", "active").await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());

        let first = watcher.poll_once().await.unwrap();
        assert_eq!(first.len(), 1);

        let second = watcher.poll_once().await.unwrap();
        assert_eq!(
            second.len(),
            0,
            "second poll should return no new deployments"
        );
    }

    #[tokio::test]
    async fn filters_by_region() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-1", b"wasm-1").await;
        insert_wasm_blob(&conn, "hash-2", b"wasm-2").await;

        // Deployment in our region.
        insert_deployment(
            &conn, "dep-iad", "ns1", "app-iad", "hash-1", "iad", "active",
        )
        .await;
        // Deployment in a different region.
        insert_deployment(
            &conn, "dep-sfo", "ns1", "app-sfo", "hash-2", "sfo", "active",
        )
        .await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());
        let new = watcher.poll_once().await.unwrap();

        assert_eq!(new.len(), 1);
        assert_eq!(new[0].deployment_id, "dep-iad");
    }

    #[tokio::test]
    async fn detects_removed_deployment() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-abc", b"fake-wasm").await;
        insert_deployment(&conn, "dep-1", "ns1", "app", "hash-abc", "iad", "active").await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());

        // First poll loads the deployment.
        let new = watcher.poll_once().await.unwrap();
        assert_eq!(new.len(), 1);

        // Delete the deployment via the watcher's connection.
        watcher
            .conn()
            .execute(
                "DELETE FROM cloud_deployments WHERE id = ?",
                libsql::params!["dep-1".to_string()],
            )
            .await
            .unwrap();

        // Detect removal.
        let removed = watcher.poll_removed().await.unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].deployment_id, "dep-1");

        // Should not be reported again.
        let removed_again = watcher.poll_removed().await.unwrap();
        assert_eq!(removed_again.len(), 0);
    }

    #[tokio::test]
    async fn skips_inactive_deployments() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-abc", b"fake-wasm").await;
        insert_deployment(&conn, "dep-1", "ns1", "app", "hash-abc", "iad", "paused").await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());
        let new = watcher.poll_once().await.unwrap();

        assert_eq!(new.len(), 0, "paused deployments should not be loaded");
    }

    #[tokio::test]
    async fn detects_new_after_initial_load() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-1", b"wasm-1").await;
        insert_deployment(&conn, "dep-1", "ns1", "app1", "hash-1", "iad", "active").await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());

        // Initial poll.
        let first = watcher.poll_once().await.unwrap();
        assert_eq!(first.len(), 1);

        // Add another deployment (simulates Turso sync delivering a new row).
        insert_wasm_blob(watcher.conn(), "hash-2", b"wasm-2").await;
        insert_deployment(
            watcher.conn(),
            "dep-2",
            "ns1",
            "app2",
            "hash-2",
            "iad",
            "active",
        )
        .await;

        // Second poll should pick up only the new one.
        let second = watcher.poll_once().await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].deployment_id, "dep-2");
    }

    #[tokio::test]
    async fn loaded_ids_tracks_state() {
        let (_db, conn) = setup_test_db().await;
        let state = warpgrid_state::StateStore::open_in_memory().unwrap();

        insert_wasm_blob(&conn, "hash-1", b"wasm-1").await;
        insert_wasm_blob(&conn, "hash-2", b"wasm-2").await;
        insert_deployment(&conn, "dep-1", "ns1", "app1", "hash-1", "iad", "active").await;
        insert_deployment(&conn, "dep-2", "ns1", "app2", "hash-2", "iad", "active").await;

        let mut watcher = DeploymentWatcher::new(conn, state, "iad".to_string());
        watcher.poll_once().await.unwrap();

        let ids = watcher.loaded_deployment_ids();
        assert!(ids.contains("dep-1"));
        assert!(ids.contains("dep-2"));
        assert_eq!(ids.len(), 2);
    }
}
