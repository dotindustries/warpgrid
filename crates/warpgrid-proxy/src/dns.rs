//! Internal DNS resolver for service discovery.
//!
//! Maps service names to their backend addresses within the mesh.
//! Supports namespace-scoped names: `{service}.{namespace}.svc.warpgrid`

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::debug;

/// A DNS record for internal service resolution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DnsRecord {
    /// Fully-qualified internal name.
    pub fqdn: String,
    /// Resolved addresses.
    pub addresses: Vec<String>,
    /// TTL in seconds.
    pub ttl: u32,
}

/// Internal DNS resolver for the service mesh.
pub struct DnsResolver {
    records: Arc<RwLock<HashMap<String, DnsRecord>>>,
    domain_suffix: String,
}

impl DnsResolver {
    /// Create a new resolver with the given domain suffix.
    pub fn new(domain_suffix: &str) -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            domain_suffix: domain_suffix.to_string(),
        }
    }

    /// Build a FQDN from service name and namespace.
    pub fn fqdn(&self, service: &str, namespace: &str) -> String {
        format!("{}.{}.svc.{}", service, namespace, self.domain_suffix)
    }

    /// Register or update a DNS record.
    pub fn upsert(&self, service: &str, namespace: &str, addresses: Vec<String>, ttl: u32) {
        let fqdn = self.fqdn(service, namespace);
        let record = DnsRecord {
            fqdn: fqdn.clone(),
            addresses,
            ttl,
        };
        let mut records = self.records.write().expect("dns lock");
        debug!(fqdn = %fqdn, "upserted DNS record");
        records.insert(fqdn, record);
    }

    /// Resolve a FQDN to addresses.
    pub fn resolve(&self, fqdn: &str) -> Option<DnsRecord> {
        let records = self.records.read().expect("dns lock");
        records.get(fqdn).cloned()
    }

    /// Resolve by service name + namespace.
    pub fn resolve_service(&self, service: &str, namespace: &str) -> Option<DnsRecord> {
        let fqdn = self.fqdn(service, namespace);
        self.resolve(&fqdn)
    }

    /// Remove a DNS record.
    pub fn remove(&self, service: &str, namespace: &str) {
        let fqdn = self.fqdn(service, namespace);
        let mut records = self.records.write().expect("dns lock");
        records.remove(&fqdn);
    }

    /// List all registered FQDNs.
    pub fn list_records(&self) -> Vec<DnsRecord> {
        let records = self.records.read().expect("dns lock");
        records.values().cloned().collect()
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new("warpgrid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fqdn_format() {
        let dns = DnsResolver::new("warpgrid");
        assert_eq!(
            dns.fqdn("api", "default"),
            "api.default.svc.warpgrid"
        );
    }

    #[test]
    fn upsert_and_resolve() {
        let dns = DnsResolver::new("warpgrid");
        dns.upsert("api", "prod", vec!["10.0.0.1".to_string()], 60);

        let record = dns.resolve_service("api", "prod").unwrap();
        assert_eq!(record.fqdn, "api.prod.svc.warpgrid");
        assert_eq!(record.addresses, vec!["10.0.0.1"]);
        assert_eq!(record.ttl, 60);
    }

    #[test]
    fn resolve_unknown_returns_none() {
        let dns = DnsResolver::new("warpgrid");
        assert!(dns.resolve_service("unknown", "ns").is_none());
    }

    #[test]
    fn remove_deletes_record() {
        let dns = DnsResolver::new("warpgrid");
        dns.upsert("api", "prod", vec!["10.0.0.1".to_string()], 60);
        dns.remove("api", "prod");
        assert!(dns.resolve_service("api", "prod").is_none());
    }

    #[test]
    fn update_overwrites() {
        let dns = DnsResolver::new("warpgrid");
        dns.upsert("api", "prod", vec!["10.0.0.1".to_string()], 60);
        dns.upsert(
            "api",
            "prod",
            vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()],
            30,
        );

        let record = dns.resolve_service("api", "prod").unwrap();
        assert_eq!(record.addresses.len(), 2);
        assert_eq!(record.ttl, 30);
    }
}
