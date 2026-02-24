//! DNS resolution shim.
//!
//! Routes hostname resolution through WarpGrid's service discovery chain:
//! 1. **Service registry** — injected `HashMap<String, Vec<IpAddr>>`
//! 2. **Virtual `/etc/hosts`** — parsed from the hosts file content
//! 3. **Host system DNS** — fallback via `tokio::net::lookup_host`
//!
//! Resolution stops at the first chain link that returns results.
//! Results are cached with configurable TTL and returned in round-robin
//! order for load balancing across service replicas.
//! All resolution steps are logged at `tracing::debug` level.

pub mod cache;
pub mod host;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

use cache::{DnsCache, DnsCacheConfig};

/// Parsed `/etc/hosts` entries: hostname → list of IP addresses.
///
/// Built from the virtual `/etc/hosts` content by [`parse_etc_hosts`].
#[derive(Clone, Debug)]
pub struct EtcHosts {
    entries: HashMap<String, Vec<IpAddr>>,
}

impl EtcHosts {
    /// Parse `/etc/hosts`-format content into a lookup table.
    ///
    /// Each non-comment, non-empty line is expected to have the format:
    /// `<IP> <hostname1> [hostname2 ...]`
    ///
    /// Lines starting with `#` are ignored. Each hostname maps to the IP
    /// on that line. If the same hostname appears on multiple lines, all
    /// IPs are collected.
    pub fn parse(content: &str) -> Self {
        let mut entries: HashMap<String, Vec<IpAddr>> = HashMap::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let mut parts = line.split_whitespace();
            let Some(ip_str) = parts.next() else {
                continue;
            };
            let Ok(ip) = ip_str.parse::<IpAddr>() else {
                tracing::debug!(line = %line, "skipping /etc/hosts line with invalid IP");
                continue;
            };

            for hostname in parts {
                // Skip inline comments
                if hostname.starts_with('#') {
                    break;
                }
                entries
                    .entry(hostname.to_lowercase())
                    .or_default()
                    .push(ip);
            }
        }

        Self { entries }
    }

    /// Look up a hostname, returning its IP addresses (if any).
    pub fn lookup(&self, hostname: &str) -> Option<&Vec<IpAddr>> {
        self.entries.get(&hostname.to_lowercase())
    }
}

/// DNS resolver with a three-tier resolution chain.
///
/// Constructed immutably with an injected service registry and `/etc/hosts`
/// content. The resolution chain is:
/// 1. Service registry (injected `HashMap<String, Vec<IpAddr>>`)
/// 2. Virtual `/etc/hosts` (parsed from content string)
/// 3. Host system DNS (via `tokio::net::lookup_host`)
///
/// Resolution stops at the first chain link that returns results.
pub struct DnsResolver {
    /// Service registry: hostname → list of IP addresses.
    service_registry: HashMap<String, Vec<IpAddr>>,
    /// Parsed `/etc/hosts` entries.
    etc_hosts: EtcHosts,
}

impl DnsResolver {
    /// Create a new DNS resolver.
    ///
    /// # Arguments
    /// - `service_registry` — map of service names to their IP addresses
    /// - `etc_hosts_content` — content of the virtual `/etc/hosts` file
    pub fn new(
        service_registry: HashMap<String, Vec<IpAddr>>,
        etc_hosts_content: &str,
    ) -> Self {
        Self {
            service_registry,
            etc_hosts: EtcHosts::parse(etc_hosts_content),
        }
    }

