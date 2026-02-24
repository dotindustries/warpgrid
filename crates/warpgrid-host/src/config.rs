//! ShimConfig — deployment spec parsing.
//!
//! Parses WarpGrid deployment specifications into shim configuration:
//! virtual filesystem entries, DNS overrides, database pool settings,
//! signal handlers, and threading model.
//!
//! Supports two parsing paths:
//! - `ShimConfig::from_warp_config()` — from a typed `warp-core::ShimsConfig`
//! - `ShimConfig::from_toml()` — from a raw `toml::Value` (the `[shims]` table)

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use crate::db_proxy::PoolConfig;
use crate::dns::cache::DnsCacheConfig;

/// Known shim domain names for forward-compatibility validation.
const KNOWN_SHIM_KEYS: &[&str] = &[
    "filesystem",
    "dns",
    "signals",
    "database_proxy",
    "threading",
];

/// Domain-specific configuration for the DNS shim.
#[derive(Debug, Clone)]
pub struct DnsConfig {
    /// TTL for cached DNS entries in seconds (default: 30).
    pub ttl_seconds: u64,
    /// Maximum number of cached DNS entries (default: 1024).
    pub cache_size: usize,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            ttl_seconds: 30,
            cache_size: 1024,
        }
    }
}

impl DnsConfig {
    /// Convert to the internal `DnsCacheConfig` used by the cached DNS resolver.
    pub fn to_cache_config(&self) -> DnsCacheConfig {
        DnsCacheConfig {
            ttl: Duration::from_secs(self.ttl_seconds),
            max_entries: self.cache_size,
        }
    }
}

/// Domain-specific configuration for the filesystem shim.
#[derive(Debug, Clone)]
pub struct FilesystemConfig {
    /// Additional virtual paths to register beyond the defaults.
    /// Maps virtual path to static content.
    pub extra_virtual_paths: HashMap<String, Vec<u8>>,
    /// Timezone name for `/usr/share/zoneinfo/` (default: "UTC").
    pub timezone_name: String,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            extra_virtual_paths: HashMap::new(),
            timezone_name: "UTC".to_string(),
        }
    }
}

/// Domain-specific configuration for the database proxy shim.
#[derive(Debug, Clone)]
pub struct DatabaseProxyConfig {
    /// Maximum connections per pool key (default: 10).
    pub pool_size: usize,
    /// Idle connection timeout in seconds (default: 300).
    pub idle_timeout_seconds: u64,
    /// Health check interval in seconds (default: 30).
    pub health_check_interval_seconds: u64,
    /// Maximum time to wait for a connection when pool is exhausted (default: 5s).
    pub connect_timeout_seconds: u64,
    /// Timeout for recv operations in seconds (default: 30).
    pub recv_timeout_seconds: u64,
}

impl Default for DatabaseProxyConfig {
    fn default() -> Self {
        Self {
            pool_size: 10,
            idle_timeout_seconds: 300,
            health_check_interval_seconds: 30,
            connect_timeout_seconds: 5,
            recv_timeout_seconds: 30,
        }
    }
}

impl DatabaseProxyConfig {
    /// Convert to the internal `PoolConfig` used by the connection pool manager.
    pub fn to_pool_config(&self) -> PoolConfig {
        PoolConfig {
            max_size: self.pool_size,
            idle_timeout: Duration::from_secs(self.idle_timeout_seconds),
            health_check_interval: Duration::from_secs(self.health_check_interval_seconds),
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds),
            recv_timeout: Duration::from_secs(self.recv_timeout_seconds),
            ..PoolConfig::default()
        }
    }
}

