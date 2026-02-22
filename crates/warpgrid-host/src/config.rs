//! ShimConfig — deployment spec parsing.
//!
//! Parses WarpGrid deployment specifications into shim configuration:
//! virtual filesystem entries, DNS overrides, database pool settings,
//! signal handlers, and threading model.

use std::collections::HashMap;
use std::net::IpAddr;

use crate::db_proxy::PoolConfig;

/// Host-side shim configuration for a single Wasm instance.
///
/// Built from a `warp-core::ShimsConfig` (the user-facing TOML config)
/// and enriched with deployment-specific data (service registry, env vars).
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// Enable timezone emulation (virtual `/usr/share/zoneinfo/`).
    pub timezone: bool,
    /// Enable `/dev/urandom` virtual file.
    pub dev_urandom: bool,
    /// Enable DNS resolution shim.
    pub dns: bool,
    /// Enable signal delivery shim.
    pub signals: bool,
    /// Enable database proxy shim.
    pub database_proxy: bool,
    /// Timezone to use if `timezone` is enabled (default: "UTC").
    pub timezone_name: String,
    /// Service registry entries for DNS resolution.
    pub service_registry: HashMap<String, Vec<IpAddr>>,
    /// Custom `/etc/hosts` content for DNS resolution.
    pub etc_hosts_content: String,
    /// Database connection pool configuration.
    pub pool_config: PoolConfig,
    /// Environment variables to expose to the guest.
    pub env: HashMap<String, String>,
}

impl Default for ShimConfig {
    fn default() -> Self {
        Self {
            timezone: true,
            dev_urandom: true,
            dns: true,
            signals: true,
            database_proxy: false,
            timezone_name: "UTC".to_string(),
            service_registry: HashMap::new(),
            etc_hosts_content: String::new(),
            pool_config: PoolConfig::default(),
            env: HashMap::new(),
        }
    }
}

impl ShimConfig {
    /// Create a ShimConfig with all shims enabled (good for development).
    pub fn with_defaults() -> Self {
        Self::default()
    }

    /// Create a ShimConfig from a `warp-core::ShimsConfig` (the TOML representation).
    pub fn from_warp_config(
        shims: &warp_core::config::ShimsConfig,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            timezone: shims.timezone.unwrap_or(true),
            dev_urandom: shims.dev_urandom.unwrap_or(true),
            dns: shims.dns.unwrap_or(true),
            signals: shims.signals.unwrap_or(true),
            database_proxy: shims.database_proxy.unwrap_or(false),
            env,
            ..Self::default()
        }
    }

    /// Builder method: set the service registry for DNS.
    pub fn with_service_registry(
        self,
        registry: HashMap<String, Vec<IpAddr>>,
    ) -> Self {
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

    #[test]
    fn default_config_enables_core_shims() {
        let config = ShimConfig::default();
        assert!(config.timezone);
        assert!(config.dev_urandom);
        assert!(config.dns);
        assert!(config.signals);
        assert!(!config.database_proxy);
    }

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

        assert!(!config.timezone);
        assert!(config.dev_urandom);
        assert!(config.dns);
        assert!(!config.signals);
        assert!(config.database_proxy);
        assert_eq!(config.env, env);
    }

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
}
