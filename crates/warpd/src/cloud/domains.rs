//! Custom domain management for cloud deployments.
//!
//! Allows users to map custom domains (e.g. `app.example.com`) to their
//! WarpGrid deployments. Each domain goes through a verification flow:
//! Pending -> Active (or Failed). Users receive CNAME instructions when
//! adding a domain.
//!
//! Supports two backends:
//! - In-memory (for tests and development without persistence)
//! - libSQL (for production — persists across restarts, edge-replicable)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

/// Status of a custom domain mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainStatus {
    Pending,
    Active,
    Failed,
}

impl fmt::Display for DomainStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Active => write!(f, "active"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// A mapping from a custom domain to a WarpGrid deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainMapping {
    pub domain: String,
    pub deployment_id: String,
    pub namespace: String,
    pub status: DomainStatus,
    pub created_at: u64,
    pub verified_at: Option<u64>,
}

/// DNS instructions returned when a domain is added.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsInstructions {
    pub record_type: String,
    pub name: String,
    pub target: String,
}

/// Response returned when adding a domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDomainResponse {
    pub mapping: DomainMapping,
    pub dns: DnsInstructions,
}

/// Errors from domain operations.
#[derive(Debug, Clone, Serialize)]
pub enum DomainError {
    InvalidDomain { reason: String },
    AlreadyExists { domain: String },
    NotFound { domain: String },
    VerificationFailed { domain: String },
    Storage(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDomain { reason } => write!(f, "invalid domain: {reason}"),
            Self::AlreadyExists { domain } => {
                write!(f, "domain already exists: {domain}")
            }
            Self::NotFound { domain } => write!(f, "domain not found: {domain}"),
            Self::VerificationFailed { domain } => {
                write!(f, "verification failed for domain: {domain}")
            }
            Self::Storage(msg) => write!(f, "storage error: {msg}"),
        }
    }
}

impl std::error::Error for DomainError {}

/// Validate that a domain string is a plausible custom domain.
///
/// Rules:
/// - Must contain at least one dot
/// - Must not contain protocol prefixes (http://, https://)
/// - Must not contain wildcards (*)
/// - Must not be empty or whitespace-only
/// - Must not contain spaces
/// - Must not contain path separators (/)
fn validate_domain(domain: &str) -> Result<(), DomainError> {
    let domain = domain.trim();

    if domain.is_empty() {
        return Err(DomainError::InvalidDomain {
            reason: "domain must not be empty".to_string(),
        });
    }

    if domain.contains("://") {
        return Err(DomainError::InvalidDomain {
            reason: "domain must not contain protocol prefix (e.g. http://)".to_string(),
        });
    }

    if domain.contains('*') {
        return Err(DomainError::InvalidDomain {
            reason: "wildcard domains are not supported".to_string(),
        });
    }

    if !domain.contains('.') {
        return Err(DomainError::InvalidDomain {
            reason: "domain must contain at least one dot".to_string(),
        });
    }

    if domain.contains(' ') {
        return Err(DomainError::InvalidDomain {
            reason: "domain must not contain spaces".to_string(),
        });
    }

    if domain.contains('/') {
        return Err(DomainError::InvalidDomain {
            reason: "domain must not contain path separators".to_string(),
        });
    }

    Ok(())
}

/// Generate the CNAME target for a deployment.
fn cname_target(deployment_id: &str) -> String {
    format!("{deployment_id}.edge.warpgrid.dev")
}

// ── Backend ─────────────────────────────────────────────────────

/// Domain store with pluggable backend (in-memory or libSQL).
#[derive(Clone)]
pub struct DomainStore {
    backend: DomainBackend,
}

#[derive(Clone)]
enum DomainBackend {
    Memory {
        domains: Arc<RwLock<HashMap<String, DomainMapping>>>,
    },
    LibSql {
        conn: libsql::Connection,
    },
}

impl DomainStore {
    /// Create an in-memory domain store (for tests and dev without persistence).
    pub fn new() -> Self {
        Self {
            backend: DomainBackend::Memory {
                domains: Arc::new(RwLock::new(HashMap::new())),
            },
        }
    }

    /// Create a persistent domain store backed by libSQL.
    pub fn with_libsql(conn: libsql::Connection) -> Self {
        Self {
            backend: DomainBackend::LibSql { conn },
        }
    }