/// Host-side shim configuration for a single Wasm instance.
///
/// Built from a `warp-core::ShimsConfig` (the user-facing TOML config)
/// and enriched with deployment-specific data (service registry, env vars).
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// Enable filesystem shim (virtual `/etc/resolv.conf`, `/dev/urandom`, timezone data, etc.).
    pub filesystem: bool,
    /// Enable DNS resolution shim.
    pub dns: bool,
    /// Enable signal delivery shim.
    pub signals: bool,
    /// Enable database proxy shim.
    pub database_proxy: bool,
    /// Enable threading model declaration shim.
    pub threading: bool,
    /// Domain-specific filesystem configuration.
    pub filesystem_config: FilesystemConfig,
    /// Domain-specific DNS configuration.
    pub dns_config: DnsConfig,
    /// Domain-specific database proxy configuration.
    pub database_proxy_config: DatabaseProxyConfig,
    /// DNS cache configuration (derived from dns_config).
    pub dns_cache_config: DnsCacheConfig,
    /// Service registry entries for DNS resolution.
    pub service_registry: HashMap<String, Vec<IpAddr>>,
    /// Custom `/etc/hosts` content for DNS resolution.
    pub etc_hosts_content: String,
    /// Database connection pool configuration (derived from database_proxy_config).
    pub pool_config: PoolConfig,
    /// Environment variables to expose to the guest.
    pub env: HashMap<String, String>,
}

impl Default for ShimConfig {
    fn default() -> Self {
        let db_config = DatabaseProxyConfig::default();
        let dns_config = DnsConfig::default();
        Self {
            filesystem: true,
            dns: true,
            signals: true,
            database_proxy: true,
            threading: true,
            filesystem_config: FilesystemConfig::default(),
            dns_cache_config: dns_config.to_cache_config(),
            dns_config,
            database_proxy_config: db_config.clone(),
            service_registry: HashMap::new(),
            etc_hosts_content: String::new(),
            pool_config: db_config.to_pool_config(),
            env: HashMap::new(),
        }
    }
}

impl ShimConfig {
    /// Create a ShimConfig with all shims enabled (good for development).
    pub fn with_defaults() -> Self {
        Self::default()
    }

    /// Parse a `ShimConfig` from a raw `toml::Value` representing the `[shims]` table.
    ///
    /// If `value` is `None` (missing `[shims]` section), returns the default config
    /// with all shims enabled.
    ///
    /// Unknown shim names produce `tracing::warn` for forward compatibility
    /// rather than returning an error.
    pub fn from_toml(value: Option<&toml::Value>) -> anyhow::Result<Self> {
        let table = match value {
            Some(toml::Value::Table(t)) => t,
            Some(_) => anyhow::bail!("[shims] must be a TOML table"),
            None => return Ok(Self::default()),
        };

        let mut config = Self::default();

        for key in table.keys() {
            if !KNOWN_SHIM_KEYS.contains(&key.as_str()) {
                tracing::warn!(shim_name = %key, "unknown shim name in [shims] config, ignoring");
            }
        }

        // Parse filesystem — accepts bool or table with sub-config
        if let Some(val) = table.get("filesystem") {
            match val {
                toml::Value::Boolean(b) => {
                    config.filesystem = *b;
                }
                toml::Value::Table(t) => {
                    config.filesystem = t
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if let Some(tz) = t.get("timezone_name").and_then(|v| v.as_str()) {
                        config.filesystem_config.timezone_name = tz.to_string();
                    }
                    if let Some(paths) = t.get("extra_virtual_paths")
                        && let Some(paths_table) = paths.as_table()
                    {
                        for (path, content) in paths_table {
                            if let Some(s) = content.as_str() {
                                config
                                    .filesystem_config
                                    .extra_virtual_paths
                                    .insert(path.clone(), s.as_bytes().to_vec());
                            }
                        }
                    }
                }
                _ => anyhow::bail!("shims.filesystem must be a boolean or table"),
            }
        }

        // Parse dns — accepts bool or table with sub-config
        if let Some(val) = table.get("dns") {
            match val {
                toml::Value::Boolean(b) => {
                    config.dns = *b;
                }
                toml::Value::Table(t) => {
                    config.dns = t
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if let Some(ttl) = t.get("ttl_seconds").and_then(|v| v.as_integer()) {
                        config.dns_config.ttl_seconds = ttl as u64;
                    }
                    if let Some(size) = t.get("cache_size").and_then(|v| v.as_integer()) {
                        config.dns_config.cache_size = size as usize;
                    }
                    config.dns_cache_config = config.dns_config.to_cache_config();
                }
                _ => anyhow::bail!("shims.dns must be a boolean or table"),
            }
        }

        // Parse signals — bool only
        if let Some(val) = table.get("signals") {
            config.signals = val
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("shims.signals must be a boolean"))?;
        }

