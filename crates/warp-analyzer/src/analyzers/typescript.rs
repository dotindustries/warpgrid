//! TypeScript/JavaScript analyzer — parses package.json.

use anyhow::Result;
use std::path::Path;
use warp_core::DependencyVerdict;

pub fn analyze_package_json(project_path: &Path) -> Result<Vec<DependencyVerdict>> {
    let pkg_path = project_path.join("package.json");
    if !pkg_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&pkg_path)?;
    let pkg: serde_json::Value = serde_json::from_str(&content)?;
    let mut deps = Vec::new();

    for section in ["dependencies", "devDependencies"] {
        if let Some(dep_obj) = pkg.get(section).and_then(|d| d.as_object()) {
            for (name, version) in dep_obj {
                deps.push(DependencyVerdict {
                    name: name.clone(),
                    version: version.as_str().map(String::from),
                    verdict: warp_core::Verdict::Unknown,
                });
            }
        }
    }

    tracing::info!(count = deps.len(), "Parsed TypeScript dependencies");
    Ok(deps)
}
