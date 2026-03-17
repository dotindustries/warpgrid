//! Agent token management for BYOC (bring-your-own-compute) deployments.
//!
//! Customers generate agent tokens in the console, then pass them to
//! `warpd agent --auth-token <token>` on their infrastructure. The agent
//! presents the token on cluster Join; the control plane validates it
//! and binds the node to the owning namespace for tenant-scoped placement.
//!
//! Token format: `wg_agent_<32 hex chars>`

use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// A registered agent token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToken {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub revoked: bool,
    pub created_at: u64,
    pub last_used_at: Option<u64>,
}

/// Result of validating an agent token.
#[derive(Debug, Clone)]
pub struct ValidatedAgent {
    pub token_id: String,
    pub namespace: String,
}

/// Store for agent tokens with pluggable backend.
#[derive(Clone)]
pub struct AgentTokenStore {
    backend: AgentTokenBackend,
}

#[derive(Clone)]
enum AgentTokenBackend {
    Memory {
        tokens: Arc<RwLock<HashMap<String, AgentToken>>>,
        hash_to_id: Arc<RwLock<HashMap<String, String>>>,
    },
    LibSql {
        conn: libsql::Connection,
    },
}

impl AgentTokenStore {
    /// Create an in-memory store (for tests).
    pub fn new() -> Self {
        Self {
            backend: AgentTokenBackend::Memory {
                tokens: Arc::new(RwLock::new(HashMap::new())),
                hash_to_id: Arc::new(RwLock::new(HashMap::new())),
            },
        }
    }

    /// Create a persistent store backed by libSQL.
    pub fn with_libsql(conn: libsql::Connection) -> Self {
        Self {
            backend: AgentTokenBackend::LibSql { conn },
        }
    }

    /// Issue a new agent token for a namespace. Returns the raw token (shown once).
    pub async fn issue(&self, namespace: &str, name: &str) -> anyhow::Result<(String, AgentToken)> {
        let raw_token = generate_agent_token();
        let token_hash = hash_token(&raw_token);
        let token_id = generate_token_id();
        let created_at = epoch_secs();

        let token = AgentToken {
            id: token_id.clone(),
            namespace: namespace.to_string(),
            name: name.to_string(),
            revoked: false,
            created_at,
            last_used_at: None,
        };

        match &self.backend {
            AgentTokenBackend::Memory {
                tokens,
                hash_to_id,
            } => {
                tokens.write().unwrap().insert(token_id.clone(), token.clone());
                hash_to_id.write().unwrap().insert(token_hash, token_id);
            }
            AgentTokenBackend::LibSql { conn } => {
                conn.execute(
                    "INSERT INTO cloud_agent_tokens (id, namespace, token_hash, name, created_at) VALUES (?, ?, ?, ?, ?)",
                    libsql::params![token.id.clone(), token.namespace.clone(), token_hash, token.name.clone(), created_at as i64],
                ).await?;
            }
        }

        Ok((raw_token, token))
    }

    /// Validate a raw agent token. Returns the namespace if valid and not revoked.
    pub async fn validate(&self, raw_token: &str) -> Option<ValidatedAgent> {
        let token_hash = hash_token(raw_token);

        match &self.backend {
            AgentTokenBackend::Memory {
                tokens,
                hash_to_id,
            } => {
                let id = hash_to_id.read().unwrap().get(&token_hash)?.clone();
                let token = tokens.read().unwrap().get(&id)?.clone();
                if token.revoked {
                    return None;
                }
                // Update last_used_at.
                if let Some(t) = tokens.write().unwrap().get_mut(&id) {
                    t.last_used_at = Some(epoch_secs());
                }
                Some(ValidatedAgent {
                    token_id: token.id,
                    namespace: token.namespace,
                })
            }
            AgentTokenBackend::LibSql { conn } => {
                let mut rows = conn
                    .query(
                        "SELECT id, namespace, revoked FROM cloud_agent_tokens WHERE token_hash = ?",
                        libsql::params![token_hash.clone()],
                    )
                    .await
                    .ok()?;

                let row = rows.next().await.ok()??;
                let id: String = row.get(0).ok()?;
                let namespace: String = row.get(1).ok()?;
                let revoked: i64 = row.get(2).ok()?;

                if revoked != 0 {
                    return None;
                }

                // Update last_used_at (best-effort).
                let _ = conn
                    .execute(
                        "UPDATE cloud_agent_tokens SET last_used_at = ? WHERE token_hash = ?",
                        libsql::params![epoch_secs() as i64, token_hash],
                    )
                    .await;

                Some(ValidatedAgent {
                    token_id: id,
                    namespace,
                })
            }
        }
    }

