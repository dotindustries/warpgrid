//! Shared libSQL database for cloud metadata.
//!
//! All cloud state (users, teams, domains, billing) is stored in a single
//! libSQL database. In production, this syncs to Turso Cloud for edge
//! replication — edge agents get read replicas for local auth validation.
//!
//! ```text
//! warpd cloud (primary, R/W)
//!   └── cloud.db (libSQL file)
//!         ├── cloud_users
//!         ├── cloud_teams + cloud_team_members
//!         ├── cloud_domains
//!         └── cloud_billing
//!
//! warpd agent (replica, R/O)
//!   └── cloud-replica.db (synced from primary)
//!         └── reads for auth validation at edge
//! ```

use anyhow::Context;
use libsql::{Builder, Connection, Database};

/// Open a local libSQL database (primary mode, for warpd cloud).
pub async fn open_local(path: &str) -> anyhow::Result<Database> {
    let db = Builder::new_local(path)
        .build()
        .await
        .with_context(|| format!("failed to open libSQL database at {path}"))?;
    Ok(db)
}

/// Open a libSQL embedded replica that syncs from a Turso Cloud primary.
/// The local file serves reads instantly; writes go through the remote primary.
pub async fn open_replica(
    local_path: &str,
    turso_url: &str,
    auth_token: &str,
) -> anyhow::Result<Database> {
    let db = Builder::new_remote_replica(local_path, turso_url.to_string(), auth_token.to_string())
        .build()
        .await
        .with_context(|| {
            format!("failed to open libSQL replica at {local_path} syncing from {turso_url}")
        })?;
    Ok(db)
}

/// Open an in-memory libSQL database (for tests).
pub async fn open_memory() -> anyhow::Result<Database> {
    let db = Builder::new_local(":memory:")
        .build()
        .await
        .context("failed to open in-memory libSQL database")?;
    Ok(db)
}

/// Run all cloud schema migrations.
pub async fn migrate(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(MIGRATIONS)
        .await
        .context("failed to run cloud migrations")?;
    Ok(())
}

const MIGRATIONS: &str = "
-- Cloud Users
CREATE TABLE IF NOT EXISTS cloud_users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    namespace TEXT NOT NULL UNIQUE,
    api_key_hash TEXT NOT NULL UNIQUE,
    quota_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cloud_users_api_key ON cloud_users(api_key_hash);

-- Cloud Teams
CREATE TABLE IF NOT EXISTS cloud_teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cloud_team_members (
    team_id TEXT NOT NULL REFERENCES cloud_teams(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (team_id, user_id)
);

-- Cloud Domains
CREATE TABLE IF NOT EXISTS cloud_domains (
    domain TEXT PRIMARY KEY,
    deployment_id TEXT NOT NULL,
    namespace TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    verified_at INTEGER
);

-- Cloud Billing
CREATE TABLE IF NOT EXISTS cloud_billing (
    customer_id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL,
    plan TEXT NOT NULL DEFAULT 'free',
    created_at INTEGER NOT NULL
);

-- Cloud Deployments (replicated to edge agents via Turso sync)
CREATE TABLE IF NOT EXISTS cloud_deployments (
    id TEXT PRIMARY KEY,
    namespace TEXT NOT NULL,
    name TEXT NOT NULL,
    wasm_hash TEXT NOT NULL,
    region TEXT NOT NULL DEFAULT 'iad',
    status TEXT NOT NULL DEFAULT 'active',
    spec_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(namespace, name)
);
CREATE INDEX IF NOT EXISTS idx_cloud_deployments_ns ON cloud_deployments(namespace);
CREATE INDEX IF NOT EXISTS idx_cloud_deployments_region ON cloud_deployments(region);

-- Wasm Blobs (content-addressed, replicated to edge via Turso sync)
-- Edge agents read BLOBs from their local replica — zero network fetch.
CREATE TABLE IF NOT EXISTS cloud_wasm_blobs (
    hash TEXT PRIMARY KEY,
    wasm BLOB NOT NULL,
    size_bytes INTEGER NOT NULL,
    uploaded_at INTEGER NOT NULL
);
";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_memory_and_migrate() {
        let db = open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        migrate(&conn).await.unwrap();

        // Verify tables exist by inserting a test row.
        conn.execute(
            "INSERT INTO cloud_users (id, email, namespace, api_key_hash, created_at) VALUES (?, ?, ?, ?, ?)",
            libsql::params!["u1", "test@example.com", "test", "hash123", 1000],
        )
        .await
        .unwrap();

        let mut rows = conn
            .query("SELECT id, email FROM cloud_users", ())
            .await
            .unwrap();

        let row = rows.next().await.unwrap().unwrap();
        let id: String = row.get(0).unwrap();
        let email: String = row.get(1).unwrap();
        assert_eq!(id, "u1");
        assert_eq!(email, "test@example.com");
    }

    #[tokio::test]
    async fn migrate_is_idempotent() {
        let db = open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        migrate(&conn).await.unwrap();
        migrate(&conn).await.unwrap(); // second run should not fail
    }

    #[tokio::test]
    async fn all_tables_created() {
        let db = open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        migrate(&conn).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'cloud_%' ORDER BY name",
                (),
            )
            .await
            .unwrap();

        let mut tables = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let name: String = row.get(0).unwrap();
            tables.push(name);
        }

        assert!(tables.contains(&"cloud_users".to_string()));
        assert!(tables.contains(&"cloud_teams".to_string()));
        assert!(tables.contains(&"cloud_team_members".to_string()));
        assert!(tables.contains(&"cloud_domains".to_string()));
        assert!(tables.contains(&"cloud_billing".to_string()));
    }
}
