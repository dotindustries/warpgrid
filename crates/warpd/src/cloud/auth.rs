//! API key authentication for the cloud platform.
//!
//! Generates and validates API keys for users. Keys are SHA-256 hashed
//! before storage — the raw key is only shown once at creation time.
//!
//! Key format: `wg_live_<32 hex chars>` (e.g., `wg_live_a1b2c3d4...`)
//!
//! Supports two backends:
//! - In-memory (for tests and development without persistence)
//! - libSQL (for production — persists across restarts, edge-replicable)

use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// A registered user in the cloud platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub namespace: String,
    pub created_at: u64,
    pub quota: UserQuota,
}

/// Resource quotas per user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserQuota {
    pub max_deployments: u32,
    pub max_instances_per_deployment: u32,
    pub max_wasm_size_bytes: u64,
    pub max_memory_per_instance_bytes: u64,
    pub max_request_rate: u32,
}

impl Default for UserQuota {
    fn default() -> Self {
        Self {
            max_deployments: 3,
            max_instances_per_deployment: 5,
            max_wasm_size_bytes: 10 * 1024 * 1024, // 10 MB
            max_memory_per_instance_bytes: 256 * 1024 * 1024, // 256 MB
            max_request_rate: 100,
        }
    }
}

// ── AuthStore ───────────────────────────────────────────────────

/// Auth store with pluggable backend (in-memory or libSQL).
#[derive(Clone)]
pub struct AuthStore {
    backend: AuthBackend,
}

#[derive(Clone)]
enum AuthBackend {
    Memory {
        keys: Arc<RwLock<HashMap<String, User>>>,
        users: Arc<RwLock<HashMap<String, User>>>,
    },
    LibSql {
        conn: libsql::Connection,
    },
}

impl AuthStore {
    /// Create an in-memory auth store (for tests and dev without persistence).
    pub fn new() -> Self {
        Self {
            backend: AuthBackend::Memory {
                keys: Arc::new(RwLock::new(HashMap::new())),
                users: Arc::new(RwLock::new(HashMap::new())),
            },
        }
    }

    /// Create a persistent auth store backed by libSQL.
    /// Requires migrations to have been run on the connection.
    pub fn with_libsql(conn: libsql::Connection) -> Self {
        Self {
            backend: AuthBackend::LibSql { conn },
        }
    }

    /// Register a new user and return the raw API key (shown once).
    pub async fn register(&self, email: &str) -> anyhow::Result<(String, User)> {
        let raw_key = generate_api_key();
        let key_hash = hash_key(&raw_key);
        let user_id = generate_user_id();
        let namespace = email_to_namespace(email);
        let quota = UserQuota::default();
        let created_at = epoch_secs();

        let user = User {
            id: user_id.clone(),
            email: email.to_string(),
            namespace,
            created_at,
            quota: quota.clone(),
        };

        match &self.backend {
            AuthBackend::Memory { keys, users } => {
                keys.write().unwrap().insert(key_hash, user.clone());
                users.write().unwrap().insert(user_id, user.clone());
            }
            AuthBackend::LibSql { conn } => {
                let quota_json = serde_json::to_string(&quota)?;
                conn.execute(
                    "INSERT INTO cloud_users (id, email, namespace, api_key_hash, quota_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                    libsql::params![user.id.clone(), user.email.clone(), user.namespace.clone(), key_hash, quota_json, created_at as i64],
                ).await?;
            }
        }

        Ok((raw_key, user))
    }

    /// Validate an API key and return the associated user.
    pub async fn validate(&self, raw_key: &str) -> Option<User> {
        let key_hash = hash_key(raw_key);

        match &self.backend {
            AuthBackend::Memory { keys, .. } => {
                keys.read().unwrap().get(&key_hash).cloned()
            }
            AuthBackend::LibSql { conn } => {
                let mut rows = conn
                    .query(
                        "SELECT id, email, namespace, quota_json, created_at FROM cloud_users WHERE api_key_hash = ?",
                        libsql::params![key_hash],
                    )
                    .await
                    .ok()?;

                let row = rows.next().await.ok()??;
                let quota_json: String = row.get(3).ok()?;
                let quota: UserQuota = serde_json::from_str(&quota_json).unwrap_or_default();

                Some(User {
                    id: row.get(0).ok()?,
                    email: row.get(1).ok()?,
                    namespace: row.get(2).ok()?,
                    created_at: row.get::<i64>(4).ok()? as u64,
                    quota,
                })
            }
        }
    }

    /// Get a user by ID.
    pub async fn get_user(&self, user_id: &str) -> Option<User> {
        match &self.backend {
            AuthBackend::Memory { users, .. } => {
                users.read().unwrap().get(user_id).cloned()
            }
            AuthBackend::LibSql { conn } => {
                let mut rows = conn
                    .query(
                        "SELECT id, email, namespace, quota_json, created_at FROM cloud_users WHERE id = ?",
                        libsql::params![user_id.to_string()],
                    )
                    .await
                    .ok()?;

                let row = rows.next().await.ok()??;
                let quota_json: String = row.get(3).ok()?;
                let quota: UserQuota = serde_json::from_str(&quota_json).unwrap_or_default();

                Some(User {
                    id: row.get(0).ok()?,
                    email: row.get(1).ok()?,
                    namespace: row.get(2).ok()?,
                    created_at: row.get::<i64>(4).ok()? as u64,
                    quota,
                })
            }
        }
    }
}

