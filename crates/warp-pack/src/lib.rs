//! warp pack — compile and package Wasm components.
//!
//! Supports multiple language targets:
//! - `rust` — cargo-component (stub)
//! - `go` — TinyGo (stub)
//! - `typescript` / `js` — ComponentizeJS via jco
//! - `bun` — Bun → jco pipeline (stub)

use anyhow::{Result, bail};
use std::path::Path;
use warp_core::WarpConfig;

mod js;

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
        "typescript" | "js" => js::pack_js(project_path, &config),
        "bun" => pack_bun(project_path, &config),
        _ => bail!("Unsupported language: {lang}. Supported: rust, go, typescript, js, bun"),
    }
}

fn pack_rust(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    bail!("Rust packaging not yet implemented. Requires cargo-component.")
}

fn pack_go(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    bail!("Go packaging not yet implemented. Requires TinyGo.")
}

fn pack_bun(_path: &Path, _config: &WarpConfig) -> Result<PackResult> {
    bail!("Bun packaging not yet implemented. Requires @warpgrid/bun-sdk.")
}
