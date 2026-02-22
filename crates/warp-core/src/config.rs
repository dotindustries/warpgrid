//! warp.toml configuration parser.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarpConfig {
    pub package: PackageConfig,
    pub build: Option<BuildConfig>,
    pub runtime: Option<RuntimeConfig>,
    pub capabilities: Option<HashMap<String, toml::Value>>,
    pub health: Option<HealthConfig>,
    pub shims: Option<ShimsConfig>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub lang: String,
    pub entry: String,
    pub target: Option<String>,
    pub flags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub trigger: Option<String>,
    pub min_instances: Option<u32>,
    pub max_instances: Option<u32>,
    pub resources: Option<ResourcesConfig>,
    pub scaling: Option<ScalingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesConfig {
    pub memory_limit: Option<String>,
    pub cpu_weight: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingConfig {
    pub metric: Option<String>,
    pub target_value: Option<u32>,
    pub scale_up_window: Option<String>,
    pub scale_down_window: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    pub endpoint: Option<String>,
    pub interval: Option<String>,
    pub timeout: Option<String>,
    pub unhealthy_threshold: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShimsConfig {
    pub timezone: Option<bool>,
    pub dev_urandom: Option<bool>,
    pub dns: Option<bool>,
    pub threading: Option<String>,
    pub signals: Option<bool>,
    pub database_proxy: Option<bool>,
}

impl WarpConfig {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: WarpConfig = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn to_toml_string(&self) -> anyhow::Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Scaffold a minimal warp.toml for the given language.
    pub fn scaffold(name: &str, lang: &str, entry: &str) -> Self {
        WarpConfig {
            package: PackageConfig {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                description: None,
            },
            build: Some(BuildConfig {
                lang: lang.to_string(),
                entry: entry.to_string(),
                target: Some("wasip2".to_string()),
                flags: None,
            }),
            runtime: Some(RuntimeConfig {
                trigger: Some("http".to_string()),
                min_instances: Some(1),
                max_instances: Some(10),
                resources: None,
                scaling: None,
            }),
            capabilities: None,
            health: Some(HealthConfig {
                endpoint: Some("/healthz".to_string()),
                interval: Some("5s".to_string()),
                timeout: Some("2s".to_string()),
                unhealthy_threshold: Some(3),
            }),
            shims: None,
            env: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scaffold() {
        let config = WarpConfig::scaffold("my-api", "rust", "src/main.rs");
        let toml_str = config.to_toml_string().unwrap();
        assert!(toml_str.contains("my-api"));
        assert!(toml_str.contains("rust"));
    }

    #[test]
    fn test_parse_minimal() {
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"
"#;
        let config: WarpConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.package.name, "test");
    }
}
