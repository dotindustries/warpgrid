pub mod rust;
pub mod go;
pub mod typescript;
pub mod dockerfile;

use anyhow::{Result, bail};
use std::path::Path;

/// Detect project language from files present.
pub fn detect_language(path: &Path) -> Result<String> {
    if path.join("Cargo.toml").exists() {
        Ok("rust".to_string())
    } else if path.join("go.mod").exists() {
        Ok("go".to_string())
    } else if path.join("package.json").exists() {
        Ok("typescript".to_string())
    } else if path.join("Dockerfile").exists() {
        dockerfile::detect_language_from_dockerfile(&path.join("Dockerfile"))
    } else {
        bail!("Could not detect project language. No Cargo.toml, go.mod, package.json, or Dockerfile found.")
    }
}