    /// Add a custom domain mapping. Returns the mapping and DNS instructions.
    pub async fn add_domain(
        &self,
        domain: &str,
        deployment_id: &str,
        namespace: &str,
    ) -> Result<AddDomainResponse, DomainError> {
        let domain = domain.trim().to_lowercase();
        validate_domain(&domain)?;

        match &self.backend {
            DomainBackend::Memory { domains } => {
                let mut store = domains.write().unwrap();

                if store.contains_key(&domain) {
                    return Err(DomainError::AlreadyExists {
                        domain: domain.clone(),
                    });
                }

                let mapping = DomainMapping {
                    domain: domain.clone(),
                    deployment_id: deployment_id.to_string(),
                    namespace: namespace.to_string(),
                    status: DomainStatus::Pending,
                    created_at: epoch_secs(),
                    verified_at: None,
                };

                store.insert(domain.clone(), mapping.clone());

                let dns = DnsInstructions {
                    record_type: "CNAME".to_string(),
                    name: domain,
                    target: cname_target(deployment_id),
                };

                Ok(AddDomainResponse { mapping, dns })
            }
            DomainBackend::LibSql { conn } => {
                // Check if domain already exists.
                let existing = self.get_domain(&domain).await;
                if existing.is_some() {
                    return Err(DomainError::AlreadyExists {
                        domain: domain.clone(),
                    });
                }

                let now = epoch_secs();
                let mapping = DomainMapping {
                    domain: domain.clone(),
                    deployment_id: deployment_id.to_string(),
                    namespace: namespace.to_string(),
                    status: DomainStatus::Pending,
                    created_at: now,
                    verified_at: None,
                };

                conn.execute(
                    "INSERT INTO cloud_domains (domain, deployment_id, namespace, status, created_at, verified_at) VALUES (?, ?, ?, ?, ?, ?)",
                    libsql::params![
                        domain.clone(),
                        deployment_id.to_string(),
                        namespace.to_string(),
                        "pending".to_string(),
                        now as i64,
                        libsql::Value::Null
                    ],
                )
                .await
                .map_err(|e| DomainError::Storage(e.to_string()))?;

                let dns = DnsInstructions {
                    record_type: "CNAME".to_string(),
                    name: domain,
                    target: cname_target(deployment_id),
                };

                Ok(AddDomainResponse { mapping, dns })
            }
        }
    }

    /// Remove a custom domain mapping.
    pub async fn remove_domain(&self, domain: &str) -> Result<(), DomainError> {
        let domain = domain.trim().to_lowercase();

        match &self.backend {
            DomainBackend::Memory { domains } => {
                let mut store = domains.write().unwrap();

                if store.remove(&domain).is_none() {
                    return Err(DomainError::NotFound {
                        domain: domain.clone(),
                    });
                }

                Ok(())
            }
            DomainBackend::LibSql { conn } => {
                let affected = conn
                    .execute(
                        "DELETE FROM cloud_domains WHERE domain = ?",
                        libsql::params![domain.clone()],
                    )
                    .await
                    .map_err(|e| DomainError::Storage(e.to_string()))?;

                if affected == 0 {
                    return Err(DomainError::NotFound {
                        domain: domain.clone(),
                    });
                }

                Ok(())
            }
        }
    }

    /// List all domains for a given namespace.
    pub async fn list_domains_for_namespace(&self, namespace: &str) -> Vec<DomainMapping> {
        match &self.backend {
            DomainBackend::Memory { domains } => {
                let store = domains.read().unwrap();
                store
                    .values()
                    .filter(|m| m.namespace == namespace)
                    .cloned()
                    .collect()
            }
            DomainBackend::LibSql { conn } => {
                let mut rows = match conn
                    .query(
                        "SELECT domain, deployment_id, namespace, status, created_at, verified_at FROM cloud_domains WHERE namespace = ?",
                        libsql::params![namespace.to_string()],
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(_) => return Vec::new(),
                };

                let mut result = Vec::new();
                while let Ok(Some(row)) = rows.next().await {
                    if let Some(mapping) = row_to_mapping(&row) {
                        result.push(mapping);
                    }
                }

                result
            }
        }
    }

    /// Get a domain mapping by domain name.
    pub async fn get_domain(&self, domain: &str) -> Option<DomainMapping> {
        let domain = domain.trim().to_lowercase();

        match &self.backend {
            DomainBackend::Memory { domains } => {
                let store = domains.read().unwrap();
                store.get(&domain).cloned()
            }
            DomainBackend::LibSql { conn } => {
                let mut rows = conn
                    .query(
                        "SELECT domain, deployment_id, namespace, status, created_at, verified_at FROM cloud_domains WHERE domain = ?",
                        libsql::params![domain],
                    )
                    .await
                    .ok()?;

                let row = rows.next().await.ok()??;
                row_to_mapping(&row)
            }
        }
    }

