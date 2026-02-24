//! DNS resolution host functions.
//!
//! Implements the `warpgrid:shim/dns` [`Host`] trait, delegating hostname
//! resolution to the [`CachedDnsResolver`] chain (cache → service registry
//! → `/etc/hosts` → system DNS) with TTL caching and round-robin address
//! selection.
//!
//! # Resolution flow
//!
//! ```text
//! Guest calls resolve_address("db.production.warp.local")
//!   → DnsHost delegates to CachedDnsResolver::resolve()
//!     → Cache hit → return cached addresses
//!     → Cache miss → Chain: registry → /etc/hosts → system DNS
//!     → First match wins → cache result → Ok(list<ip-address-record>)
//!     → No match         → Err("HostNotFound: ...")
//! ```

use std::sync::Arc;

use crate::bindings::warpgrid::shim::dns::{Host, IpAddressRecord};
use super::CachedDnsResolver;

/// Host-side implementation of the `warpgrid:shim/dns` interface.
///
/// Wraps a [`CachedDnsResolver`] and converts between Rust `IpAddr` types and
/// the WIT `ip-address-record` type. Resolution results are cached with TTL
/// and returned in round-robin order for multi-address hostnames.
pub struct DnsHost {
    /// The cached DNS resolver implementing the resolution chain with caching.
    resolver: Arc<CachedDnsResolver>,
    /// Tokio runtime handle for running async resolution from sync context.
    runtime_handle: tokio::runtime::Handle,
}

impl DnsHost {
    /// Create a new `DnsHost` wrapping the given cached resolver.
    ///
    /// The `runtime_handle` is used to block on the async resolver from
    /// the synchronous `Host` trait methods.
    pub fn new(resolver: Arc<CachedDnsResolver>, runtime_handle: tokio::runtime::Handle) -> Self {
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

    use crate::dns::{DnsResolver, cache::DnsCacheConfig};

    /// Create a `DnsHost` with the given registry, hosts content, and default cache.
    fn make_host(
        registry: HashMap<String, Vec<IpAddr>>,
        hosts_content: &str,
    ) -> DnsHost {
        let resolver = DnsResolver::new(registry, hosts_content);
        let cached = Arc::new(CachedDnsResolver::new(resolver, DnsCacheConfig::default()));
        let handle = tokio::runtime::Handle::current();
        DnsHost::new(cached, handle)
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_resolve_caches_result() {
        let mut registry = HashMap::new();
        registry.insert(
            "cached.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
        );
        let resolver = DnsResolver::new(registry, "");
        let cached = Arc::new(CachedDnsResolver::new(resolver, DnsCacheConfig::default()));
        let handle = tokio::runtime::Handle::current();
        let mut host = DnsHost::new(Arc::clone(&cached), handle);

        // First call — cache miss
        host.resolve_address("cached.warp.local".into()).unwrap();

        // Second call — cache hit
        host.resolve_address("cached.warp.local".into()).unwrap();

        let (hits, misses, _) = cached.cache_stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }
}
