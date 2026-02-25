//! WarpGrid Bun WASI runtime support.
//!
//! This crate provides the compilation pipeline and host-side support for
//! running Bun TypeScript workloads as WASI components on WarpGrid.
//!
//! The compilation pipeline is: `bun build` (single-file bundle) →
//! `jco componentize` (Wasm component) → WIT validation.

/// Configuration for the Bun compilation pipeline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BunPipelineConfig {
    /// Path to the bun binary. Defaults to `bun` on PATH.
    pub bun_path: String,
    /// Path to the jco binary. Defaults to `jco` on PATH.
    pub jco_path: String,
    /// Whether to inject WarpGrid polyfills during bundling.
    pub inject_polyfills: bool,
}

impl Default for BunPipelineConfig {
    fn default() -> Self {
        Self {
            bun_path: "bun".to_string(),
            jco_path: "jco".to_string(),
            inject_polyfills: true,
        }
    }
}

/// Validates that a Bun pipeline configuration has resolvable tool paths.
///
/// Returns `Ok(())` if the config is structurally valid, or an error
/// describing which fields are problematic.
pub fn validate_config(config: &BunPipelineConfig) -> anyhow::Result<()> {
    if config.bun_path.is_empty() {
        anyhow::bail!("bun_path must not be empty");
    }
    if config.jco_path.is_empty() {
        anyhow::bail!("jco_path must not be empty");
    }
    tracing::debug!(
        bun_path = %config.bun_path,
        jco_path = %config.jco_path,
        inject_polyfills = config.inject_polyfills,
        "Bun pipeline config validated"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = BunPipelineConfig::default();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn empty_bun_path_rejected() {
        let config = BunPipelineConfig {
            bun_path: String::new(),
            ..BunPipelineConfig::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("bun_path"));
    }

    #[test]
    fn empty_jco_path_rejected() {
        let config = BunPipelineConfig {
            jco_path: String::new(),
            ..BunPipelineConfig::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("jco_path"));
    }

    #[test]
    fn config_serializes_to_json() {
        let config = BunPipelineConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: BunPipelineConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.bun_path, config.bun_path);
        assert_eq!(roundtrip.jco_path, config.jco_path);
        assert_eq!(roundtrip.inject_polyfills, config.inject_polyfills);
    }
}
