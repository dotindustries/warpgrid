//! Custom domain management for cloud deployments.
//!
//! Allows users to map custom domains (e.g. `app.example.com`) to their
//! WarpGrid deployments. Each domain goes through a verification flow:
//! Pending -> Active (or Failed). Users receive CNAME instructions when
//! adding a domain.

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

/// In-memory domain store. Will be replaced with Postgres in production.
#[derive(Clone)]
pub struct DomainStore {
    domains: Arc<RwLock<HashMap<String, DomainMapping>>>,
}

impl DomainStore {
    pub fn new() -> Self {
        Self {
            domains: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a custom domain mapping. Returns the mapping and DNS instructions.
    pub fn add_domain(
        &self,
        domain: &str,
        deployment_id: &str,
        namespace: &str,
    ) -> Result<AddDomainResponse, DomainError> {
        let domain = domain.trim().to_lowercase();
        validate_domain(&domain)?;

        let mut domains = self.domains.write().unwrap();

        if domains.contains_key(&domain) {
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

        domains.insert(domain.clone(), mapping.clone());

        let dns = DnsInstructions {
            record_type: "CNAME".to_string(),
            name: domain,
            target: cname_target(deployment_id),
        };

        Ok(AddDomainResponse { mapping, dns })
    }

    /// Remove a custom domain mapping.
    pub fn remove_domain(&self, domain: &str) -> Result<(), DomainError> {
        let domain = domain.trim().to_lowercase();
        let mut domains = self.domains.write().unwrap();

        if domains.remove(&domain).is_none() {
            return Err(DomainError::NotFound {
                domain: domain.clone(),
            });
        }

        Ok(())
    }

    /// List all domains for a given namespace.
    pub fn list_domains_for_namespace(&self, namespace: &str) -> Vec<DomainMapping> {
        let domains = self.domains.read().unwrap();
        domains
            .values()
            .filter(|m| m.namespace == namespace)
            .cloned()
            .collect()
    }

    /// Get a domain mapping by domain name.
    pub fn get_domain(&self, domain: &str) -> Option<DomainMapping> {
        let domain = domain.trim().to_lowercase();
        let domains = self.domains.read().unwrap();
        domains.get(&domain).cloned()
    }

    /// Mark a domain as verified (Active). Only pending domains can be verified.
    pub fn verify_domain(&self, domain: &str) -> Result<DomainMapping, DomainError> {
        let domain = domain.trim().to_lowercase();
        let mut domains = self.domains.write().unwrap();

        let mapping = domains.get_mut(&domain).ok_or_else(|| DomainError::NotFound {
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
    fn add_and_remove_roundtrip() {
        let store = DomainStore::new();

        let resp = store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .unwrap();

        assert_eq!(resp.mapping.domain, "app.example.com");
        assert_eq!(resp.mapping.deployment_id, "dep_123");
        assert_eq!(resp.mapping.namespace, "ns_alice");
        assert_eq!(resp.mapping.status, DomainStatus::Pending);
        assert!(resp.mapping.verified_at.is_none());
        assert_eq!(resp.dns.record_type, "CNAME");
        assert_eq!(resp.dns.target, "dep_123.edge.warpgrid.dev");

        // Domain should be retrievable.
        assert!(store.get_domain("app.example.com").is_some());

        // Remove it.
        store.remove_domain("app.example.com").unwrap();

        // Should be gone.
        assert!(store.get_domain("app.example.com").is_none());
    }

    #[test]
    fn duplicate_domain_rejected() {
        let store = DomainStore::new();

        store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .unwrap();

        let err = store
            .add_domain("app.example.com", "dep_456", "ns_bob")
            .unwrap_err();
        assert!(matches!(err, DomainError::AlreadyExists { .. }));
    }

    #[test]
    fn invalid_domain_no_dot() {
        let store = DomainStore::new();
        let err = store
            .add_domain("localhost", "dep_123", "ns_alice")
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[test]
    fn invalid_domain_with_protocol() {
        let store = DomainStore::new();
        let err = store
            .add_domain("https://app.example.com", "dep_123", "ns_alice")
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[test]
    fn invalid_domain_with_wildcard() {
        let store = DomainStore::new();
        let err = store
            .add_domain("*.example.com", "dep_123", "ns_alice")
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[test]
    fn invalid_domain_empty() {
        let store = DomainStore::new();
        let err = store.add_domain("", "dep_123", "ns_alice").unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[test]
    fn invalid_domain_with_spaces() {
        let store = DomainStore::new();
        let err = store
            .add_domain("app .example.com", "dep_123", "ns_alice")
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[test]
    fn invalid_domain_with_path() {
        let store = DomainStore::new();
        let err = store
            .add_domain("app.example.com/path", "dep_123", "ns_alice")
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidDomain { .. }));
    }

    #[test]
    fn list_filters_by_namespace() {
        let store = DomainStore::new();

        store
            .add_domain("alice.example.com", "dep_1", "ns_alice")
            .unwrap();
        store
            .add_domain("alice2.example.com", "dep_2", "ns_alice")
            .unwrap();
        store
            .add_domain("bob.example.com", "dep_3", "ns_bob")
            .unwrap();

        let alice_domains = store.list_domains_for_namespace("ns_alice");
        assert_eq!(alice_domains.len(), 2);
        assert!(alice_domains.iter().all(|d| d.namespace == "ns_alice"));

        let bob_domains = store.list_domains_for_namespace("ns_bob");
        assert_eq!(bob_domains.len(), 1);
        assert_eq!(bob_domains[0].domain, "bob.example.com");

        let empty = store.list_domains_for_namespace("ns_nobody");
        assert!(empty.is_empty());
    }

    #[test]
    fn verify_transitions_status_to_active() {
        let store = DomainStore::new();

        store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .unwrap();

        // Verify the domain.
        let verified = store.verify_domain("app.example.com").unwrap();
        assert_eq!(verified.status, DomainStatus::Active);
        assert!(verified.verified_at.is_some());

        // Confirm persisted.
        let fetched = store.get_domain("app.example.com").unwrap();
        assert_eq!(fetched.status, DomainStatus::Active);
    }

    #[test]
    fn verify_already_active_domain_fails() {
        let store = DomainStore::new();

        store
            .add_domain("app.example.com", "dep_123", "ns_alice")
            .unwrap();
        store.verify_domain("app.example.com").unwrap();

        // Trying to verify again should fail.
        let err = store.verify_domain("app.example.com").unwrap_err();
        assert!(matches!(err, DomainError::VerificationFailed { .. }));
    }

    #[test]
    fn verify_nonexistent_domain_fails() {
        let store = DomainStore::new();
        let err = store.verify_domain("ghost.example.com").unwrap_err();
        assert!(matches!(err, DomainError::NotFound { .. }));
    }

    #[test]
    fn remove_nonexistent_domain_fails() {
        let store = DomainStore::new();
        let err = store.remove_domain("ghost.example.com").unwrap_err();
        assert!(matches!(err, DomainError::NotFound { .. }));
    }

    #[test]
    fn domain_normalized_to_lowercase() {
        let store = DomainStore::new();

        store
            .add_domain("App.Example.COM", "dep_123", "ns_alice")
            .unwrap();

        // Should be retrievable with any case.
        assert!(store.get_domain("app.example.com").is_some());
        assert!(store.get_domain("APP.EXAMPLE.COM").is_some());
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
}
