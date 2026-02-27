//! warp pack — compile and package Wasm components.
//!
//! Phase 1: wraps cargo-component, TinyGo, and ComponentizeJS.
//! Phase 2: adds Bun compilation via bun build + jco componentize.

use anyhow::{Result, bail};
use sha2::{Sha256, Digest};
use std::path::Path;
use tracing::info;
use warp_core::WarpConfig;

mod bun;

/// Supported languages for `warp pack`.
pub const SUPPORTED_LANGUAGES: &[&str] = &["rust", "go", "typescript", "bun"];

#[derive(Debug)]
pub struct PackResult {
    pub output_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Pack a project at the given path, reading language from `warp.toml`.
pub fn pack(project_path: &Path) -> Result<PackResult> {
    pack_with_lang(project_path, None)
}

/// Pack a project with an optional language override.
///
/// If `lang_override` is `Some`, it takes precedence over `warp.toml`.
/// If `warp.toml` has no `[build].lang`, the language is auto-detected
/// from project marker files (e.g., `bunfig.toml` → bun).
pub fn pack_with_lang(project_path: &Path, lang_override: Option<&str>) -> Result<PackResult> {
    let config = WarpConfig::from_file(&project_path.join("warp.toml"))?;

    let lang = if let Some(override_lang) = lang_override {
        override_lang.to_string()
    } else {
        match config.build.as_ref().map(|b| b.lang.as_str()) {
            Some(l) if !l.is_empty() => l.to_string(),
            _ => detect_language(project_path)?,
        }
    };

    match lang.as_str() {
        "rust" => pack_rust(project_path, &config),
        "go" => pack_go(project_path, &config),
        "typescript" => pack_typescript(project_path, &config),
        "bun" => bun::pack_bun(project_path, &config),
        _ => bail!(
            "Unsupported language: '{lang}'. Supported: {}",
            SUPPORTED_LANGUAGES.join(", ")
        ),
    }
}

/// Auto-detect language from project marker files.
///
/// Detection order:
/// 1. `bunfig.toml` → bun
/// 2. `Cargo.toml` (non-workspace) → rust
/// 3. `go.mod` → go
/// 4. `package.json` → typescript
fn detect_language(project_path: &Path) -> Result<String> {
    if project_path.join("bunfig.toml").exists() {
        info!("Auto-detected language: bun (found bunfig.toml)");
        return Ok("bun".to_string());
    }

    let cargo_toml = project_path.join("Cargo.toml");
    if cargo_toml.exists()
        && let Ok(content) = std::fs::read_to_string(&cargo_toml)
        && !content.contains("[workspace]")
    {
        info!("Auto-detected language: rust (found Cargo.toml)");
        return Ok("rust".to_string());
    }

    if project_path.join("go.mod").exists() {
        info!("Auto-detected language: go (found go.mod)");
        return Ok("go".to_string());
    }

    if project_path.join("package.json").exists() {
        info!("Auto-detected language: typescript (found package.json)");
        return Ok("typescript".to_string());
    }

    bail!(
        "Cannot auto-detect language. No marker files found \
         (bunfig.toml, Cargo.toml, go.mod, package.json). \
         Either add [build].lang to warp.toml or use --lang."
    )
}

/// Compute SHA-256 hash of a file and return the hex digest.
pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let hash = Sha256::digest(&bytes);
    Ok(hex::encode(hash))
}

fn pack_rust(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    // TODO: Invoke cargo-component
    // cargo component build --release --target wasm32-wasip2
    bail!("Rust packaging not yet implemented. Requires cargo-component.")
}

fn pack_go(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    // TODO: Invoke TinyGo or Go 1.24+
    // tinygo build -target=wasip2 -o output.wasm
    bail!("Go packaging not yet implemented. Requires TinyGo.")
}

fn pack_typescript(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    // TODO: Invoke ComponentizeJS
    bail!("TypeScript packaging not yet implemented. Requires ComponentizeJS.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_warp_toml(dir: &Path, lang: Option<&str>) {
        let build_section = match lang {
            Some(l) => format!("\n[build]\nlang = \"{l}\"\nentry = \"src/index.ts\""),
            None => String::new(),
        };
        let content = format!(
            "[package]\nname = \"test\"\nversion = \"0.1.0\"{build_section}"
        );
        fs::write(dir.join("warp.toml"), content).unwrap();
    }

    #[test]
    fn supported_languages_includes_bun() {
        assert!(
            SUPPORTED_LANGUAGES.contains(&"bun"),
            "bun should be in SUPPORTED_LANGUAGES"
        );
        assert!(
            SUPPORTED_LANGUAGES.contains(&"rust"),
            "rust should be in SUPPORTED_LANGUAGES"
        );
        assert!(
            SUPPORTED_LANGUAGES.contains(&"go"),
            "go should be in SUPPORTED_LANGUAGES"
        );
        assert!(
            SUPPORTED_LANGUAGES.contains(&"typescript"),
            "typescript should be in SUPPORTED_LANGUAGES"
        );
    }

    #[test]
    fn auto_detect_bun_from_bunfig_toml() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bunfig.toml"), "[install]\n").unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_ok(), "Should detect bun: {:?}", result.err());
        assert_eq!(result.unwrap(), "bun");
    }

    #[test]
    fn auto_detect_go_from_go_mod() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/test\n").unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "go");
    }

    #[test]
    fn auto_detect_typescript_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name": "test", "version": "1.0.0"}"#,
        ).unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "typescript");
    }

    #[test]
    fn auto_detect_rust_from_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"",
        ).unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "rust");
    }

    #[test]
    fn auto_detect_skips_workspace_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]",
        ).unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_err(), "Workspace Cargo.toml should not auto-detect as rust");
    }

    #[test]
    fn auto_detect_bunfig_takes_priority_over_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bunfig.toml"), "").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "bun", "bunfig.toml should take priority");
    }

    #[test]
    fn auto_detect_fails_with_no_markers() {
        let dir = tempfile::tempdir().unwrap();

        let result = detect_language(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cannot auto-detect"), "Error: {err}");
        assert!(err.contains("--lang"), "Should suggest --lang: {err}");
    }

    #[test]
    fn pack_with_bunfig_auto_detection() {
        let dir = tempfile::tempdir().unwrap();
        write_warp_toml(dir.path(), None);
        fs::write(dir.path().join("bunfig.toml"), "").unwrap();

        // Auto-detect should route to bun, which will fail because
        // entry point doesn't exist — but it should NOT say "Unsupported language"
        let result = pack(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            !err.contains("Unsupported language"),
            "Should auto-detect bun, not fail on language: {err}"
        );
    }

    #[test]
    fn pack_with_lang_override() {
        let dir = tempfile::tempdir().unwrap();
        // warp.toml says typescript, but we override with bun
        write_warp_toml(dir.path(), Some("typescript"));
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/index.ts"), "export default {}").unwrap();

        let result = pack_with_lang(dir.path(), Some("bun"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Should try to pack as bun (entry point validation), not typescript
        assert!(
            !err.contains("Unsupported language"),
            "Should use bun override: {err}"
        );
    }

    #[test]
    fn unsupported_language_error_lists_all_supported() {
        let dir = tempfile::tempdir().unwrap();
        write_warp_toml(dir.path(), Some("python"));

        let result = pack(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported language"), "Error: {err}");
        assert!(err.contains("bun"), "Should list bun in error: {err}");
        assert!(err.contains("rust"), "Should list rust in error: {err}");
        assert!(err.contains("go"), "Should list go in error: {err}");
        assert!(err.contains("typescript"), "Should list typescript in error: {err}");
    }
}