        // Parse database_proxy — accepts bool or table with sub-config
        if let Some(val) = table.get("database_proxy") {
            match val {
                toml::Value::Boolean(b) => {
                    config.database_proxy = *b;
                }
                toml::Value::Table(t) => {
                    config.database_proxy = t
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if let Some(size) = t.get("pool_size").and_then(|v| v.as_integer()) {
                        config.database_proxy_config.pool_size = size as usize;
                    }
                    if let Some(timeout) = t.get("idle_timeout_seconds").and_then(|v| v.as_integer())
                    {
                        config.database_proxy_config.idle_timeout_seconds = timeout as u64;
                    }
                    if let Some(interval) = t
                        .get("health_check_interval_seconds")
                        .and_then(|v| v.as_integer())
                    {
                        config.database_proxy_config.health_check_interval_seconds =
                            interval as u64;
                    }
                    if let Some(timeout) =
                        t.get("connect_timeout_seconds").and_then(|v| v.as_integer())
                    {
                        config.database_proxy_config.connect_timeout_seconds = timeout as u64;
                    }
                    if let Some(timeout) =
                        t.get("recv_timeout_seconds").and_then(|v| v.as_integer())
                    {
                        config.database_proxy_config.recv_timeout_seconds = timeout as u64;
                    }
                    config.pool_config = config.database_proxy_config.to_pool_config();
                }
                _ => anyhow::bail!("shims.database_proxy must be a boolean or table"),
            }
        }

