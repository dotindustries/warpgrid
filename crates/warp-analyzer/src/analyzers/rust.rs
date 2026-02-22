//! Rust project analyzer — parses Cargo.toml for dependencies.

use anyhow::Result;
use std::path::Path;
use warp_core::DependencyVerdict;

pub fn analyze_cargo_toml(project_path: &Path) -> Result<Vec<DependencyVerdict>> {
    let cargo_path = project_path.join("Cargo.toml");
    if !cargo_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&cargo_path)?;
    let manifest: toml::Value = toml::from_str(&content)?;

    let mut deps = Vec::new();

    if let Some(dep_table) = manifest.get("dependencies").and_then(|d| d.as_table()) {
        for (name, value) in dep_table {
            let version = match value {
                toml::Value::String(v) => Some(v.clone()),
                toml::Value::Table(t) => t.get("version").and_then(|v| v.as_str()).map(String::from),
                _ => None,
            };
            deps.push(DependencyVerdict {
                name: name.clone(),
                version,
                verdict: warp_core::Verdict::Unknown, // Will be resolved by compat DB
            });
        }
    }

    // Also check dev-dependencies and build-dependencies
    for section in ["dev-dependencies", "build-dependencies"] {
        if let Some(dep_table) = manifest.get(section).and_then(|d| d.as_table()) {
            for (name, value) in dep_table {
                let version = match value {
                    toml::Value::String(v) => Some(v.clone()),
                    toml::Value::Table(t) => t.get("version").and_then(|v| v.as_str()).map(String::from),
                    _ => None,
                };
                deps.push(DependencyVerdict {
                    name: name.clone(),
                    version,
                    verdict: warp_core::Verdict::Unknown,
                });
            }
        }
    }

    tracing::info!(count = deps.len(), "Parsed Rust dependencies");
    Ok(deps)
}
