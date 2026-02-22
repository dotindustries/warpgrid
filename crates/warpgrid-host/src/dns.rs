//! DNS resolution shim.
//!
//! Routes hostname resolution through WarpGrid's service discovery chain:
//! 1. **Service registry** — injected `HashMap<String, Vec<IpAddr>>`
//! 2. **Virtual `/etc/hosts`** — parsed from the hosts file content
//! 3. **Host system DNS** — fallback via `tokio::net::lookup_host`
//!
//! Resolution stops at the first chain link that returns results.
//! All resolution steps are logged at `tracing::debug` level.

pub mod host;

use std::collections::HashMap;
use std::net::IpAddr;

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
}