        // Parse threading — bool only
        if let Some(val) = table.get("threading") {
            config.threading = val
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("shims.threading must be a boolean"))?;
        }

        Ok(config)
    }

    /// Create a ShimConfig from a `warp-core::ShimsConfig` (the TOML representation).
    pub fn from_warp_config(
        shims: &warp_core::config::ShimsConfig,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            filesystem: shims.timezone.unwrap_or(true) || shims.dev_urandom.unwrap_or(true),
            dns: shims.dns.unwrap_or(true),
            signals: shims.signals.unwrap_or(true),
            database_proxy: shims.database_proxy.unwrap_or(false),
            threading: shims.threading.is_some(),
            env,
            ..Self::default()
        }
    }

    /// Builder method: set the service registry for DNS.
    pub fn with_service_registry(self, registry: HashMap<String, Vec<IpAddr>>) -> Self {
        Self {
            service_registry: registry,
            ..self
        }
    }

    /// Builder method: set custom `/etc/hosts` content.
    pub fn with_etc_hosts(self, content: String) -> Self {
        Self {
            etc_hosts_content: content,
            ..self
        }
    }

    /// Builder method: set database pool configuration.
    pub fn with_pool_config(self, pool_config: PoolConfig) -> Self {
        Self {
            pool_config,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Default config tests ----

    #[test]
    fn default_config_enables_all_shims() {
        let config = ShimConfig::default();
        assert!(config.filesystem);
        assert!(config.dns);
        assert!(config.signals);
        assert!(config.database_proxy);
        assert!(config.threading);
    }

    #[test]
    fn default_dns_config_has_sensible_values() {
        let config = ShimConfig::default();
        assert_eq!(config.dns_config.ttl_seconds, 30);
        assert_eq!(config.dns_config.cache_size, 1024);
    }

    #[test]
    fn default_filesystem_config_has_utc_timezone() {
        let config = ShimConfig::default();
        assert_eq!(config.filesystem_config.timezone_name, "UTC");
        assert!(config.filesystem_config.extra_virtual_paths.is_empty());
    }

    #[test]
    fn default_database_proxy_config_has_sensible_values() {
        let config = ShimConfig::default();
        assert_eq!(config.database_proxy_config.pool_size, 10);
        assert_eq!(config.database_proxy_config.idle_timeout_seconds, 300);
        assert_eq!(config.database_proxy_config.health_check_interval_seconds, 30);
        assert_eq!(config.database_proxy_config.connect_timeout_seconds, 5);
        assert_eq!(config.database_proxy_config.recv_timeout_seconds, 30);
    }

    // ---- from_toml: missing [shims] section ----

    #[test]
    fn from_toml_none_returns_default() {
        let config = ShimConfig::from_toml(None).unwrap();
        assert!(config.filesystem);
        assert!(config.dns);
        assert!(config.signals);
        assert!(config.database_proxy);
        assert!(config.threading);
    }

    // ---- from_toml: boolean shim toggling ----

    #[test]
    fn from_toml_boolean_fields_toggle_shims() {
        let toml_str = r#"
            filesystem = true
            dns = false
            signals = true
            database_proxy = false
            threading = true
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.filesystem);
        assert!(!config.dns);
        assert!(config.signals);
        assert!(!config.database_proxy);
        assert!(config.threading);
    }

    #[test]
    fn from_toml_partial_fields_default_to_enabled() {
        let toml_str = r#"
            dns = false
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        // Unspecified shims default to enabled
        assert!(config.filesystem);
        assert!(!config.dns);
        assert!(config.signals);
        assert!(config.database_proxy);
        assert!(config.threading);
    }

    // ---- from_toml: domain-specific sub-config ----

    #[test]
    fn from_toml_dns_table_with_sub_config() {
        let toml_str = r#"
            [dns]
            enabled = true
            ttl_seconds = 60
            cache_size = 2048
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.dns);
        assert_eq!(config.dns_config.ttl_seconds, 60);
        assert_eq!(config.dns_config.cache_size, 2048);
    }

    #[test]
    fn from_toml_dns_table_defaults_enabled_to_true() {
        let toml_str = r#"
            [dns]
            ttl_seconds = 15
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.dns);
        assert_eq!(config.dns_config.ttl_seconds, 15);
    }

    #[test]
    fn from_toml_filesystem_table_with_timezone() {
        let toml_str = r#"
            [filesystem]
            enabled = true
            timezone_name = "America/New_York"
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.filesystem);
        assert_eq!(config.filesystem_config.timezone_name, "America/New_York");
    }

    #[test]
    fn from_toml_filesystem_table_with_extra_virtual_paths() {
        let toml_str = r#"
            [filesystem]
            [filesystem.extra_virtual_paths]
            "/etc/myapp.conf" = "key=value"
            "/etc/motd" = "Welcome to WarpGrid"
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.filesystem);
        assert_eq!(
            config.filesystem_config.extra_virtual_paths.get("/etc/myapp.conf"),
            Some(&b"key=value".to_vec())
        );
        assert_eq!(
            config.filesystem_config.extra_virtual_paths.get("/etc/motd"),
            Some(&b"Welcome to WarpGrid".to_vec())
        );
    }

    #[test]
    fn from_toml_database_proxy_table_with_pool_size() {
        let toml_str = r#"
            [database_proxy]
            enabled = true
            pool_size = 20
            idle_timeout_seconds = 600
            connect_timeout_seconds = 10
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.database_proxy);
        assert_eq!(config.database_proxy_config.pool_size, 20);
        assert_eq!(config.database_proxy_config.idle_timeout_seconds, 600);
        assert_eq!(config.database_proxy_config.connect_timeout_seconds, 10);
        // Pool config should be synced
        assert_eq!(config.pool_config.max_size, 20);
        assert_eq!(config.pool_config.idle_timeout, Duration::from_secs(600));
        assert_eq!(config.pool_config.connect_timeout, Duration::from_secs(10));
    }

    #[test]
    fn from_toml_database_proxy_table_disabled() {
        let toml_str = r#"
            [database_proxy]
            enabled = false
            pool_size = 5
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(!config.database_proxy);
        // Sub-config is still parsed even when disabled
        assert_eq!(config.database_proxy_config.pool_size, 5);
    }

    // ---- from_toml: unknown shim names warn but don't error ----

    #[test]
    fn from_toml_unknown_keys_do_not_error() {
        let toml_str = r#"
            filesystem = true
            dns = true
            gpu_acceleration = true
            quantum_entanglement = false
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.filesystem);
        assert!(config.dns);
    }

    // ---- from_toml: error cases ----

    #[test]
    fn from_toml_non_table_value_errors() {
        let value = toml::Value::String("not a table".to_string());
        let result = ShimConfig::from_toml(Some(&value));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be a TOML table"));
    }

    #[test]
    fn from_toml_wrong_type_for_signals_errors() {
        let toml_str = r#"
            signals = "yes"
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let result = ShimConfig::from_toml(Some(&value));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("signals must be a boolean"));
    }

    #[test]
    fn from_toml_wrong_type_for_threading_errors() {
        let toml_str = r#"
            threading = 42
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let result = ShimConfig::from_toml(Some(&value));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("threading must be a boolean"));
    }

    // ---- from_toml: mixed boolean and table forms ----

    #[test]
    fn from_toml_mixed_boolean_and_table_shims() {
        let toml_str = r#"
            filesystem = true
            signals = false
            threading = true
            [dns]
            ttl_seconds = 120
            [database_proxy]
            pool_size = 30
        "#;
        let value: toml::Value = toml::from_str(toml_str).unwrap();
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.filesystem);
        assert!(!config.signals);
        assert!(config.threading);
        assert!(config.dns);
        assert_eq!(config.dns_config.ttl_seconds, 120);
        assert!(config.database_proxy);
        assert_eq!(config.database_proxy_config.pool_size, 30);
    }

    // ---- from_toml: empty table ----

    #[test]
    fn from_toml_empty_table_returns_default() {
        let value = toml::Value::Table(toml::map::Map::new());
        let config = ShimConfig::from_toml(Some(&value)).unwrap();

        assert!(config.filesystem);
        assert!(config.dns);
        assert!(config.signals);
        assert!(config.database_proxy);
        assert!(config.threading);
    }

    // ---- from_warp_config (legacy path) ----

    #[test]
    fn from_warp_config_maps_fields() {
        let shims = warp_core::config::ShimsConfig {
            timezone: Some(false),
            dev_urandom: Some(true),
            dns: Some(true),
            threading: None,
            signals: Some(false),
            database_proxy: Some(true),
        };
        let env = HashMap::from([("DB_HOST".to_string(), "localhost".to_string())]);

        let config = ShimConfig::from_warp_config(&shims, env.clone());

        // filesystem enabled because dev_urandom is true
        assert!(config.filesystem);
        assert!(config.dns);
        assert!(!config.signals);
        assert!(config.database_proxy);
        assert!(!config.threading); // threading is None
        assert_eq!(config.env, env);
    }

    // ---- Builder methods ----

    #[test]
    fn builder_methods_chain() {
        let config = ShimConfig::default()
            .with_etc_hosts("127.0.0.1 myhost".to_string())
            .with_service_registry(HashMap::from([(
                "api.local".to_string(),
                vec!["10.0.0.1".parse().unwrap()],
            )]));

        assert_eq!(config.etc_hosts_content, "127.0.0.1 myhost");
        assert!(config.service_registry.contains_key("api.local"));
    }

    // ---- DatabaseProxyConfig -> PoolConfig conversion ----

    #[test]
    fn database_proxy_config_converts_to_pool_config() {
        let db_config = DatabaseProxyConfig {
            pool_size: 25,
            idle_timeout_seconds: 120,
            health_check_interval_seconds: 15,
            connect_timeout_seconds: 3,
            recv_timeout_seconds: 45,
        };
        let pool = db_config.to_pool_config();

        assert_eq!(pool.max_size, 25);
        assert_eq!(pool.idle_timeout, Duration::from_secs(120));
        assert_eq!(pool.health_check_interval, Duration::from_secs(15));
        assert_eq!(pool.connect_timeout, Duration::from_secs(3));
        assert_eq!(pool.recv_timeout, Duration::from_secs(45));
    }
}
