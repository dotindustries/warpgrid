pub mod bun;
pub mod rust;
pub mod go;
pub mod typescript;
pub mod dockerfile;

use anyhow::{Result, bail};
use std::path::Path;

/// Detect project language from files present.
///
/// `bunfig.toml` takes priority over `package.json` — a project with both
/// is detected as `"bun"`, while `package.json` alone maps to `"typescript"`.
pub fn detect_language(path: &Path) -> Result<String> {
    if path.join("Cargo.toml").exists() {
        Ok("rust".to_string())
    } else if path.join("go.mod").exists() {
        Ok("go".to_string())
    } else if path.join("bunfig.toml").exists() {
        Ok("bun".to_string())
    } else if path.join("package.json").exists() {
        Ok("typescript".to_string())
    } else if path.join("Dockerfile").exists() {
        dockerfile::detect_language_from_dockerfile(&path.join("Dockerfile"))
    } else {
        bail!("Could not detect project language. No Cargo.toml, go.mod, bunfig.toml, package.json, or Dockerfile found.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_bun_from_bunfig_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("bunfig.toml"), "[install]\n").unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();

        let lang = detect_language(tmp.path()).unwrap();
        assert_eq!(lang, "bun");
    }

    #[test]
    fn test_bunfig_takes_priority_over_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("bunfig.toml"), "").unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();

        // bunfig.toml should win over package.json → "bun" not "typescript"
        let lang = detect_language(tmp.path()).unwrap();
        assert_eq!(lang, "bun");
    }

    #[test]
    fn test_package_json_without_bunfig_is_typescript() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), "{}").unwrap();

        let lang = detect_language(tmp.path()).unwrap();
        assert_eq!(lang, "typescript");
    }

    #[test]
    fn test_detect_rust() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();

        let lang = detect_language(tmp.path()).unwrap();
        assert_eq!(lang, "rust");
    }

    #[test]
    fn test_detect_go() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("go.mod"), "module test").unwrap();

        let lang = detect_language(tmp.path()).unwrap();
        assert_eq!(lang, "go");
    }

    #[test]
    fn test_no_marker_files_errors() {
        let tmp = TempDir::new().unwrap();
        let result = detect_language(tmp.path());
        assert!(result.is_err());
    }
}
