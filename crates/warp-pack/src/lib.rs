//! warp pack — compile and package Wasm components.
//!
//! Phase 1: wraps cargo-component, TinyGo, and ComponentizeJS.
//! Phase 2: adds Bun compilation via bun build + jco componentize.

use anyhow::{Result, bail};
use sha2::{Sha256, Digest};
use std::path::Path;
use warp_core::WarpConfig;

mod bun;

#[derive(Debug)]
pub struct PackResult {
    pub output_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

pub fn pack(project_path: &Path) -> Result<PackResult> {
    let config = WarpConfig::from_file(&project_path.join("warp.toml"))?;
    let lang = config.build.as_ref()
        .map(|b| b.lang.as_str())
        .unwrap_or("unknown");

    match lang {
        "rust" => pack_rust(project_path, &config),
        "go" => pack_go(project_path, &config),
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

fn pack_go(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    // TODO: Invoke TinyGo or Go 1.24+
    // tinygo build -target=wasip2 -o output.wasm
    bail!("Go packaging not yet implemented. Requires TinyGo.")
}

fn pack_typescript(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    // TODO: Invoke ComponentizeJS
    bail!("TypeScript packaging not yet implemented. Requires ComponentizeJS.")
}