    /// List all tokens for a namespace.
    pub async fn list(&self, namespace: &str) -> anyhow::Result<Vec<AgentToken>> {
        match &self.backend {
            AgentTokenBackend::Memory { tokens, .. } => {
                let all = tokens.read().unwrap();
                Ok(all
                    .values()
                    .filter(|t| t.namespace == namespace)
                    .cloned()
                    .collect())
            }
            AgentTokenBackend::LibSql { conn } => {
                let mut rows = conn
                    .query(
                        "SELECT id, namespace, name, revoked, created_at, last_used_at FROM cloud_agent_tokens WHERE namespace = ? ORDER BY created_at DESC",
                        libsql::params![namespace.to_string()],
                    )
                    .await?;

                let mut result = Vec::new();
                while let Some(row) = rows.next().await? {
                    result.push(AgentToken {
                        id: row.get(0)?,
                        namespace: row.get(1)?,
                        name: row.get(2)?,
                        revoked: row.get::<i64>(3)? != 0,
                        created_at: row.get::<i64>(4)? as u64,
                        last_used_at: row.get::<Option<i64>>(5)?.map(|v| v as u64),
                    });
                }
                Ok(result)
            }
        }
    }

    /// Revoke a token by ID.
    pub async fn revoke(&self, namespace: &str, token_id: &str) -> anyhow::Result<bool> {
        match &self.backend {
            AgentTokenBackend::Memory { tokens, .. } => {
                let mut all = tokens.write().unwrap();
                if let Some(t) = all.get_mut(token_id) {
                    if t.namespace != namespace {
                        return Ok(false);
                    }
                    t.revoked = true;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            AgentTokenBackend::LibSql { conn } => {
                let affected = conn
                    .execute(
                        "UPDATE cloud_agent_tokens SET revoked = 1 WHERE id = ? AND namespace = ?",
                        libsql::params![token_id.to_string(), namespace.to_string()],
                    )
                    .await?;
                Ok(affected > 0)
            }
        }
    }
}

/// Generate a random agent token: `wg_agent_<32 hex chars>`.
fn generate_agent_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.r#gen();
    format!("wg_agent_{}", hex::encode(bytes))
}

/// Generate a random token ID.
fn generate_token_id() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.r#gen();
    format!("agt_{}", hex::encode(bytes))
}

/// Hash a token with SHA-256 for storage.
pub fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_and_validate_memory() {
        let store = AgentTokenStore::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let (raw, token) = rt.block_on(store.issue("acme", "prod-node-1")).unwrap();
        assert!(raw.starts_with("wg_agent_"));
        assert_eq!(raw.len(), 9 + 32); // "wg_agent_" + 32 hex
        assert_eq!(token.namespace, "acme");
        assert_eq!(token.name, "prod-node-1");

        let validated = rt.block_on(store.validate(&raw)).unwrap();
        assert_eq!(validated.namespace, "acme");
    }

    #[test]
    fn revoked_token_fails_validation() {
        let store = AgentTokenStore::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let (raw, token) = rt.block_on(store.issue("acme", "test")).unwrap();
        rt.block_on(store.revoke("acme", &token.id)).unwrap();

        assert!(rt.block_on(store.validate(&raw)).is_none());
    }

    #[test]
    fn invalid_token_returns_none() {
        let store = AgentTokenStore::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(store.validate("wg_agent_bogus")).is_none());
    }

    #[test]
    fn list_filters_by_namespace() {
        let store = AgentTokenStore::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(store.issue("acme", "node-1")).unwrap();
        rt.block_on(store.issue("acme", "node-2")).unwrap();
        rt.block_on(store.issue("other", "node-3")).unwrap();

        let acme = rt.block_on(store.list("acme")).unwrap();
        assert_eq!(acme.len(), 2);
    }

    #[test]
    fn revoke_wrong_namespace_fails() {
        let store = AgentTokenStore::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let (_, token) = rt.block_on(store.issue("acme", "test")).unwrap();
        let result = rt.block_on(store.revoke("other", &token.id)).unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn libsql_issue_and_validate() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = AgentTokenStore::with_libsql(conn);
        let (raw, token) = store.issue("acme", "prod-1").await.unwrap();

        assert!(raw.starts_with("wg_agent_"));
        assert_eq!(token.namespace, "acme");

        let validated = store.validate(&raw).await.unwrap();
        assert_eq!(validated.namespace, "acme");
    }

    #[tokio::test]
    async fn libsql_revoke_and_validate() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = AgentTokenStore::with_libsql(conn);
        let (raw, token) = store.issue("acme", "test").await.unwrap();

        store.revoke("acme", &token.id).await.unwrap();
        assert!(store.validate(&raw).await.is_none());
    }

    #[tokio::test]
    async fn libsql_list() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = AgentTokenStore::with_libsql(conn);
        store.issue("acme", "a").await.unwrap();
        store.issue("acme", "b").await.unwrap();
        store.issue("other", "c").await.unwrap();

        let acme = store.list("acme").await.unwrap();
        assert_eq!(acme.len(), 2);
    }
}
