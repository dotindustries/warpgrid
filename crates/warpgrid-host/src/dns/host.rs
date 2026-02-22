//! DNS resolution host functions.
//!
//! Implements the `warpgrid:shim/dns` [`Host`] trait, delegating hostname
//! resolution to the [`DnsResolver`] chain (service registry → `/etc/hosts`
//! → system DNS).
//!
//! # Resolution flow
//!
//! ```text
//! Guest calls resolve_address("db.production.warp.local")
//!   → DnsHost delegates to DnsResolver::resolve()
//!     → Chain: registry → /etc/hosts → system DNS
//!     → First match wins → Ok(list<ip-address-record>)
//!     → No match         → Err("HostNotFound: ...")
//! ```

use std::sync::Arc;

use crate::bindings::warpgrid::shim::dns::{Host, IpAddressRecord};
use super::DnsResolver;

/// Host-side implementation of the `warpgrid:shim/dns` interface.
///
/// Wraps a [`DnsResolver`] and converts between Rust `IpAddr` types and
/// the WIT `ip-address-record` type.
pub struct DnsHost {
    /// The DNS resolver implementing the three-tier resolution chain.
    resolver: Arc<DnsResolver>,
    /// Tokio runtime handle for running async resolution from sync context.
    runtime_handle: tokio::runtime::Handle,
}

impl DnsHost {
    /// Create a new `DnsHost` wrapping the given resolver.
    ///
    /// The `runtime_handle` is used to block on the async resolver from
    /// the synchronous `Host` trait methods.
    pub fn new(resolver: Arc<DnsResolver>, runtime_handle: tokio::runtime::Handle) -> Self {
        Self {
            resolver,
            runtime_handle,
        }
    }
}

impl Host for DnsHost {
    fn resolve_address(
        &mut self,
        hostname: String,
    ) -> Result<Vec<IpAddressRecord>, String> {
        tracing::debug!(hostname = %hostname, "dns intercept: resolve_address");

        let resolver = Arc::clone(&self.resolver);
        let hostname_clone = hostname.clone();

        let handle = self.runtime_handle.clone();
        let result = tokio::task::block_in_place(|| {
            handle.block_on(resolver.resolve(&hostname_clone))
        });

        match result {
            Ok(addrs) => {
                let records: Vec<IpAddressRecord> = addrs
                    .into_iter()
                    .map(|ip| IpAddressRecord {
                        address: ip.to_string(),
                        is_ipv6: ip.is_ipv6(),
                    })
                    .collect();

                tracing::debug!(
                    hostname = %hostname,
                    count = records.len(),
                    "dns resolve_address succeeded"
                );
                Ok(records)
            }
            Err(e) => {
                tracing::debug!(
                    hostname = %hostname,
                    error = %e,
                    "dns resolve_address failed"
                );
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    /// Create a `DnsHost` with the given registry and hosts content.
    fn make_host(
        registry: HashMap<String, Vec<IpAddr>>,
        hosts_content: &str,
    ) -> DnsHost {
        let resolver = Arc::new(DnsResolver::new(registry, hosts_content));
        let handle = tokio::runtime::Handle::current();
        DnsHost::new(resolver, handle)
    }

    // ── resolve_address via Host trait ───────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_registry_entry() {
        let mut registry = HashMap::new();
        registry.insert(
            "db.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
        );
        let mut host = make_host(registry, "");

        let result = host.resolve_address("db.warp.local".into());
        assert!(result.is_ok());
        let records = result.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].address, "10.0.0.5");
        assert!(!records[0].is_ipv6);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_etc_hosts_entry() {
        let hosts = "10.0.0.20 cache.warp.local\n";
        let mut host = make_host(HashMap::new(), hosts);

        let result = host.resolve_address("cache.warp.local".into());
        assert!(result.is_ok());
        let records = result.unwrap();
        assert_eq!(records[0].address, "10.0.0.20");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_ipv6_record() {
        let mut registry = HashMap::new();
        registry.insert(
            "ipv6-svc.warp.local".to_string(),
            vec![IpAddr::V6(Ipv6Addr::LOCALHOST)],
        );
        let mut host = make_host(registry, "");

        let records = host.resolve_address("ipv6-svc.warp.local".into()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].address, "::1");
        assert!(records[0].is_ipv6);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_mixed_ip_versions() {
        let mut registry = HashMap::new();
        registry.insert(
            "dual.warp.local".to_string(),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                IpAddr::V6("fd00::1".parse().unwrap()),
            ],
        );
        let mut host = make_host(registry, "");

        let records = host.resolve_address("dual.warp.local".into()).unwrap();
        assert_eq!(records.len(), 2);
        assert!(!records[0].is_ipv6);
        assert!(records[1].is_ipv6);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_nonexistent_returns_error() {
        let mut host = make_host(HashMap::new(), "");

        let result = host.resolve_address(
            "this-definitely-does-not-exist.invalid".into(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("HostNotFound"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_via_system_dns() {
        let mut host = make_host(HashMap::new(), "");

        let result = host.resolve_address("localhost".into());
        assert!(result.is_ok());
        let records = result.unwrap();
        assert!(!records.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_registry_priority_over_hosts() {
        let mut registry = HashMap::new();
        registry.insert(
            "svc.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))],
        );
        let hosts = "10.10.10.10 svc.local\n";
        let mut host = make_host(registry, hosts);

        let records = host.resolve_address("svc.local".into()).unwrap();
        // Registry IP wins
        assert_eq!(records[0].address, "192.168.1.1");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_case_insensitive() {
        let mut registry = HashMap::new();
        registry.insert(
            "db.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
        );
        let mut host = make_host(registry, "");

        let result = host.resolve_address("DB.WARP.LOCAL".into());
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_returns_wit_compatible_records() {
        let mut registry = HashMap::new();
        registry.insert(
            "test.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))],
        );
        let mut host = make_host(registry, "");

        let records = host.resolve_address("test.local".into()).unwrap();
        // Verify the IpAddressRecord fields match WIT contract
        let record = &records[0];
        assert!(!record.address.is_empty());
        // address is a string representation of the IP
        assert!(record.address.parse::<IpAddr>().is_ok());
    }
}
