//! API key authentication for the cloud platform.
//!
//! Generates and validates API keys for users. Keys are SHA-256 hashed
//! before storage — the raw key is only shown once at creation time.
//!
//! Key format: `wg_live_<32 hex chars>` (e.g., `wg_live_a1b2c3d4...`)

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

/// In-memory auth store. Will be replaced with Postgres in production.
#[derive(Clone)]
pub struct AuthStore {
    /// Maps API key hash → user
    keys: Arc<RwLock<HashMap<String, User>>>,
    /// Maps user ID → API key hash (for lookup)
    users: Arc<RwLock<HashMap<String, User>>>,
}

impl AuthStore {
    pub fn new() -> Self {
        Self {
            keys: Arc::new(RwLock::new(HashMap::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new user and return the raw API key (shown once).
    pub fn register(&self, email: &str) -> (String, User) {
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

        {
            let mut keys = self.keys.write().unwrap();
            keys.insert(key_hash, user.clone());
        }
        {
            let mut users = self.users.write().unwrap();
            users.insert(user_id, user.clone());
        }

        (raw_key, user)
    }

    /// Validate an API key and return the associated user.
    pub fn validate(&self, raw_key: &str) -> Option<User> {
        let key_hash = hash_key(raw_key);
        let keys = self.keys.read().unwrap();
        keys.get(&key_hash).cloned()
    }

    /// Get a user by ID.
    pub fn get_user(&self, user_id: &str) -> Option<User> {
        let users = self.users.read().unwrap();
        users.get(user_id).cloned()
    }
}

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
fn hash_key(raw_key: &str) -> String {
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

    #[test]
    fn register_and_validate_roundtrip() {
        let store = AuthStore::new();
        let (key, user) = store.register("alice@example.com");

        assert!(key.starts_with("wg_live_"));
        assert_eq!(key.len(), 8 + 32); // "wg_live_" + 32 hex chars
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.namespace, "alice");

        let validated = store.validate(&key).unwrap();
        assert_eq!(validated.id, user.id);
    }

    #[test]
    fn invalid_key_returns_none() {
        let store = AuthStore::new();
        store.register("bob@test.com");
        assert!(store.validate("wg_live_invalid_key_here_000000").is_none());
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
}
