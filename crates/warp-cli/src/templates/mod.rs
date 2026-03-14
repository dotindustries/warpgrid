mod async_go;
mod async_rust;
mod async_ts;

use std::fs;
use std::path::Path;

use anyhow::{bail, Result};

/// A single file to write during scaffolding.
struct TemplateFile {
    /// Relative path within the project directory.
    path: &'static str,
    /// File content.
    content: &'static str,
}

/// Scaffold a template project into `target_dir`.
///
/// `target_dir` must not already exist — this function creates it.
pub fn scaffold(template_name: &str, target_dir: &Path) -> Result<()> {
    let files = match template_name {
        "async-rust" => async_rust::files(),
        "async-go" => async_go::files(),
        "async-ts" => async_ts::files(),
        _ => bail!(
            "Unknown template '{template_name}'. Available templates: async-rust, async-go, async-ts"
        ),
    };

    for file in &files {
        let dest = target_dir.join(file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, file.content)?;
    }

    Ok(())
}