// ── Sync wrappers for routes that aren't async yet ──────────────

impl AuthStore {
    /// Blocking register — for use in sync contexts.
    /// Panics if called outside a tokio runtime.
    pub fn register_sync(&self, email: &str) -> (String, User) {
        match &self.backend {
            AuthBackend::Memory { .. } => {
                // For memory backend, we can do it synchronously.
                let raw_key = generate_api_key();
                let key_hash = hash_key(&raw_key);
                let user_id = generate_user_id();
                let namespace = email_to_namespace(email);

                let user = User {
                    id: user_id.clone(),
                    email: email.to_string(),
                    namespace,
                    created_at: epoch_secs(),
                    quota: UserQuota::default(),
                };

                if let AuthBackend::Memory { keys, users } = &self.backend {
                    keys.write().unwrap().insert(key_hash, user.clone());
                    users.write().unwrap().insert(user_id, user.clone());
                }

                (raw_key, user)
            }
            AuthBackend::LibSql { .. } => {
                // For libSQL, use tokio::task::block_in_place.
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(self.register(email))
                        .expect("register failed")
                })
            }
        }
    }

    /// Blocking validate — for use in sync contexts.
    pub fn validate_sync(&self, raw_key: &str) -> Option<User> {
        match &self.backend {
            AuthBackend::Memory { keys, .. } => {
                let key_hash = hash_key(raw_key);
                keys.read().unwrap().get(&key_hash).cloned()
            }
            AuthBackend::LibSql { .. } => {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(self.validate(raw_key))
                })
            }
        }
    }
}

// ── Key generation helpers ──────────────────────────────────────

/// Generate a random API key in the format `wg_live_<32 hex chars>`.
fn generate_api_key() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.r#gen();
    format!("wg_live_{}", hex::encode(bytes))
}

/// Generate a random user ID.
fn generate_user_id() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.r#gen();
    format!("usr_{}", hex::encode(bytes))
}

/// Hash an API key with SHA-256 for storage.
pub fn hash_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Convert email to a valid namespace (lowercase, alphanumeric + hyphens).
fn email_to_namespace(email: &str) -> String {
    let local = email.split('@').next().unwrap_or("user");
    local
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
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

    // ── In-memory backend tests (existing) ──────────────────────

    #[test]
    fn register_and_validate_roundtrip() {
        let store = AuthStore::new();
        let (key, user) = store.register_sync("alice@example.com");

        assert!(key.starts_with("wg_live_"));
        assert_eq!(key.len(), 8 + 32); // "wg_live_" + 32 hex chars
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.namespace, "alice");

        let validated = store.validate_sync(&key).unwrap();
        assert_eq!(validated.id, user.id);
    }

    #[test]
    fn invalid_key_returns_none() {
        let store = AuthStore::new();
        store.register_sync("bob@test.com");
        assert!(store.validate_sync("wg_live_invalid_key_here_000000").is_none());
    }

    #[test]
    fn email_to_namespace_handles_special_chars() {
        assert_eq!(email_to_namespace("john.doe@test.com"), "john-doe");
        assert_eq!(email_to_namespace("user+tag@test.com"), "user-tag");
        assert_eq!(email_to_namespace("UPPER@test.com"), "upper");
    }

    #[test]
    fn default_quota_matches_beta_constraints() {
        let q = UserQuota::default();
        assert_eq!(q.max_deployments, 3);
        assert_eq!(q.max_instances_per_deployment, 5);
        assert_eq!(q.max_wasm_size_bytes, 10 * 1024 * 1024);
        assert_eq!(q.max_request_rate, 100);
    }

    // ── libSQL backend tests ────────────────────────────────────

    #[tokio::test]
    async fn libsql_register_and_validate() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = AuthStore::with_libsql(conn);
        let (key, user) = store.register("alice@example.com").await.unwrap();

        assert!(key.starts_with("wg_live_"));
        assert_eq!(user.email, "alice@example.com");

        let validated = store.validate(&key).await.unwrap();
        assert_eq!(validated.id, user.id);
        assert_eq!(validated.namespace, "alice");
    }

    #[tokio::test]
    async fn libsql_invalid_key_returns_none() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = AuthStore::with_libsql(conn);
        store.register("bob@test.com").await.unwrap();
        assert!(store.validate("wg_live_wrong_key_0000000000000").await.is_none());
    }

    #[tokio::test]
    async fn libsql_get_user_by_id() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = AuthStore::with_libsql(conn);
        let (_, user) = store.register("charlie@test.com").await.unwrap();

        let fetched = store.get_user(&user.id).await.unwrap();
        assert_eq!(fetched.email, "charlie@test.com");
    }

    #[tokio::test]
    async fn libsql_persists_across_stores() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        // Register on store 1.
        let store1 = AuthStore::with_libsql(conn.clone());
        let (key, _) = store1.register("persist@test.com").await.unwrap();

        // Validate on store 2 (same connection, simulating process restart
        // against the same database file).
        let store2 = AuthStore::with_libsql(conn);
        let validated = store2.validate(&key).await.unwrap();
        assert_eq!(validated.email, "persist@test.com");
    }
}
