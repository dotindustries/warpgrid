use std::path::Path;

use anyhow::{bail, Result};
use tracing::info;

use crate::templates;

pub fn init(template: &str, path: Option<&str>) -> Result<()> {
    let target_dir = match path {
        Some(p) => p.to_string(),
        None => format!("./{template}"),
    };

    let target = Path::new(&target_dir);
    if target.exists() {
        bail!(
            "Target directory '{}' already exists. Remove it or choose a different path with --path.",
            target_dir
        );
    }

    info!("Scaffolding template '{template}' into {target_dir}");
    templates::scaffold(template, target)?;
    println!("✓ Project scaffolded at {target_dir}");
    println!("  cd {target_dir} && warp pack");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_template() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out");
        let result = crate::templates::scaffold("no-such-template", &target);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown template")
        );
    }

    #[test]
    fn test_target_exists() {
        let dir = tempfile::tempdir().unwrap();
        // dir already exists, so init should fail
        let result = init("async-rust", Some(dir.path().to_str().unwrap()));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
    }

    #[test]
    fn test_scaffold_async_rust() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("my-project");
        crate::templates::scaffold("async-rust", &target).unwrap();
        assert!(target.join("Cargo.toml").exists());
        assert!(target.join("src/lib.rs").exists());
        assert!(target.join("warp.toml").exists());
        assert!(target.join("README.md").exists());
    }

    #[test]
    fn test_scaffold_async_go() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("my-project");
        crate::templates::scaffold("async-go", &target).unwrap();
        assert!(target.join("go.mod").exists());
        assert!(target.join("main.go").exists());
        assert!(target.join("warp.toml").exists());
        assert!(target.join("README.md").exists());
    }

    #[test]
    fn test_scaffold_async_ts() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("my-project");
        crate::templates::scaffold("async-ts", &target).unwrap();
        assert!(target.join("package.json").exists());
        assert!(target.join("src/handler.ts").exists());
        assert!(target.join("warp.toml").exists());
        assert!(target.join("README.md").exists());
    }
}
