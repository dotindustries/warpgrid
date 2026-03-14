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
    use std::collections::BTreeSet;

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
        assert!(target.join("wit/world.wit").exists());
        assert!(target.join("wit/async-handler.wit").exists());
        assert!(target.join("wit/http-types.wit").exists());
    }

    #[test]
    fn test_scaffold_async_go() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("my-project");
        crate::templates::scaffold("async-go", &target).unwrap();
        assert!(target.join("go.mod").exists());
        assert!(target.join("main.go").exists());
        assert!(target.join("main_test.go").exists());
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
        assert!(target.join("wit/handler.wit").exists());
        assert!(target.join("wit/deps/http/types.wit").exists());
        assert!(target.join("wit/deps/shim/dns.wit").exists());
    }

    // ── Template ↔ Fixture consistency tests ─────────────────────
    //
    // These tests ensure the embedded template content stays in sync with
    // the fixture directories that integration tests build. A drift between
    // template and fixture means integration tests validate stale code.

    /// Collect all relative file paths under `dir`, excluding build artifacts.
    fn collect_files(dir: &std::path::Path) -> BTreeSet<String> {
        let mut files = BTreeSet::new();
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let rel = entry
                .path()
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .to_string();
            // Skip build artifacts that aren't part of the template
            if rel.starts_with("target/") || rel == "Cargo.lock" {
                continue;
            }
            files.insert(rel);
        }
        files
    }

    /// Normalize fixture content so it can be compared to template output.
    ///
    /// Fixtures use their directory name as the project name (e.g.
    /// "async-rust-template") whereas templates use the placeholder
    /// "my-async-handler". The Go fixture also has a local `replace`
    /// directive needed for workspace builds that the template omits.
    fn normalize_fixture_content(
        content: &str,
        fixture_name: &str,
    ) -> String {
        content
            .replace(fixture_name, "my-async-handler")
            // Go fixture has a local replace directive for workspace builds
            .replace(
                "\nreplace github.com/anthropics/warpgrid/packages/warpgrid-go => ../../../packages/warpgrid-go\n",
                "",
            )
    }

    /// Scaffold a template and compare every generated file against the
    /// corresponding fixture file. Fails if content differs or if the
    /// fixture has files the template doesn't produce (or vice versa).
    ///
    /// Known differences (project name, local replace directives) are
    /// normalized before comparison.
    fn assert_template_matches_fixture(template_name: &str, fixture_subdir: &str) {
        let dir = tempfile::tempdir().unwrap();
        let scaffolded = dir.path().join("project");
        crate::templates::scaffold(template_name, &scaffolded).unwrap();

        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures")
            .join(fixture_subdir);

        let scaffolded_files = collect_files(&scaffolded);
        let fixture_files = collect_files(&fixture_dir);

        // Check for files in fixture but missing from template
        let missing_from_template: BTreeSet<_> =
            fixture_files.difference(&scaffolded_files).collect();
        assert!(
            missing_from_template.is_empty(),
            "Fixture '{fixture_subdir}' has files not produced by template '{template_name}': {missing_from_template:?}"
        );

        // Check for files in template but missing from fixture
        let missing_from_fixture: BTreeSet<_> =
            scaffolded_files.difference(&fixture_files).collect();
        assert!(
            missing_from_fixture.is_empty(),
            "Template '{template_name}' produces files not in fixture '{fixture_subdir}': {missing_from_fixture:?}"
        );

        // Compare content of every file (normalizing known differences)
        for file in &scaffolded_files {
            let scaffolded_content =
                std::fs::read_to_string(scaffolded.join(file)).unwrap();
            let fixture_content =
                std::fs::read_to_string(fixture_dir.join(file)).unwrap();
            let normalized_fixture =
                normalize_fixture_content(&fixture_content, fixture_subdir);
            assert_eq!(
                scaffolded_content, normalized_fixture,
                "Content mismatch in '{file}' between template '{template_name}' and fixture '{fixture_subdir}'"
            );
        }
    }

    #[test]
    fn test_async_rust_template_matches_fixture() {
        assert_template_matches_fixture("async-rust", "async-rust-template");
    }

    #[test]
    fn test_async_go_template_matches_fixture() {
        assert_template_matches_fixture("async-go", "async-go-template");
    }

    #[test]
    fn test_async_ts_template_matches_fixture() {
        assert_template_matches_fixture("async-ts", "async-ts-template");
    }
}
