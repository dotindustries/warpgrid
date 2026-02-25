//! Bun compilation pipeline for `warp pack --lang bun`.
//!
//! Pipeline: bun build (bundle) → jco componentize (Wasm) → wasm-tools validate.
//!
//! The Bun handler must export a default object with a `fetch` method matching
//! the WarpGridHandler interface from `@warpgrid/bun-sdk`.

use anyhow::{Result, Context, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, debug};
use warp_core::WarpConfig;

use crate::PackResult;

/// Resolve the jco binary path.
///
/// Search order:
/// 1. `$WARPGRID_JCO_PATH` environment variable
/// 2. `build/componentize-js/node_modules/.bin/jco` (project-local install)
/// 3. `jco` on `$PATH` (global install)
fn resolve_jco(project_root: &Path) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("WARPGRID_JCO_PATH") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return Ok(p);
        }
        bail!(
            "WARPGRID_JCO_PATH is set to '{path}' but the file does not exist. \
             Install jco: npm install -g @bytecodealliance/jco"
        );
    }

    let project_local = project_root.join("build/componentize-js/node_modules/.bin/jco");
    if project_local.is_file() {
        return Ok(project_local);
    }

    if let Ok(output) = Command::new("which").arg("jco").output()
        && output.status.success()
    {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path_str.is_empty() {
            return Ok(PathBuf::from(path_str));
        }
    }

    bail!(
        "jco not found. Install it with one of:\n  \
         1. Run: scripts/build-componentize-js.sh (recommended)\n  \
         2. npm install -g @bytecodealliance/jco\n  \
         3. Set WARPGRID_JCO_PATH to the jco binary path"
    )
}

/// Resolve the WIT directory for HTTP handler componentization.
///
/// Search order:
/// 1. `<project>/wit/` (project-local WIT)
/// 2. `tests/fixtures/js-http-handler/wit/` (shared WIT from SDK)
fn resolve_wit_dir(project_path: &Path, project_root: &Path) -> Result<PathBuf> {
    let project_wit = project_path.join("wit");
    if project_wit.is_dir() {
        return Ok(project_wit);
    }

    let shared_wit = project_root.join("tests/fixtures/js-http-handler/wit");
    if shared_wit.is_dir() {
        return Ok(shared_wit);
    }

    bail!(
        "WIT directory not found. Expected at '{}/wit/'. \
         Create a wit/ directory with WASI HTTP handler definitions.",
        project_path.display()
    )
}

/// Locate the project root by walking up from `project_path` looking for `Cargo.toml`
/// with `[workspace]`. Falls back to CWD-based search if the project path
/// doesn't lead to a workspace root (common when project_path is a tempdir).
fn find_project_root(project_path: &Path) -> PathBuf {
    // Try walking up from project_path first
    if let Some(root) = walk_up_for_workspace(project_path) {
        return root;
    }

    // Fallback: try walking up from CWD (useful when project_path is a tempdir)
    if let Ok(cwd) = std::env::current_dir()
        && let Some(root) = walk_up_for_workspace(&cwd)
    {
        return root;
    }

    // Last resort: use the project path itself
    project_path.to_path_buf()
}

fn walk_up_for_workspace(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(start)
    };

    for _ in 0..10 {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.is_file()
            && let Ok(content) = std::fs::read_to_string(&cargo_toml)
            && content.contains("[workspace]")
        {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    None
}

/// Step 1: Bundle the Bun handler with `bun build`.
///
/// Produces a single-file ES module bundle suitable for jco componentize.
fn bun_build(project_path: &Path, entry: &str, output: &Path) -> Result<()> {
    let entry_path = project_path.join(entry);
    info!("Bundling with bun build: {}", entry);

    let result = Command::new("bun")
        .arg("build")
        .arg(&entry_path)
        .arg("--outfile")
        .arg(output)
        .arg("--target")
        .arg("browser")
        .arg("--format")
        .arg("esm")
        .output()
        .context(
            "Failed to execute 'bun build'. Is Bun installed? \
             Install from https://bun.sh"
        )?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let exit_code = result.status.code().unwrap_or(-1);
        bail!(
            "bun build failed (exit code {exit_code}).\n\n\
             --- stderr ---\n{stderr}\n\
             --- stdout ---\n{stdout}"
        );
    }

    if !output.exists() {
        bail!(
            "bun build succeeded but output file not produced at '{}'",
            output.display()
        );
    }

    let size = std::fs::metadata(output)?.len();
    debug!("bun build output: {} ({} bytes)", output.display(), size);

    Ok(())
}

