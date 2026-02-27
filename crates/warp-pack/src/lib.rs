//! warp pack — compile and package Wasm components.
//!
//! Phase 1: wraps cargo-component, TinyGo, and ComponentizeJS.
//! Phase 2: adds Bun compilation via bun build + jco componentize.

use anyhow::{Result, bail};
use sha2::{Sha256, Digest};
use std::path::Path;
use warp_core::WarpConfig;

mod bun;
pub mod go;

#[derive(Debug)]
pub struct PackResult {
    pub output_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Pack a project using the language specified in `warp.toml`.
pub fn pack(project_path: &Path) -> Result<PackResult> {
    pack_with_lang(project_path, None)
}

/// Pack a project, optionally overriding the language from `warp.toml`.
///
/// When `lang_override` is `Some`, the specified language is used instead of
/// reading `[build].lang` from `warp.toml`. The `warp.toml` is still required
/// for other fields like `[build].entry` and `[package].name`.
pub fn pack_with_lang(project_path: &Path, lang_override: Option<&str>) -> Result<PackResult> {
    let config = WarpConfig::from_file(&project_path.join("warp.toml"))?;
    let lang = lang_override
        .or_else(|| config.build.as_ref().map(|b| b.lang.as_str()))
        .unwrap_or("unknown");

    match lang {
        "rust" => pack_rust(project_path, &config),
        "go" => go::pack_go(project_path, &config),
        "typescript" => pack_typescript(project_path, &config),
        "bun" => bun::pack_bun(project_path, &config),
        _ => bail!("Unsupported language: {lang}. Supported: rust, go, typescript, bun"),
    }
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


fn pack_typescript(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    // TODO: Invoke ComponentizeJS
    bail!("TypeScript packaging not yet implemented. Requires ComponentizeJS.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_project_with_lang(lang: &str, entry: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_path_buf();

        let warp_toml = format!(
            r#"
[package]
name = "test-handler"
version = "0.1.0"

[build]
lang = "{lang}"
entry = "{entry}"
"#
        );
        fs::write(project.join("warp.toml"), warp_toml).unwrap();
        fs::write(project.join(entry), "package main\n\nfunc main() {}\n").unwrap();

        (dir, project)
    }

    #[test]
    fn test_pack_with_lang_override_routes_to_go() {
        // warp.toml says "rust", but --lang go should override
        let (_dir, project) = create_project_with_lang("rust", "main.go");

        let result = pack_with_lang(&project, Some("go"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Should reach Go pipeline (TinyGo error), not "Rust packaging not yet implemented"
        assert!(
            !err_msg.contains("Rust packaging"),
            "Expected Go pipeline, got Rust stub: {err_msg}"
        );
        assert!(
            !err_msg.contains("Unsupported language"),
            "Go should be supported: {err_msg}"
        );
    }

    #[test]
    fn test_pack_with_lang_none_uses_config() {
        // No override → uses warp.toml lang
        let (_dir, project) = create_project_with_lang("go", "main.go");

        let result = pack_with_lang(&project, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Should reach Go pipeline, not "Unsupported language"
        assert!(
            !err_msg.contains("Unsupported language"),
            "Go should be supported via config: {err_msg}"
        );
    }

    #[test]
    fn test_pack_with_lang_unsupported() {
        let (_dir, project) = create_project_with_lang("go", "main.go");

        let result = pack_with_lang(&project, Some("python"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unsupported language: python"),
            "Expected unsupported language error: {err_msg}"
        );
    }

    #[test]
    fn test_pack_delegates_to_pack_with_lang() {
        // pack() should be equivalent to pack_with_lang(_, None)
        let (_dir, project) = create_project_with_lang("go", "main.go");

        let result = pack(&project);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("Unsupported language"),
            "pack() should route Go correctly: {err_msg}"
        );
    }
}
