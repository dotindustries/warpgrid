//! Virtual filesystem shim.
//!
//! Intercepts file reads for virtual paths (`/etc/resolv.conf`, `/dev/urandom`,
//! `/proc/self/`, timezone data) and falls through to real WASI filesystem otherwise.
//!
//! The [`VirtualFileMap`] holds an immutable set of path-to-content mappings
//! constructed via [`VirtualFileMapBuilder`]. After construction, it cannot be
//! mutated — this makes it safe to share across Wasm instances.

pub mod host;

use crate::tzdata;
use std::collections::HashMap;
use std::sync::Arc;

/// A content provider that generates bytes for a virtual file path.
///
/// Each variant handles a distinct category of virtual path:
/// - `DevNull` — always returns empty bytes, absorbs writes
/// - `DevUrandom` — generates cryptographically random bytes on each read
/// - `StaticContent` — returns a fixed byte sequence (used for `/etc/resolv.conf`, etc.)
/// - `PrefixMapped` — dispatches sub-paths under a prefix to a lookup table
///   (used for `/usr/share/zoneinfo/**` and `/proc/self/`)
#[derive(Clone, Debug)]
enum ContentProvider {
    /// `/dev/null` — empty reads, discarded writes.
    DevNull,
    /// `/dev/urandom` — cryptographically random bytes on each read.
    DevUrandom,
    /// Fixed content returned verbatim.
    StaticContent(Arc<[u8]>),
    /// A set of sub-paths mapping to static content under a prefix directory.
    /// For example, `/usr/share/zoneinfo/` prefix with `{ "UTC": <bytes>, ... }`.
    PrefixMapped(Arc<HashMap<String, Arc<[u8]>>>),
}

/// An immutable map of virtual file paths to content providers.
///
/// Constructed via [`VirtualFileMapBuilder`] and not modifiable after creation.
/// Supports both exact path matches and prefix matches.
#[derive(Clone, Debug)]
pub struct VirtualFileMap {
    /// Exact path → provider (e.g., `/dev/null`, `/etc/resolv.conf`).
    exact: HashMap<String, ContentProvider>,
    /// Prefix path → provider (e.g., `/usr/share/zoneinfo/`, `/proc/self/`).
    /// Prefixes must end with `/`.
    prefixes: Vec<(String, ContentProvider)>,
}

/// Builder for constructing an immutable [`VirtualFileMap`].
///
/// Use the builder methods to register virtual paths, then call [`build`] to
/// produce the final immutable map.
///
/// [`build`]: VirtualFileMapBuilder::build
pub struct VirtualFileMapBuilder {
    exact: HashMap<String, ContentProvider>,
    prefixes: Vec<(String, ContentProvider)>,
}

