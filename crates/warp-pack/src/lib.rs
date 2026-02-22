//! warp pack — compile and package Wasm components.
//!
//! Phase 1: wraps cargo-component, TinyGo, and ComponentizeJS.
//! This is a stub — actual compilation integration comes with
//! Milestone 1.4 of the Wedge phase.

use anyhow::{Result, bail};
use std::path::Path;
use warp_core::WarpConfig;

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
        _ => bail!("Unsupported language: {lang}"),
    }
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
