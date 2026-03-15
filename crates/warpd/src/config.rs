//! Configuration file support for warpd.
//!
//! Loads settings from a TOML config file, with CLI args and env vars
//! taking precedence. Config file is searched in order:
//!
//! 1. `--config <path>` (explicit)
//! 2. `./warpgrid.toml` (current directory)
//! 3. `$data_dir/warpgrid.toml`
//! 4. `/etc/warpgrid/warpgrid.toml`
//!
//! Example config:
//!
//! ```toml
//! [cloud]
//! api_port = 8443
//! data_dir = "/var/lib/warpgrid"
//! edge_regions = "iad,ams,sin"
//! metrics_interval = 60
//!
//! [cloud.turso]
//! url = "libsql://my-db.turso.io"
//! auth_token = "eyJhbGci..."
//!
//! [cloud.fly]
//! api_token = "fo1_..."
//!
//! [cloud.registry]
//! bucket = "warpgrid-registry"
//!
//! [cloud.posthog]
//! api_key = "phc_..."
//!
//! [cloud.stripe]
//! secret_key = "sk_live_..."
//! ```

use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Debug, Default, Deserialize)]
pub struct WarpGridConfig {
    #[serde(default)]
    pub cloud: CloudConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct CloudConfig {
    pub api_port: Option<u16>,
    pub data_dir: Option<PathBuf>,
    pub edge_regions: Option<String>,
    pub metrics_interval: Option<u64>,

    #[serde(default)]
    pub turso: TursoConfig,

    #[serde(default)]
    pub fly: FlyConfig,

    #[serde(default)]
    pub registry: RegistryConfig,

    #[serde(default)]
    pub posthog: PosthogConfig,

    #[serde(default)]
    pub stripe: StripeConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct TursoConfig {
    pub url: Option<String>,
    pub auth_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FlyConfig {
    pub api_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RegistryConfig {
    pub bucket: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PosthogConfig {
    pub api_key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StripeConfig {
    pub secret_key: Option<String>,
    pub webhook_secret: Option<String>,
}

impl WarpGridConfig {
    /// Load config from the first found config file.
    /// Returns default config if no file is found (not an error).
    pub fn load(explicit_path: Option<&Path>, data_dir: Option<&Path>) -> Self {
        let candidates = config_candidates(explicit_path, data_dir);

        for path in &candidates {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(content) => match toml::from_str::<WarpGridConfig>(&content) {
                        Ok(config) => {
                            info!(path = %path.display(), "loaded config file");
                            return config;
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "failed to parse config file, using defaults"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to read config file"
                        );
                    }
                }
            }
        }

        Self::default()
    }
}

/// Generate ordered list of config file candidates.
fn config_candidates(explicit: Option<&Path>, data_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(p) = explicit {
        paths.push(p.to_path_buf());
    }

    paths.push(PathBuf::from("warpgrid.toml"));

    if let Some(d) = data_dir {
        paths.push(d.join("warpgrid.toml"));
    }

    paths.push(PathBuf::from("/etc/warpgrid/warpgrid.toml"));

    paths
}

/// Merge: CLI arg > env var > config file > default.
/// Returns the first non-None value.
pub fn merge_option<T>(cli: Option<T>, config: Option<T>) -> Option<T> {
    cli.or(config)
}

/// Merge with a default value.
pub fn merge_with_default<T>(cli: T, config: Option<T>, default: T) -> T
where
    T: PartialEq,
{
    // If CLI value differs from its declared default, CLI wins.
    // Otherwise, config file wins if present.
    // This is a simplification — in practice clap handles this via `is_present`.
    if config.is_some() {
        // If CLI is at its default, prefer config.
        if cli == default {
            config.unwrap_or(default)
        } else {
            cli
        }
    } else {
        cli
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let toml = r#"
[cloud]
api_port = 9443
data_dir = "/data/warpgrid"
edge_regions = "iad,ams,sin"
metrics_interval = 30

[cloud.turso]
url = "libsql://test.turso.io"
auth_token = "token123"

[cloud.fly]
api_token = "fo1_test"

[cloud.registry]
bucket = "my-bucket"

[cloud.posthog]
api_key = "phc_test"

[cloud.stripe]
secret_key = "sk_test_123"
"#;

        let config: WarpGridConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.cloud.api_port, Some(9443));
        assert_eq!(config.cloud.data_dir, Some(PathBuf::from("/data/warpgrid")));
        assert_eq!(config.cloud.edge_regions.as_deref(), Some("iad,ams,sin"));
        assert_eq!(
            config.cloud.turso.url.as_deref(),
            Some("libsql://test.turso.io")
        );
        assert_eq!(config.cloud.turso.auth_token.as_deref(), Some("token123"));
        assert_eq!(config.cloud.fly.api_token.as_deref(), Some("fo1_test"));
        assert_eq!(config.cloud.posthog.api_key.as_deref(), Some("phc_test"));
        assert_eq!(
            config.cloud.stripe.secret_key.as_deref(),
            Some("sk_test_123")
        );
    }

    #[test]
    fn empty_config_returns_defaults() {
        let config: WarpGridConfig = toml::from_str("").unwrap();
        assert_eq!(config.cloud.api_port, None);
        assert!(config.cloud.turso.url.is_none());
    }

    #[test]
    fn partial_config() {
        let toml = r#"
[cloud]
api_port = 3000

[cloud.turso]
url = "libsql://db.turso.io"
"#;
        let config: WarpGridConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.cloud.api_port, Some(3000));
        assert_eq!(
            config.cloud.turso.url.as_deref(),
            Some("libsql://db.turso.io")
        );
        assert!(config.cloud.turso.auth_token.is_none());
        assert!(config.cloud.fly.api_token.is_none());
    }

    #[test]
    fn load_returns_default_when_no_file() {
        let config = WarpGridConfig::load(None, None);
        assert!(config.cloud.api_port.is_none());
    }

    #[test]
    fn merge_option_prefers_cli() {
        assert_eq!(merge_option(Some(42), Some(10)), Some(42));
        assert_eq!(merge_option(None, Some(10)), Some(10));
        assert_eq!(merge_option::<i32>(None, None), None);
    }

    #[test]
    fn config_candidates_order() {
        let paths = config_candidates(None, Some(Path::new("/data")));
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], PathBuf::from("warpgrid.toml"));
        assert_eq!(paths[1], PathBuf::from("/data/warpgrid.toml"));
        assert_eq!(paths[2], PathBuf::from("/etc/warpgrid/warpgrid.toml"));
    }

    #[test]
    fn config_candidates_with_explicit() {
        let paths = config_candidates(Some(Path::new("/custom/config.toml")), None);
        assert_eq!(paths[0], PathBuf::from("/custom/config.toml"));
    }
}