    /// Mark a domain as verified (Active). Only pending domains can be verified.
    pub async fn verify_domain(&self, domain: &str) -> Result<DomainMapping, DomainError> {
        let domain = domain.trim().to_lowercase();

        match &self.backend {
            DomainBackend::Memory { domains } => {
                let mut store = domains.write().unwrap();

                let mapping =
                    store
                        .get_mut(&domain)
                        .ok_or_else(|| DomainError::NotFound {
                            domain: domain.clone(),
                        })?;

                if mapping.status != DomainStatus::Pending {
                    return Err(DomainError::VerificationFailed {
                        domain: domain.clone(),
                    });
                }

                mapping.status = DomainStatus::Active;
                mapping.verified_at = Some(epoch_secs());

                Ok(mapping.clone())
            }
            DomainBackend::LibSql { conn } => {
                let existing = self.get_domain(&domain).await.ok_or_else(|| {
                    DomainError::NotFound {
                        domain: domain.clone(),
                    }
                })?;

                if existing.status != DomainStatus::Pending {
                    return Err(DomainError::VerificationFailed {
                        domain: domain.clone(),
                    });
                }

                let now = epoch_secs();
                conn.execute(
                    "UPDATE cloud_domains SET status = ?, verified_at = ? WHERE domain = ?",
                    libsql::params!["active".to_string(), now as i64, domain.clone()],
                )
                .await
                .map_err(|e| DomainError::Storage(e.to_string()))?;

                self.get_domain(&domain).await.ok_or_else(|| {
                    DomainError::NotFound {
                        domain: domain.clone(),
                    }
                })
            }
        }
    }
}