    /// Resolve a hostname through the three-tier chain.
    ///
    /// Returns `Ok(addresses)` on success, or `Err` with a `HostNotFound`
    /// message if no chain link resolves the hostname.
    ///
    /// This is an async method because the final fallback uses
    /// `tokio::net::lookup_host`.
    pub async fn resolve(&self, hostname: &str) -> Result<Vec<IpAddr>, String> {
        let hostname_lower = hostname.to_lowercase();

        // Chain link 1: Service registry
        if let Some(addrs) = self.service_registry.get(&hostname_lower)
            && !addrs.is_empty()
        {
            tracing::debug!(
                hostname = %hostname,
                source = "service_registry",
                count = addrs.len(),
                "DNS resolved via service registry"
            );
            return Ok(addrs.clone());
        }

        // Chain link 2: Virtual /etc/hosts
        if let Some(addrs) = self.etc_hosts.lookup(&hostname_lower)
            && !addrs.is_empty()
        {
            tracing::debug!(
                hostname = %hostname,
                source = "etc_hosts",
                count = addrs.len(),
                "DNS resolved via /etc/hosts"
            );
            return Ok(addrs.clone());
        }

        // Chain link 3: Host system DNS
        tracing::debug!(
            hostname = %hostname,
            source = "system_dns",
            "DNS falling back to system DNS"
        );

        let lookup_addr = format!("{hostname_lower}:0");
        match tokio::net::lookup_host(&lookup_addr).await {
            Ok(addrs) => {
                let ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
                if ips.is_empty() {
                    tracing::debug!(
                        hostname = %hostname,
                        "system DNS returned no addresses"
                    );
                    Err(format!("HostNotFound: {hostname}"))
                } else {
                    tracing::debug!(
                        hostname = %hostname,
                        source = "system_dns",
                        count = ips.len(),
                        "DNS resolved via system DNS"
                    );
                    Ok(ips)
                }
            }
            Err(e) => {
                tracing::debug!(
                    hostname = %hostname,
                    error = %e,
                    "system DNS lookup failed"
                );
                Err(format!("HostNotFound: {hostname}"))
            }
        }
    }
}

/// DNS resolver with TTL caching and round-robin address selection.
///
/// Wraps a [`DnsResolver`] and a [`DnsCache`], caching successful resolution
/// results with a configurable TTL. Consecutive lookups for hostnames with
/// multiple addresses are returned in round-robin order using per-hostname
/// atomic counters (no mutex contention on the hot path once the cache
/// lock is released).
///
/// # Concurrency
///
/// The cache is protected by a `std::sync::Mutex`. Lock hold time is
/// minimised to a single hash map lookup or insert — the expensive async
/// resolution runs *outside* the lock.
pub struct CachedDnsResolver {
    /// The underlying resolver implementing the three-tier chain.
    resolver: DnsResolver,
    /// TTL-bounded, LRU-evicting DNS cache.
    cache: Mutex<DnsCache>,
}

impl CachedDnsResolver {
    /// Create a new cached resolver.
    ///
    /// # Arguments
    /// - `resolver` — the underlying DNS resolver
    /// - `cache_config` — TTL and capacity configuration for the cache
    pub fn new(resolver: DnsResolver, cache_config: DnsCacheConfig) -> Self {
        Self {
            resolver,
            cache: Mutex::new(DnsCache::new(cache_config)),
        }
    }

    /// Resolve a hostname, returning all addresses.
    ///
    /// Checks the cache first. On a miss (or TTL expiry), delegates to the
    /// underlying resolver and caches the result.
    pub async fn resolve(&self, hostname: &str) -> Result<Vec<IpAddr>, String> {
        // Fast path: check cache
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(addrs) = cache.get(hostname) {
                return Ok(addrs.to_vec());
            }
        }

        // Cache miss — resolve through the chain
        let addrs = self.resolver.resolve(hostname).await?;

