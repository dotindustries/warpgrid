//! Go project analyzer — parses go.mod.

use anyhow::Result;
use regex::Regex;
use std::path::Path;
use warp_core::DependencyVerdict;

pub fn analyze_go_mod(project_path: &Path) -> Result<Vec<DependencyVerdict>> {
    let go_mod_path = project_path.join("go.mod");
    if !go_mod_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&go_mod_path)?;
    let dep_re = Regex::new(r"^\s+(\S+)\s+(v\S+)")?;
    let mut deps = Vec::new();
    let mut in_require = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("require (") || trimmed == "require (" {
            in_require = true;
            continue;
        }
        if trimmed == ")" {
            in_require = false;
            continue;
        }
        if in_require {
            if let Some(caps) = dep_re.captures(line) {
                deps.push(DependencyVerdict {
                    name: caps[1].to_string(),
                    version: Some(caps[2].to_string()),
                    verdict: warp_core::Verdict::Unknown,
                });
            }
        }
    }

    tracing::info!(count = deps.len(), "Parsed Go dependencies");
    Ok(deps)
}