impl VirtualFileMapBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            exact: HashMap::new(),
            prefixes: Vec::new(),
        }
    }

    /// Register `/dev/null` — returns empty content on read, discards writes.
    pub fn with_dev_null(mut self) -> Self {
        self.exact
            .insert("/dev/null".to_string(), ContentProvider::DevNull);
        self
    }

    /// Register `/dev/urandom` — returns cryptographically random bytes on read.
    pub fn with_dev_urandom(mut self) -> Self {
        self.exact
            .insert("/dev/urandom".to_string(), ContentProvider::DevUrandom);
        self
    }

    /// Register `/etc/resolv.conf` with a configurable nameserver configuration.
    pub fn with_resolv_conf(mut self, content: &str) -> Self {
        self.exact.insert(
            "/etc/resolv.conf".to_string(),
            ContentProvider::StaticContent(content.as_bytes().into()),
        );
        self
    }

    /// Register `/etc/hosts` with a configurable hosts mapping.
    pub fn with_etc_hosts(mut self, content: &str) -> Self {
        self.exact.insert(
            "/etc/hosts".to_string(),
            ContentProvider::StaticContent(content.as_bytes().into()),
        );
        self
    }

    /// Register `/proc/self/` prefix with synthetic process metadata files.
    ///
    /// `entries` maps sub-paths (e.g., `"status"`, `"cmdline"`) to content.
    pub fn with_proc_self(mut self, entries: HashMap<String, Vec<u8>>) -> Self {
        let mapped: HashMap<String, Arc<[u8]>> = entries
            .into_iter()
            .map(|(k, v)| (k, Arc::from(v.into_boxed_slice())))
            .collect();
        self.prefixes.push((
            "/proc/self/".to_string(),
            ContentProvider::PrefixMapped(Arc::new(mapped)),
        ));
        self
    }

    /// Register `/usr/share/zoneinfo/` prefix with embedded timezone data.
    ///
    /// `zones` maps timezone names (e.g., `"UTC"`, `"US/Eastern"`) to TZif data.
    pub fn with_timezone_data(mut self, zones: HashMap<String, Vec<u8>>) -> Self {
        let mapped: HashMap<String, Arc<[u8]>> = zones
            .into_iter()
            .map(|(k, v)| (k, Arc::from(v.into_boxed_slice())))
            .collect();
        self.prefixes.push((
            "/usr/share/zoneinfo/".to_string(),
            ContentProvider::PrefixMapped(Arc::new(mapped)),
        ));
        self
    }

    /// Register an arbitrary exact-path with static content.
    pub fn with_static_file(mut self, path: &str, content: &[u8]) -> Self {
        self.exact.insert(
            path.to_string(),
            ContentProvider::StaticContent(content.into()),
        );
        self
    }

    /// Consume the builder and produce an immutable [`VirtualFileMap`].
    pub fn build(self) -> VirtualFileMap {
        VirtualFileMap {
            exact: self.exact,
            prefixes: self.prefixes,
        }
    }
}

impl Default for VirtualFileMapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents content retrieved from a virtual path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualContent {
    /// The path matched and content is available.
    Found(Vec<u8>),
    /// The path was `/dev/null` — empty read, absorbs writes.
    DevNull,
    /// The path was `/dev/urandom` — callers should generate random bytes of the
    /// requested length on each read (not buffered).
    DevUrandom,
    /// The path did not match any virtual entry.
    NotFound,
}

impl VirtualFileMap {
    /// Create a builder for constructing a `VirtualFileMap`.
    pub fn builder() -> VirtualFileMapBuilder {
        VirtualFileMapBuilder::new()
    }

    /// Create a default `VirtualFileMap` with standard WarpGrid virtual paths.
    ///
    /// Includes `/dev/null`, `/dev/urandom`, default `/etc/resolv.conf`,
    /// `/etc/hosts`, default `/proc/self/` entries, and real TZif v2 timezone
    /// data for UTC, America/New_York, US/Eastern, America/Chicago,
    /// America/Denver, America/Los_Angeles, US/Pacific, Europe/London,
    /// Asia/Tokyo.
    pub fn with_defaults() -> Self {
        let mut proc_entries = HashMap::new();
        proc_entries.insert(
            "status".to_string(),
            b"Name:\twarpgrid-guest\nState:\tR (running)\nPid:\t1\nUid:\t0\t0\t0\t0\n".to_vec(),
        );
        proc_entries.insert("cmdline".to_string(), b"warpgrid-guest\0".to_vec());

        let zones = tzdata::default_timezone_data();

        Self::builder()
            .with_dev_null()
            .with_dev_urandom()
            .with_resolv_conf("nameserver 127.0.0.1\n")
            .with_etc_hosts("127.0.0.1 localhost\n::1 localhost\n")
            .with_proc_self(proc_entries)
            .with_timezone_data(zones)
            .build()
    }

    /// Look up a virtual path, returning the content if it matches.
    ///
    /// Path canonicalization: normalizes `..` and `.` components to prevent
    /// bypass via paths like `/etc/../etc/hosts`.
    pub fn lookup(&self, path: &str) -> VirtualContent {
        let canonical = canonicalize_path(path);

        // Check exact matches first.
        if let Some(provider) = self.exact.get(&canonical) {
            return read_provider(provider, None);
        }

        // Check prefix matches.
        for (prefix, provider) in &self.prefixes {
            if let Some(sub_path) = canonical.strip_prefix(prefix.as_str()) {
                return read_provider(provider, Some(sub_path));
            }
        }

        VirtualContent::NotFound
    }