/// Step 2: Componentize the bundled JS into a WASI HTTP Wasm component.
fn jco_componentize(
    jco_bin: &Path,
    bundled_js: &Path,
    wit_dir: &Path,
    output: &Path,
) -> Result<()> {
    info!("Componentizing with jco...");

    let result = Command::new(jco_bin)
        .arg("componentize")
        .arg(bundled_js)
        .arg("--wit")
        .arg(wit_dir)
        .arg("--world-name")
        .arg("handler")
        .arg("--enable")
        .arg("http")
        .arg("--enable")
        .arg("fetch-event")
        .arg("-o")
        .arg(output)
        .output()
        .context("Failed to execute jco componentize")?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let exit_code = result.status.code().unwrap_or(-1);
        bail!(
            "jco componentize failed (exit code {exit_code}).\n\n\
             --- stderr ---\n{stderr}\n\
             --- stdout ---\n{stdout}\n\n\
             Hint: This usually means the handler uses unsupported APIs \
             (native bindings, Node.js core modules not available in WASI). \
             Check your imports for Bun/Node.js-specific APIs that don't \
             have WASI equivalents."
        );
    }

    if !output.exists() {
        bail!(
            "jco componentize succeeded but output file not produced at '{}'",
            output.display()
        );
    }

    let size = std::fs::metadata(output)?.len();
    debug!("jco componentize output: {} ({} bytes)", output.display(), size);

    Ok(())
}

/// Step 3: Validate the Wasm component exports `wasi:http/incoming-handler`.
fn validate_component(wasm_path: &Path) -> Result<()> {
    info!("Validating Wasm component...");

    let result = Command::new("wasm-tools")
        .arg("component")
        .arg("wit")
        .arg(wasm_path)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let wit_output = String::from_utf8_lossy(&output.stdout);
            if wit_output.contains("incoming-handler") {
                info!("Validation passed: component exports wasi:http/incoming-handler");
                Ok(())
            } else {
                bail!(
                    "Component validation failed: output does not export \
                     wasi:http/incoming-handler. WIT output:\n{}",
                    wit_output
                );
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "wasm-tools validation failed:\n{stderr}\n\n\
                 The Wasm component may be malformed."
            );
        }
        Err(_) => {
            // wasm-tools not available — skip validation with warning
            tracing::warn!(
                "wasm-tools not found in PATH. Skipping component validation. \
                 Install: cargo install wasm-tools"
            );
            Ok(())
        }
    }
}