        // Populate cache
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(hostname, addrs.clone());
        }

        Ok(addrs)
    }

    /// Resolve a hostname and return one address in round-robin order.
    ///
    /// Checks the cache first. On a miss, resolves and caches, then returns
    /// the first address. Subsequent calls cycle through all cached addresses.
    pub async fn resolve_round_robin(&self, hostname: &str) -> Result<IpAddr, String> {
        // Fast path: check cache for round-robin hit
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(addr) = cache.get_round_robin(hostname) {
                return Ok(addr);
            }
        }

        // Cache miss — resolve through the chain
        let addrs = self.resolver.resolve(hostname).await?;

        if addrs.is_empty() {
            return Err(format!("HostNotFound: {hostname}"));
        }

        // First address to return (before cache takes ownership)
        let first = addrs[0];

        // Populate cache (next call will get round-robin index 1)
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(hostname, addrs);
            // Advance the counter once since we're returning the first address
            // The insert creates a fresh entry with counter 0, and the caller
            // gets index 0. Next get_round_robin will get index 0 again (since
            // we haven't called get_round_robin yet for this entry). We need
            // to call get_round_robin to advance the counter properly.
            let _ = cache.get_round_robin(hostname);
        }

        Ok(first)
    }

    /// Get a reference to the underlying resolver.
    pub fn resolver(&self) -> &DnsResolver {
        &self.resolver
    }

    /// Get cache statistics: `(hits, misses, evictions)`.
    pub fn cache_stats(&self) -> (u64, u64, u64) {
        let cache = self.cache.lock().unwrap();
        cache.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    // ── EtcHosts parsing ────────────────────────────────────────────

    #[test]
    fn parse_empty_content() {
        let hosts = EtcHosts::parse("");
        assert!(hosts.lookup("anything").is_none());
    }

    #[test]
    fn parse_comment_only() {
        let hosts = EtcHosts::parse("# this is a comment\n# another one\n");
        assert!(hosts.lookup("anything").is_none());
    }

    #[test]
    fn parse_single_ipv4_entry() {
        let hosts = EtcHosts::parse("127.0.0.1 localhost\n");
        let addrs = hosts.lookup("localhost").unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn parse_single_ipv6_entry() {
        let hosts = EtcHosts::parse("::1 localhost\n");
        let addrs = hosts.lookup("localhost").unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], IpAddr::V6(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn parse_multiple_hostnames_per_line() {
        let hosts = EtcHosts::parse("10.0.0.1 api.warp.local api\n");
        assert!(hosts.lookup("api.warp.local").is_some());
        assert!(hosts.lookup("api").is_some());
        let addr = hosts.lookup("api.warp.local").unwrap()[0];
        assert_eq!(addr, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn parse_multiple_lines_same_hostname() {
        let content = "127.0.0.1 localhost\n::1 localhost\n";
        let hosts = EtcHosts::parse(content);
        let addrs = hosts.lookup("localhost").unwrap();
        assert_eq!(addrs.len(), 2);
        assert!(addrs.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(addrs.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn parse_skips_invalid_ip_lines() {
        let content = "not-an-ip badhost\n127.0.0.1 goodhost\n";
        let hosts = EtcHosts::parse(content);
        assert!(hosts.lookup("badhost").is_none());
        assert!(hosts.lookup("goodhost").is_some());
    }

    #[test]
    fn parse_handles_inline_comments() {
        let content = "10.0.0.1 myhost # this is a comment\n";
        let hosts = EtcHosts::parse(content);
        assert!(hosts.lookup("myhost").is_some());
        // The comment part should not be treated as a hostname
        assert!(hosts.lookup("#").is_none());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let hosts = EtcHosts::parse("10.0.0.1 MyHost.Local\n");
        assert!(hosts.lookup("myhost.local").is_some());
        assert!(hosts.lookup("MYHOST.LOCAL").is_some());
        assert!(hosts.lookup("MyHost.Local").is_some());
    }

    #[test]
    fn parse_handles_mixed_whitespace() {
        let content = "  10.0.0.1\t\tspaced-host  \n";
        let hosts = EtcHosts::parse(content);
        assert!(hosts.lookup("spaced-host").is_some());
    }

    // ── DnsResolver construction ────────────────────────────────────

    #[test]
    fn resolver_constructed_with_empty_registry() {
        let resolver = DnsResolver::new(HashMap::new(), "");
        assert!(resolver.service_registry.is_empty());
    }

    #[test]
    fn resolver_constructed_with_registry_entries() {
        let mut registry = HashMap::new();
        registry.insert(
            "db.production.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
        );
        let resolver = DnsResolver::new(registry, "");
        assert_eq!(resolver.service_registry.len(), 1);
    }

    // ── DnsResolver resolution chain (async) ─────────────────────────

    #[tokio::test]
    async fn resolve_from_service_registry() {
        let mut registry = HashMap::new();
        let expected_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        registry.insert(
            "db.production.warp.local".to_string(),
            vec![expected_ip],
        );
        let resolver = DnsResolver::new(registry, "");

        let result = resolver.resolve("db.production.warp.local").await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], expected_ip);
    }

    #[tokio::test]
    async fn resolve_registry_is_case_insensitive() {
        let mut registry = HashMap::new();
        registry.insert(
            "db.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
        );
        let resolver = DnsResolver::new(registry, "");

        let result = resolver.resolve("DB.WARP.LOCAL").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn resolve_from_etc_hosts_when_not_in_registry() {
        let hosts_content = "10.0.0.10 cache.staging.warp.local\n";
        let resolver = DnsResolver::new(HashMap::new(), hosts_content);

        let result = resolver.resolve("cache.staging.warp.local").await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert_eq!(addrs[0], IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)));
    }

    #[tokio::test]
    async fn resolve_registry_takes_priority_over_etc_hosts() {
        // Same hostname in both registry and /etc/hosts — registry wins.
        let mut registry = HashMap::new();
        let registry_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        registry.insert("db.warp.local".to_string(), vec![registry_ip]);

        let hosts_content = "10.0.0.99 db.warp.local\n";
        let resolver = DnsResolver::new(registry, hosts_content);

        let result = resolver.resolve("db.warp.local").await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert_eq!(addrs[0], registry_ip);
    }

    #[tokio::test]
    async fn resolve_stops_at_first_chain_link_with_results() {
        // If registry resolves it, /etc/hosts and system DNS are not consulted.
        // We can verify this indirectly: registry returns specific IPs that
        // differ from /etc/hosts, confirming registry was used.
        let mut registry = HashMap::new();
        let registry_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        registry.insert("myservice".to_string(), vec![registry_ip]);

        let hosts_content = "10.10.10.10 myservice\n";
        let resolver = DnsResolver::new(registry, hosts_content);

        let addrs = resolver.resolve("myservice").await.unwrap();
        // Must be the registry IP, not the /etc/hosts IP
        assert_eq!(addrs[0], registry_ip);
    }

    #[tokio::test]
    async fn resolve_multiple_registry_addresses() {
        let mut registry = HashMap::new();
        registry.insert(
            "api.warp.local".to_string(),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
            ],
        );
        let resolver = DnsResolver::new(registry, "");

        let addrs = resolver.resolve("api.warp.local").await.unwrap();
        assert_eq!(addrs.len(), 3);
    }

    #[tokio::test]
    async fn resolve_via_system_dns_for_localhost() {
        // "localhost" should be resolvable by the system DNS when not
        // in the registry or /etc/hosts.
        let resolver = DnsResolver::new(HashMap::new(), "");

        let result = resolver.resolve("localhost").await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert!(!addrs.is_empty());
    }

    #[tokio::test]
    async fn resolve_unresolvable_hostname_returns_host_not_found() {
        let resolver = DnsResolver::new(HashMap::new(), "");

        let result = resolver
            .resolve("this-hostname-definitely-does-not-exist.invalid")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("HostNotFound"),
            "expected HostNotFound, got: {err}"
        );
    }

    #[tokio::test]
    async fn resolve_empty_hostname_returns_error() {
        let resolver = DnsResolver::new(HashMap::new(), "");

        let result = resolver.resolve("").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_etc_hosts_with_ipv6() {
        let hosts_content = "::1 ipv6host\n";
        let resolver = DnsResolver::new(HashMap::new(), hosts_content);

        let addrs = resolver.resolve("ipv6host").await.unwrap();
        assert_eq!(addrs[0], IpAddr::V6(Ipv6Addr::LOCALHOST));
    }

    #[tokio::test]
    async fn resolve_registry_with_mixed_ip_versions() {
        let mut registry = HashMap::new();
        registry.insert(
            "dual-stack.warp.local".to_string(),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                IpAddr::V6("fd00::1".parse().unwrap()),
            ],
        );
        let resolver = DnsResolver::new(registry, "");

        let addrs = resolver.resolve("dual-stack.warp.local").await.unwrap();
        assert_eq!(addrs.len(), 2);
        assert!(addrs.iter().any(|a| a.is_ipv4()));
        assert!(addrs.iter().any(|a| a.is_ipv6()));
    }

    // ── CachedDnsResolver ────────────────────────────────────────────

    fn make_cached_resolver(
        registry: HashMap<String, Vec<IpAddr>>,
        hosts_content: &str,
        cache_config: DnsCacheConfig,
    ) -> CachedDnsResolver {
        let resolver = DnsResolver::new(registry, hosts_content);
        CachedDnsResolver::new(resolver, cache_config)
    }

    #[tokio::test]
    async fn cached_resolve_returns_same_as_uncached() {
        let mut registry = HashMap::new();
        let expected = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        registry.insert("db.warp.local".to_string(), vec![expected]);

        let cached = make_cached_resolver(registry, "", DnsCacheConfig::default());
        let addrs = cached.resolve("db.warp.local").await.unwrap();
        assert_eq!(addrs, vec![expected]);
    }

    #[tokio::test]
    async fn cached_resolve_second_call_hits_cache() {
        let mut registry = HashMap::new();
        registry.insert(
            "db.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
        );
        let cached = make_cached_resolver(registry, "", DnsCacheConfig::default());

        // First call: miss
        cached.resolve("db.warp.local").await.unwrap();
        // Second call: hit
        cached.resolve("db.warp.local").await.unwrap();

        let (hits, misses, _) = cached.cache_stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }

    #[tokio::test]
    async fn cached_resolve_ttl_expiry_re_resolves() {
        use std::time::Duration;

        let mut registry = HashMap::new();
        registry.insert(
            "svc.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))],
        );
        let config = DnsCacheConfig {
            ttl: Duration::from_millis(50),
            max_entries: 1024,
        };
        let cached = make_cached_resolver(registry, "", config);

        // First call: miss
        cached.resolve("svc.warp.local").await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Second call: miss again (expired)
        cached.resolve("svc.warp.local").await.unwrap();

        let (hits, misses, _) = cached.cache_stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 2);
    }

    #[tokio::test]
    async fn cached_round_robin_cycles_addresses() {
        let mut registry = HashMap::new();
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
        ];
        registry.insert("api.warp.local".to_string(), addrs.clone());

        let cached = make_cached_resolver(registry, "", DnsCacheConfig::default());

        // First call populates cache and returns first address
        let a1 = cached.resolve_round_robin("api.warp.local").await.unwrap();
        assert_eq!(a1, addrs[0]);

        // Subsequent calls cycle through cached addresses
        let a2 = cached.resolve_round_robin("api.warp.local").await.unwrap();
        assert_eq!(a2, addrs[1]);

        let a3 = cached.resolve_round_robin("api.warp.local").await.unwrap();
        assert_eq!(a3, addrs[2]);

        // Wraps around
        let a4 = cached.resolve_round_robin("api.warp.local").await.unwrap();
        assert_eq!(a4, addrs[0]);
    }

    #[tokio::test]
    async fn cached_round_robin_single_address() {
        let mut registry = HashMap::new();
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        registry.insert("single.warp.local".to_string(), vec![addr]);

        let cached = make_cached_resolver(registry, "", DnsCacheConfig::default());

        for _ in 0..5 {
            let result = cached.resolve_round_robin("single.warp.local").await.unwrap();
            assert_eq!(result, addr);
        }
    }

    #[tokio::test]
    async fn cached_resolve_nonexistent_returns_error() {
        let cached = make_cached_resolver(HashMap::new(), "", DnsCacheConfig::default());

        let result = cached
            .resolve("this-hostname-definitely-does-not-exist.invalid")
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HostNotFound"));
    }

    #[tokio::test]
    async fn cached_resolve_nonexistent_not_cached() {
        let cached = make_cached_resolver(HashMap::new(), "", DnsCacheConfig::default());

        // Failed resolutions should not be cached
        let _ = cached
            .resolve("nonexistent.invalid")
            .await;

        let (hits, _, _) = cached.cache_stats();
        assert_eq!(hits, 0);
    }

    #[tokio::test]
    async fn cached_resolve_etc_hosts_fallback() {
        let hosts = "10.0.0.20 cache.warp.local\n";
        let cached = make_cached_resolver(HashMap::new(), hosts, DnsCacheConfig::default());

        let addrs = cached.resolve("cache.warp.local").await.unwrap();
        assert_eq!(addrs[0], IpAddr::V4(Ipv4Addr::new(10, 0, 0, 20)));

        // Second call should hit cache
        cached.resolve("cache.warp.local").await.unwrap();
        let (hits, _, _) = cached.cache_stats();
        assert_eq!(hits, 1);
    }
}