    /// Check whether a path matches any virtual entry (without reading content).
    pub fn contains(&self, path: &str) -> bool {
        let canonical = canonicalize_path(path);

        if self.exact.contains_key(&canonical) {
            return true;
        }

        for (prefix, provider) in &self.prefixes {
            if let Some(sub_path) = canonical.strip_prefix(prefix.as_str()) {
                if let ContentProvider::PrefixMapped(map) = provider {
                    return map.contains_key(sub_path);
                }
                return true;
            }
        }

        false
    }
}

/// Read content from a provider, optionally using a sub-path for prefix-mapped providers.
fn read_provider(provider: &ContentProvider, sub_path: Option<&str>) -> VirtualContent {
    match provider {
        ContentProvider::DevNull => VirtualContent::DevNull,
        ContentProvider::DevUrandom => VirtualContent::DevUrandom,
        ContentProvider::StaticContent(data) => VirtualContent::Found(data.to_vec()),
        ContentProvider::PrefixMapped(map) => {
            let key = sub_path.unwrap_or("");
            match map.get(key) {
                Some(data) => VirtualContent::Found(data.to_vec()),
                None => VirtualContent::NotFound,
            }
        }
    }
}

/// Canonicalize a path by resolving `.` and `..` components.
///
/// This prevents bypass attempts like `/etc/../etc/hosts` mapping back to `/etc/hosts`.
/// Does NOT resolve symlinks (we don't have a real filesystem).
fn canonicalize_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();

    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other),
        }
    }

    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Builder & Immutability ────────────────────────────────────────

    #[test]
    fn builder_produces_immutable_map() {
        let map = VirtualFileMap::builder()
            .with_dev_null()
            .with_dev_urandom()
            .build();

        // The map exists and is usable — immutability is enforced by the
        // type system (no &mut self methods on VirtualFileMap).
        assert!(map.contains("/dev/null"));
        assert!(map.contains("/dev/urandom"));
    }

    #[test]
    fn empty_builder_produces_empty_map() {
        let map = VirtualFileMap::builder().build();
        assert!(!map.contains("/dev/null"));
        assert_eq!(map.lookup("/anything"), VirtualContent::NotFound);
    }

    #[test]
    fn default_map_has_all_standard_paths() {
        let map = VirtualFileMap::with_defaults();
        assert!(map.contains("/dev/null"));
        assert!(map.contains("/dev/urandom"));
        assert!(map.contains("/etc/resolv.conf"));
        assert!(map.contains("/etc/hosts"));
        assert!(map.contains("/proc/self/status"));
        assert!(map.contains("/proc/self/cmdline"));
        assert!(map.contains("/usr/share/zoneinfo/UTC"));
        assert!(map.contains("/usr/share/zoneinfo/US/Eastern"));
        assert!(map.contains("/usr/share/zoneinfo/US/Pacific"));
        assert!(map.contains("/usr/share/zoneinfo/Europe/London"));
        assert!(map.contains("/usr/share/zoneinfo/America/New_York"));
        assert!(map.contains("/usr/share/zoneinfo/America/Chicago"));
        assert!(map.contains("/usr/share/zoneinfo/America/Denver"));
        assert!(map.contains("/usr/share/zoneinfo/America/Los_Angeles"));
        assert!(map.contains("/usr/share/zoneinfo/Asia/Tokyo"));
    }

    // ── /dev/null ────────────────────────────────────────────────────

    #[test]
    fn dev_null_returns_dev_null_variant() {
        let map = VirtualFileMap::builder().with_dev_null().build();
        assert_eq!(map.lookup("/dev/null"), VirtualContent::DevNull);
    }

    #[test]
    fn dev_null_is_distinct_from_empty_found() {
        let map = VirtualFileMap::builder().with_dev_null().build();
        // DevNull is semantically different from Found(vec![]) — it also accepts writes.
        assert_ne!(map.lookup("/dev/null"), VirtualContent::Found(vec![]));
    }

    // ── /dev/urandom ─────────────────────────────────────────────────

    #[test]
    fn dev_urandom_returns_dev_urandom_variant() {
        let map = VirtualFileMap::builder().with_dev_urandom().build();
        // DevUrandom is a distinct variant — actual random byte generation
        // happens in the FilesystemHost (see filesystem::host module).
        assert_eq!(map.lookup("/dev/urandom"), VirtualContent::DevUrandom);
    }

    // ── /etc/resolv.conf ─────────────────────────────────────────────

    #[test]
    fn resolv_conf_returns_configured_content() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("nameserver 10.0.0.1\n")
            .build();
        match map.lookup("/etc/resolv.conf") {
            VirtualContent::Found(bytes) => {
                assert_eq!(
                    String::from_utf8_lossy(&bytes),
                    "nameserver 10.0.0.1\n"
                );
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn resolv_conf_default_has_nameserver() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/etc/resolv.conf") {
            VirtualContent::Found(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                assert!(content.contains("nameserver"));
                assert!(content.contains("127.0.0.1"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    // ── /etc/hosts ───────────────────────────────────────────────────

    #[test]
    fn etc_hosts_returns_configured_content() {
        let hosts = "127.0.0.1 localhost\n10.0.0.5 db.production.warp.local\n";
        let map = VirtualFileMap::builder().with_etc_hosts(hosts).build();
        match map.lookup("/etc/hosts") {
            VirtualContent::Found(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                assert!(content.contains("db.production.warp.local"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn etc_hosts_populated_from_service_registry_data() {
        // Simulate building hosts from a service registry.
        let mut hosts = String::from("127.0.0.1 localhost\n");
        let registry = [
            ("10.0.0.1", "api.staging.warp.local"),
            ("10.0.0.2", "cache.staging.warp.local"),
        ];
        for (ip, hostname) in &registry {
            hosts.push_str(&format!("{ip} {hostname}\n"));
        }

        let map = VirtualFileMap::builder().with_etc_hosts(&hosts).build();
        match map.lookup("/etc/hosts") {
            VirtualContent::Found(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                assert!(content.contains("api.staging.warp.local"));
                assert!(content.contains("cache.staging.warp.local"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    // ── /proc/self/ ──────────────────────────────────────────────────

    #[test]
    fn proc_self_status_returns_synthetic_metadata() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/proc/self/status") {
            VirtualContent::Found(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                assert!(content.contains("Name:"));
                assert!(content.contains("Pid:"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn proc_self_cmdline_returns_synthetic_data() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/proc/self/cmdline") {
            VirtualContent::Found(bytes) => {
                assert!(!bytes.is_empty());
                // cmdline should contain a null-terminated string
                assert!(bytes.contains(&0u8));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn proc_self_unknown_subpath_returns_not_found() {
        let map = VirtualFileMap::with_defaults();
        assert_eq!(
            map.lookup("/proc/self/nonexistent"),
            VirtualContent::NotFound
        );
    }

    #[test]
    fn proc_self_custom_entries() {
        let mut entries = HashMap::new();
        entries.insert("maps".to_string(), b"custom-maps-data".to_vec());

        let map = VirtualFileMap::builder().with_proc_self(entries).build();
        match map.lookup("/proc/self/maps") {
            VirtualContent::Found(bytes) => {
                assert_eq!(bytes, b"custom-maps-data");
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    // ── /usr/share/zoneinfo/** ───────────────────────────────────────

    #[test]
    fn timezone_utc_has_valid_tzif_v2() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/usr/share/zoneinfo/UTC") {
            VirtualContent::Found(bytes) => {
                assert!(bytes.starts_with(b"TZif"), "missing TZif magic");
                assert_eq!(bytes[4], b'2', "expected TZif version 2");
                // Real TZif v2: must have two headers (v1 + v2).
                assert!(bytes.len() > 88, "too small for TZif v2 (two 44-byte headers)");
                // Should contain "UTC" abbreviation.
                assert!(bytes.windows(3).any(|w| w == b"UTC"));
                // Footer should end with newline.
                assert_eq!(bytes[bytes.len() - 1], b'\n');
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn timezone_america_new_york_has_est_edt() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/usr/share/zoneinfo/America/New_York") {
            VirtualContent::Found(bytes) => {
                assert!(bytes.starts_with(b"TZif"));
                // Should contain both abbreviations.
                assert!(bytes.windows(3).any(|w| w == b"EST"));
                assert!(bytes.windows(3).any(|w| w == b"EDT"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn timezone_us_eastern_available_in_defaults() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/usr/share/zoneinfo/US/Eastern") {
            VirtualContent::Found(bytes) => {
                assert!(bytes.starts_with(b"TZif"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn timezone_us_pacific_available_in_defaults() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/usr/share/zoneinfo/US/Pacific") {
            VirtualContent::Found(bytes) => {
                assert!(bytes.starts_with(b"TZif"));
                assert!(bytes.windows(3).any(|w| w == b"PST"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn timezone_europe_london_has_gmt_bst() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/usr/share/zoneinfo/Europe/London") {
            VirtualContent::Found(bytes) => {
                assert!(bytes.starts_with(b"TZif"));
                assert!(bytes.windows(3).any(|w| w == b"GMT"));
                assert!(bytes.windows(3).any(|w| w == b"BST"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn timezone_asia_tokyo_has_jst() {
        let map = VirtualFileMap::with_defaults();
        match map.lookup("/usr/share/zoneinfo/Asia/Tokyo") {
            VirtualContent::Found(bytes) => {
                assert!(bytes.starts_with(b"TZif"));
                assert!(bytes.windows(3).any(|w| w == b"JST"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn timezone_unknown_returns_not_found() {
        let map = VirtualFileMap::with_defaults();
        assert_eq!(
            map.lookup("/usr/share/zoneinfo/Mars/Olympus"),
            VirtualContent::NotFound
        );
    }

    #[test]
    fn custom_timezone_data() {
        let mut zones = HashMap::new();
        zones.insert("Custom/Zone".to_string(), b"TZif2custom-data".to_vec());

        let map = VirtualFileMap::builder()
            .with_timezone_data(zones)
            .build();
        match map.lookup("/usr/share/zoneinfo/Custom/Zone") {
            VirtualContent::Found(bytes) => {
                assert_eq!(bytes, b"TZif2custom-data");
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn all_default_timezones_within_vfd_size_limit() {
        let map = VirtualFileMap::with_defaults();
        let zones = [
            "UTC",
            "America/New_York",
            "US/Eastern",
            "America/Chicago",
            "America/Denver",
            "America/Los_Angeles",
            "US/Pacific",
            "Europe/London",
            "Asia/Tokyo",
        ];
        for zone in &zones {
            match map.lookup(&format!("/usr/share/zoneinfo/{zone}")) {
                VirtualContent::Found(bytes) => {
                    assert!(
                        bytes.len() <= 8192,
                        "{zone}: TZif data ({} bytes) exceeds WARPGRID_FS_MAX_CONTENT",
                        bytes.len()
                    );
                }
                other => panic!("{zone}: expected Found, got {:?}", other),
            }
        }
    }

    // ── Path canonicalization ────────────────────────────────────────

    #[test]
    fn path_traversal_dot_dot_is_canonicalized() {
        let map = VirtualFileMap::with_defaults();
        // `/etc/../etc/hosts` should resolve to `/etc/hosts`.
        match map.lookup("/etc/../etc/hosts") {
            VirtualContent::Found(bytes) => {
                assert!(!bytes.is_empty());
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn path_traversal_dot_is_stripped() {
        let map = VirtualFileMap::with_defaults();
        // `/etc/./resolv.conf` should resolve to `/etc/resolv.conf`.
        match map.lookup("/etc/./resolv.conf") {
            VirtualContent::Found(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                assert!(content.contains("nameserver"));
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn path_traversal_multiple_dot_dots() {
        let map = VirtualFileMap::with_defaults();
        // `/a/b/c/../../../dev/null` → pop c, pop b, pop a → `/dev/null`.
        assert_eq!(
            map.lookup("/a/b/c/../../../dev/null"),
            VirtualContent::DevNull
        );
    }

    #[test]
    fn path_traversal_beyond_root_is_clamped() {
        let map = VirtualFileMap::with_defaults();
        // `/../../../dev/null` should still resolve to `/dev/null`.
        assert_eq!(
            map.lookup("/../../../dev/null"),
            VirtualContent::DevNull
        );
    }

    // ── Non-virtual paths return NotFound ─────────────────────────────

    #[test]
    fn non_virtual_path_returns_not_found() {
        let map = VirtualFileMap::with_defaults();
        assert_eq!(
            map.lookup("/tmp/some-real-file.txt"),
            VirtualContent::NotFound
        );
    }

    #[test]
    fn root_path_returns_not_found() {
        let map = VirtualFileMap::with_defaults();
        assert_eq!(map.lookup("/"), VirtualContent::NotFound);
    }

    // ── Static file registration ─────────────────────────────────────

    #[test]
    fn custom_static_file() {
        let map = VirtualFileMap::builder()
            .with_static_file("/etc/warpgrid/proxy.conf", b"proxy_addr=127.0.0.1:54321\n")
            .build();
        match map.lookup("/etc/warpgrid/proxy.conf") {
            VirtualContent::Found(bytes) => {
                assert_eq!(
                    String::from_utf8_lossy(&bytes),
                    "proxy_addr=127.0.0.1:54321\n"
                );
            }
            other => panic!("expected Found, got {:?}", other),
        }
    }

    // ── contains() ───────────────────────────────────────────────────

    #[test]
    fn contains_exact_path() {
        let map = VirtualFileMap::builder().with_dev_null().build();
        assert!(map.contains("/dev/null"));
        assert!(!map.contains("/dev/urandom"));
    }

    #[test]
    fn contains_prefix_path() {
        let map = VirtualFileMap::with_defaults();
        assert!(map.contains("/proc/self/status"));
        assert!(!map.contains("/proc/self/nonexistent"));
    }

    #[test]
    fn contains_with_path_canonicalization() {
        let map = VirtualFileMap::with_defaults();
        assert!(map.contains("/etc/../etc/hosts"));
    }

    // ── canonicalize_path unit tests ─────────────────────────────────

    #[test]
    fn canonicalize_simple_path() {
        assert_eq!(canonicalize_path("/etc/hosts"), "/etc/hosts");
    }

    #[test]
    fn canonicalize_dot_dot() {
        assert_eq!(canonicalize_path("/etc/../dev/null"), "/dev/null");
    }

    #[test]
    fn canonicalize_dot() {
        assert_eq!(canonicalize_path("/etc/./resolv.conf"), "/etc/resolv.conf");
    }

    #[test]
    fn canonicalize_multiple_slashes() {
        assert_eq!(canonicalize_path("//etc///hosts"), "/etc/hosts");
    }

    #[test]
    fn canonicalize_beyond_root() {
        assert_eq!(canonicalize_path("/../../../dev/null"), "/dev/null");
    }

    #[test]
    fn canonicalize_root() {
        assert_eq!(canonicalize_path("/"), "/");
    }

    #[test]
    fn canonicalize_trailing_slash() {
        assert_eq!(canonicalize_path("/etc/"), "/etc");
    }

    // ── US-208 Edge Cases ───────────────────────────────────────────

    #[test]
    fn repeated_lookup_idempotency() {
        let map = VirtualFileMap::with_defaults();
        let first = map.lookup("/etc/resolv.conf");

        for i in 0..100 {
            let result = map.lookup("/etc/resolv.conf");
            assert_eq!(
                result, first,
                "lookup #{i} returned different content — immutability violated"
            );
        }
    }
}
