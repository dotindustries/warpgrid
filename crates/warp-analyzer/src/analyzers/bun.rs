//! Bun analyzer — parses package.json and detects bun.lockb.

use anyhow::Result;
use std::path::Path;
use warp_core::DependencyVerdict;

/// Analyze a Bun project by parsing `package.json` and detecting `bun.lockb`.
///
/// Returns a list of dependency verdicts extracted from `dependencies`
/// and `devDependencies` sections of `package.json`.
pub fn analyze_package_json(project_path: &Path) -> Result<Vec<DependencyVerdict>> {
    let pkg_path = project_path.join("package.json");
    if !pkg_path.exists() {
        return Ok(vec![]);
    }

    let has_lockb = project_path.join("bun.lockb").exists();
    if has_lockb {
        tracing::info!("Found bun.lockb — confirmed Bun project");
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

    tracing::info!(count = deps.len(), has_lockb, "Parsed Bun dependencies");
    Ok(deps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_package_json(dir: &Path, content: &str) {
        fs::write(dir.join("package.json"), content).unwrap();
    }

    #[test]
    fn test_parse_dependencies_and_dev_dependencies() {
        let tmp = TempDir::new().unwrap();
        create_package_json(
            tmp.path(),
            r#"{
                "name": "test-app",
                "dependencies": {
                    "hono": "4.7.4",
                    "drizzle-orm": "0.39.3"
                },
                "devDependencies": {
                    "typescript": "5.0.0"
                }
            }"#,
        );

        let deps = analyze_package_json(tmp.path()).unwrap();
        assert_eq!(deps.len(), 3);

        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"hono"));
        assert!(names.contains(&"drizzle-orm"));
        assert!(names.contains(&"typescript"));
    }

    #[test]
    fn test_empty_dependencies() {
        let tmp = TempDir::new().unwrap();
        create_package_json(
            tmp.path(),
            r#"{
                "name": "empty-app",
                "dependencies": {},
                "devDependencies": {}
            }"#,
        );

        let deps = analyze_package_json(tmp.path()).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_no_package_json() {
        let tmp = TempDir::new().unwrap();
        let deps = analyze_package_json(tmp.path()).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_missing_sections() {
        let tmp = TempDir::new().unwrap();
        create_package_json(tmp.path(), r#"{"name": "minimal"}"#);

        let deps = analyze_package_json(tmp.path()).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_version_extracted() {
        let tmp = TempDir::new().unwrap();
        create_package_json(
            tmp.path(),
            r#"{
                "dependencies": {
                    "hono": "^4.7.4"
                }
            }"#,
        );

        let deps = analyze_package_json(tmp.path()).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "hono");
        assert_eq!(deps[0].version.as_deref(), Some("^4.7.4"));
    }

    #[test]
    fn test_bun_lockb_detection() {
        let tmp = TempDir::new().unwrap();
        create_package_json(
            tmp.path(),
            r#"{"dependencies": {"hono": "4.7.4"}}"#,
        );
        // Create a bun.lockb file (binary, but we only check existence)
        fs::write(tmp.path().join("bun.lockb"), b"\x00binary").unwrap();

        let deps = analyze_package_json(tmp.path()).unwrap();
        assert_eq!(deps.len(), 1);
        // The function should succeed regardless — lockb is informational
    }

    #[test]
    fn test_scoped_package_names() {
        let tmp = TempDir::new().unwrap();
        create_package_json(
            tmp.path(),
            r#"{
                "dependencies": {
                    "@sinclair/typebox": "0.34.14",
                    "@hono/node-server": "1.0.0"
                }
            }"#,
        );

        let deps = analyze_package_json(tmp.path()).unwrap();
        assert_eq!(deps.len(), 2);
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"@sinclair/typebox"));
        assert!(names.contains(&"@hono/node-server"));
    }
}