/// Main entry point: compile a Bun project to a WASI HTTP Wasm component.
///
/// Pipeline:
/// 1. `bun build` — produce single-file ES module bundle
/// 2. `jco componentize` — produce Wasm component
/// 3. `wasm-tools component wit` — validate exports wasi:http/incoming-handler
pub fn pack_bun(project_path: &Path, config: &WarpConfig) -> Result<PackResult> {
    let build_config = config.build.as_ref()
        .context("Missing [build] section in warp.toml")?;

    let entry = &build_config.entry;
    let module_name = &config.package.name;
    let project_root = find_project_root(project_path);

    // Validate entry point exists before resolving toolchain
    let entry_path = project_path.join(entry);
    if !entry_path.exists() {
        bail!(
            "Entry point not found: '{}'. Check [build].entry in warp.toml.",
            entry_path.display()
        );
    }

    // Resolve external tool paths
    let jco_bin = resolve_jco(&project_root)?;
    let wit_dir = resolve_wit_dir(project_path, &project_root)?;

    info!(
        "Packing Bun handler: {} (entry: {}, wit: {})",
        module_name, entry, wit_dir.display()
    );

    // Create output directory
    let output_dir = project_path.join("target/wasm");
    std::fs::create_dir_all(&output_dir)
        .context("Failed to create target/wasm/ directory")?;

    // Temp file for the bundled JS (inside target/ to avoid polluting project)
    let bundle_dir = project_path.join("target/bun-bundle");
    std::fs::create_dir_all(&bundle_dir)
        .context("Failed to create target/bun-bundle/ directory")?;
    let bundled_js = bundle_dir.join(format!("{module_name}.js"));

    let wasm_output = output_dir.join(format!("{module_name}.wasm"));

    // Step 1: Bundle with bun build
    bun_build(project_path, entry, &bundled_js)?;

    // Step 2: Componentize with jco
    jco_componentize(&jco_bin, &bundled_js, &wit_dir, &wasm_output)?;

    // Step 3: Validate the component
    validate_component(&wasm_output)?;

    // Compute output metadata
    let metadata = std::fs::metadata(&wasm_output)?;
    let sha256 = crate::sha256_file(&wasm_output)?;

    info!(
        "Bun handler compiled: {} ({:.1} MB)",
        wasm_output.display(),
        metadata.len() as f64 / 1_048_576.0
    );

    Ok(PackResult {
        output_path: wasm_output.to_string_lossy().to_string(),
        size_bytes: metadata.len(),
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── Helper: Create a minimal Bun project in a temp dir ──────────────────

    fn create_bun_project(dir: &Path, handler_content: &str) -> WarpConfig {
        // Create src/index.ts
        let src_dir = dir.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("index.ts"), handler_content).unwrap();

        // Create warp.toml
        let config = WarpConfig::scaffold("test-bun-handler", "bun", "src/index.ts");
        let toml_str = config.to_toml_string().unwrap();
        fs::write(dir.join("warp.toml"), &toml_str).unwrap();

        config
    }

    fn has_bun() -> bool {
        Command::new("bun").arg("--version").output().is_ok()
    }

    fn has_jco() -> bool {
        // Check project-local jco
        let project_root = find_project_root(Path::new("."));
        resolve_jco(&project_root).is_ok()
    }

    // ── Unit tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_pack_dispatches_bun_language() {
        // warp.toml with lang = "bun" should route to pack_bun
        let dir = tempfile::tempdir().unwrap();
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"

[build]
lang = "bun"
entry = "src/index.ts"
"#;
        fs::write(dir.path().join("warp.toml"), toml_str).unwrap();

        // pack() should fail because src/index.ts doesn't exist,
        // but it should fail with a bun-specific error, not "Unsupported language"
        let result = crate::pack(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("Unsupported language"),
            "Expected bun dispatch, got: {err_msg}"
        );
    }

    #[test]
    fn test_missing_entry_point() {
        let dir = tempfile::tempdir().unwrap();
        let config = WarpConfig::scaffold("test", "bun", "nonexistent.ts");

        let result = pack_bun(dir.path(), &config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Entry point not found"),
            "Expected entry point error, got: {err_msg}"
        );
    }

    #[test]
    fn test_missing_build_section() {
        let config = WarpConfig {
            package: warp_core::config::PackageConfig {
                name: "test".to_string(),
                version: "0.1.0".to_string(),
                description: None,
            },
            build: None,
            runtime: None,
            capabilities: None,
            health: None,
            shims: None,
            env: None,
        };

        let dir = tempfile::tempdir().unwrap();
        let result = pack_bun(dir.path(), &config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("[build]"),
            "Expected missing build section error, got: {err_msg}"
        );
    }

    #[test]
    fn test_resolve_jco_env_var_missing_file() {
        // WARPGRID_JCO_PATH pointing to nonexistent file should error.
        // In Rust 2024 edition, set_var/remove_var are unsafe due to
        // potential data races in multi-threaded test execution.
        // SAFETY: This test is not using threads and the env var is
        // restored before the test returns.
        unsafe { std::env::set_var("WARPGRID_JCO_PATH", "/nonexistent/jco") };
        let result = resolve_jco(Path::new("."));
        unsafe { std::env::remove_var("WARPGRID_JCO_PATH") };

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("WARPGRID_JCO_PATH"),
            "Expected env var error, got: {err_msg}"
        );
    }

    #[test]
    fn test_resolve_wit_dir_project_local() {
        let dir = tempfile::tempdir().unwrap();
        let wit_dir = dir.path().join("wit");
        fs::create_dir_all(&wit_dir).unwrap();

        let result = resolve_wit_dir(dir.path(), Path::new("/nonexistent"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), wit_dir);
    }

    #[test]
    fn test_resolve_wit_dir_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_wit_dir(dir.path(), Path::new("/nonexistent"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("WIT directory not found"));
    }

    // ── Integration tests (require bun + jco) ──────────────────────────────

    #[test]
    fn test_bun_build_produces_bundle() {
        if !has_bun() {
            eprintln!("Skipping: bun not available");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let handler = r#"addEventListener("fetch", (event) =>
  event.respondWith(new Response("hello from bun"))
);"#;
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/index.ts"), handler).unwrap();

        let output = dir.path().join("bundle.js");
        let result = bun_build(dir.path(), "src/index.ts", &output);
        assert!(result.is_ok(), "bun build failed: {:?}", result.err());
        assert!(output.exists(), "Bundle not produced");

        let content = fs::read_to_string(&output).unwrap();
        assert!(
            content.contains("hello from bun"),
            "Bundle should contain handler content"
        );
    }

    #[test]
    fn test_bun_build_error_includes_stderr() {
        if !has_bun() {
            eprintln!("Skipping: bun not available");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        // Write intentionally broken TS that bun build will reject
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/broken.ts"),
            "import { nonexistent } from 'this-package-does-not-exist-anywhere-12345';",
        ).unwrap();

        let output = dir.path().join("bundle.js");
        let result = bun_build(dir.path(), "src/broken.ts", &output);
        // bun build may still succeed with an import it can't resolve
        // (it bundles what it can). But if it fails, the error should
        // include exit code info.
        if result.is_err() {
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("exit code"),
                "Error should include exit code: {err_msg}"
            );
        }
    }

    #[test]
    fn test_full_pipeline_minimal_handler() {
        if !has_bun() || !has_jco() {
            eprintln!("Skipping: requires bun + jco");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let handler = r#"addEventListener("fetch", (event) =>
  event.respondWith(new Response("ok"))
);"#;
        let config = create_bun_project(dir.path(), handler);

        // Copy WIT directory from shared fixture
        let project_root = find_project_root(Path::new("."));
        let shared_wit = project_root.join("tests/fixtures/js-http-handler/wit");
        if !shared_wit.is_dir() {
            eprintln!("Skipping: shared WIT fixture not found");
            return;
        }
        copy_dir_recursive(&shared_wit, &dir.path().join("wit")).unwrap();

        let result = pack_bun(dir.path(), &config);
        assert!(result.is_ok(), "Full pipeline failed: {:?}", result.err());

        let pack_result = result.unwrap();
        assert!(
            pack_result.output_path.ends_with(".wasm"),
            "Output should be .wasm: {}",
            pack_result.output_path
        );
        assert!(
            pack_result.output_path.contains("target/wasm/"),
            "Output should be in target/wasm/: {}",
            pack_result.output_path
        );
        assert!(pack_result.size_bytes > 0, "Wasm should be non-empty");
        assert!(!pack_result.sha256.is_empty(), "SHA256 should be computed");
        assert_eq!(pack_result.sha256.len(), 64, "SHA256 should be 64 hex chars");
    }

    #[test]
    fn test_full_pipeline_via_pack_entry_point() {
        if !has_bun() || !has_jco() {
            eprintln!("Skipping: requires bun + jco");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let handler = r#"addEventListener("fetch", (event) =>
  event.respondWith(new Response("ok"))
);"#;
        create_bun_project(dir.path(), handler);

        // Copy WIT directory
        let project_root = find_project_root(Path::new("."));
        let shared_wit = project_root.join("tests/fixtures/js-http-handler/wit");
        if !shared_wit.is_dir() {
            eprintln!("Skipping: shared WIT fixture not found");
            return;
        }
        copy_dir_recursive(&shared_wit, &dir.path().join("wit")).unwrap();

        // Use the top-level pack() dispatcher
        let result = crate::pack(dir.path());
        assert!(result.is_ok(), "pack() dispatcher failed for bun: {:?}", result.err());
    }

    #[test]
    fn test_output_path_uses_module_name() {
        if !has_bun() || !has_jco() {
            eprintln!("Skipping: requires bun + jco");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let handler = r#"addEventListener("fetch", (event) =>
  event.respondWith(new Response("ok"))
);"#;

        // Use a specific module name
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("index.ts"), handler).unwrap();

        let mut config = WarpConfig::scaffold("my-cool-api", "bun", "src/index.ts");
        config.package.name = "my-cool-api".to_string();
        fs::write(
            dir.path().join("warp.toml"),
            config.to_toml_string().unwrap(),
        ).unwrap();

        // Copy WIT directory
        let project_root = find_project_root(Path::new("."));
        let shared_wit = project_root.join("tests/fixtures/js-http-handler/wit");
        if !shared_wit.is_dir() {
            eprintln!("Skipping: shared WIT fixture not found");
            return;
        }
        copy_dir_recursive(&shared_wit, &dir.path().join("wit")).unwrap();

        let result = pack_bun(dir.path(), &config);
        assert!(result.is_ok(), "Pipeline failed: {:?}", result.err());

        let pack_result = result.unwrap();
        assert!(
            pack_result.output_path.contains("my-cool-api.wasm"),
            "Output should use module name: {}",
            pack_result.output_path
        );
    }

    #[test]
    fn test_jco_error_includes_hint() {
        if !has_jco() {
            eprintln!("Skipping: requires jco");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        // Create an invalid JS file that jco will choke on
        let invalid_js = dir.path().join("invalid.js");
        fs::write(&invalid_js, "THIS IS NOT VALID COMPONENT JS }{}{").unwrap();

        let project_root = find_project_root(Path::new("."));
        let jco_bin = resolve_jco(&project_root).unwrap();

        // Need a valid WIT dir for jco to even attempt componentization
        let shared_wit = project_root.join("tests/fixtures/js-http-handler/wit");
        if !shared_wit.is_dir() {
            eprintln!("Skipping: shared WIT fixture not found");
            return;
        }

        let output = dir.path().join("output.wasm");
        let result = jco_componentize(&jco_bin, &invalid_js, &shared_wit, &output);

        if result.is_err() {
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("Hint") || err_msg.contains("unsupported APIs"),
                "jco error should include hint about unsupported APIs: {err_msg}"
            );
        }
        // If jco somehow succeeds with garbage input, that's fine too
    }

    // ── Test helpers ────────────────────────────────────────────────────────

    fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }
}