/// Convert a libSQL row to a DomainMapping.
fn row_to_mapping(row: &libsql::Row) -> Option<DomainMapping> {
    let domain: String = row.get(0).ok()?;
    let deployment_id: String = row.get(1).ok()?;
    let namespace: String = row.get(2).ok()?;
    let status_str: String = row.get(3).ok()?;
    let created_at = row.get::<i64>(4).ok()? as u64;
    let verified_at: Option<u64> = row
        .get::<i64>(5)
        .ok()
        .map(|v| v as u64);

    let status = match status_str.as_str() {
        "active" => DomainStatus::Active,
        "failed" => DomainStatus::Failed,
        _ => DomainStatus::Pending,
    };

    Some(DomainMapping {
        domain,
        deployment_id,
        namespace,
        status,
        created_at,
        verified_at,
    })
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

    #[tokio::test]
    async fn add_and_remove_roundtrip() {
        let store = DomainStore::new();

        let resp = store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .await
            .unwrap();

        assert_eq!(resp.mapping.domain, "app.example.com");
        assert_eq!(resp.mapping.deployment_id, "dep_123");
        assert_eq!(resp.mapping.namespace, "ns_alice");
        assert_eq!(resp.mapping.status, DomainStatus::Pending);
        assert!(resp.mapping.verified_at.is_none());
        assert_eq!(resp.dns.record_type, "CNAME");
        assert_eq!(resp.dns.target, "dep_123.edge.warpgrid.dev");

        // Domain should be retrievable.
        assert!(store.get_domain("app.example.com").await.is_some());

        // Remove it.
        store.remove_domain("app.example.com").await.unwrap();

        // Should be gone.
        assert!(store.get_domain("app.example.com").await.is_none());
    }

    #[tokio::test]
    async fn duplicate_domain_rejected() {
        let store = DomainStore::new();

        store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .await
            .unwrap();

        let err = store
            .add_domain("app.example.com", "dep_456", "ns_bob")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::AlreadyExists { .. }));
    }

    #[tokio::test]
    async fn invalid_domain_no_dot() {
        let store = DomainStore::new();
        let err = store
            .add_domain("localhost", "dep_123", "ns_alice")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[tokio::test]
    async fn invalid_domain_with_protocol() {
        let store = DomainStore::new();
        let err = store
            .add_domain("https://app.example.com", "dep_123", "ns_alice")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[tokio::test]
    async fn invalid_domain_with_wildcard() {
        let store = DomainStore::new();
        let err = store
            .add_domain("*.example.com", "dep_123", "ns_alice")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[tokio::test]
    async fn invalid_domain_empty() {
        let store = DomainStore::new();
        let err = store.add_domain("", "dep_123", "ns_alice").await.unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[tokio::test]
    async fn invalid_domain_with_spaces() {
        let store = DomainStore::new();
        let err = store
            .add_domain("app .example.com", "dep_123", "ns_alice")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[tokio::test]
    async fn invalid_domain_with_path() {
        let store = DomainStore::new();
        let err = store
            .add_domain("app.example.com/path", "dep_123", "ns_alice")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[tokio::test]
    async fn list_filters_by_namespace() {
        let store = DomainStore::new();

        store
            .add_domain("alice.example.com", "dep_1", "ns_alice")
            .await
            .unwrap();
        store
            .add_domain("alice2.example.com", "dep_2", "ns_alice")
            .await
            .unwrap();
        store
            .add_domain("bob.example.com", "dep_3", "ns_bob")
            .await
            .unwrap();

        let alice_domains = store.list_domains_for_namespace("ns_alice").await;
        assert_eq!(alice_domains.len(), 2);
        assert!(alice_domains.iter().all(|d| d.namespace == "ns_alice"));

        let bob_domains = store.list_domains_for_namespace("ns_bob").await;
        assert_eq!(bob_domains.len(), 1);
        assert_eq!(bob_domains[0].domain, "bob.example.com");

        let empty = store.list_domains_for_namespace("ns_nobody").await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn verify_transitions_status_to_active() {
        let store = DomainStore::new();

        store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .await
            .unwrap();

        // Verify the domain.
        let verified = store.verify_domain("app.example.com").await.unwrap();
        assert_eq!(verified.status, DomainStatus::Active);
        assert!(verified.verified_at.is_some());

        // Confirm persisted.
        let fetched = store.get_domain("app.example.com").await.unwrap();
        assert_eq!(fetched.status, DomainStatus::Active);
    }

    #[tokio::test]
    async fn verify_already_active_domain_fails() {
        let store = DomainStore::new();

        store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .await
            .unwrap();
        store.verify_domain("app.example.com").await.unwrap();

        // Trying to verify again should fail.
        let err = store.verify_domain("app.example.com").await.unwrap_err();
        assert!(matches!(err, DomainError::VerificationFailed { .. }));
    }

    #[tokio::test]
    async fn verify_nonexistent_domain_fails() {
        let store = DomainStore::new();
        let err = store.verify_domain("ghost.example.com").await.unwrap_err();
        assert!(matches!(err, DomainError::NotFound { .. }));
    }

    #[tokio::test]
    async fn remove_nonexistent_domain_fails() {
        let store = DomainStore::new();
        let err = store.remove_domain("ghost.example.com").await.unwrap_err();
        assert!(matches!(err, DomainError::NotFound { .. }));
    }

    #[tokio::test]
    async fn domain_normalized_to_lowercase() {
        let store = DomainStore::new();

        store
            .add_domain("App.Example.COM", "dep_123", "ns_alice")
            .await
            .unwrap();

        // Should be retrievable with any case.
        assert!(store.get_domain("app.example.com").await.is_some());
        assert!(store.get_domain("APP.EXAMPLE.COM").await.is_some());
    }

    #[test]
    fn status_display_formats() {
        assert_eq!(DomainStatus::Pending.to_string(), "pending");
        assert_eq!(DomainStatus::Active.to_string(), "active");
        assert_eq!(DomainStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn error_display_formats() {
        let err = DomainError::InvalidDomain {
            reason: "bad".to_string(),
        };
        assert_eq!(err.to_string(), "invalid domain: bad");

        let err = DomainError::AlreadyExists {
            domain: "x.com".to_string(),
        };
        assert_eq!(err.to_string(), "domain already exists: x.com");

        let err = DomainError::NotFound {
            domain: "x.com".to_string(),
        };
        assert_eq!(err.to_string(), "domain not found: x.com");

        let err = DomainError::VerificationFailed {
            domain: "x.com".to_string(),
        };
        assert_eq!(err.to_string(), "verification failed for domain: x.com");
    }

    // ── libSQL backend tests ────────────────────────────────────

    #[tokio::test]
    async fn libsql_add_and_get_domain() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = DomainStore::with_libsql(conn);

        let resp = store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .await
            .unwrap();

        assert_eq!(resp.mapping.domain, "app.example.com");
        assert_eq!(resp.mapping.status, DomainStatus::Pending);
        assert_eq!(resp.dns.record_type, "CNAME");

        // Read it back.
        let fetched = store.get_domain("app.example.com").await.unwrap();
        assert_eq!(fetched.deployment_id, "dep_123");
        assert_eq!(fetched.namespace, "ns_alice");
        assert_eq!(fetched.status, DomainStatus::Pending);
    }

    #[tokio::test]
    async fn libsql_list_and_remove_domain() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = DomainStore::with_libsql(conn);

        store
            .add_domain("a.example.com", "dep_1", "ns_alice")
            .await
            .unwrap();
        store
            .add_domain("b.example.com", "dep_2", "ns_alice")
            .await
            .unwrap();

        let domains = store.list_domains_for_namespace("ns_alice").await;
        assert_eq!(domains.len(), 2);

        // Remove one.
        store.remove_domain("a.example.com").await.unwrap();

        let domains = store.list_domains_for_namespace("ns_alice").await;
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].domain, "b.example.com");
    }
}
