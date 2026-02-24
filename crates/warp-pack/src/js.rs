//! JavaScript/TypeScript packaging via ComponentizeJS (jco).
//!
//! Pipeline:
//! 1. Locate jco binary at `build/componentize-js/node_modules/.bin/jco`
//! 2. Locate handler entry point from `warp.toml` build.entry
//! 3. Generate a WarpGrid prelude (warpgrid global, process.env polyfill)
//! 4. Create a combined handler = prelude + user handler
//! 5. Ensure WIT directory exists with required interfaces
//! 6. Invoke `jco componentize` to produce a Wasm component
//! 7. Validate output, compute size + SHA256, return PackResult

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};
use warp_core::WarpConfig;

use crate::PackResult;

/// Locate the jco binary relative to the project root.
fn find_jco(project_root: &Path) -> Result<PathBuf> {
    let jco_path = project_root
        .join("build")
        .join("componentize-js")
        .join("node_modules")
        .join(".bin")
        .join("jco");

    if jco_path.is_file() {
        debug!("Found jco at {}", jco_path.display());
        return Ok(jco_path);
    }

    // Try project-local node_modules (for projects that install jco themselves)
    let local_jco = project_root
        .join("node_modules")
        .join(".bin")
        .join("jco");

    if local_jco.is_file() {
        debug!("Found jco at {} (project-local)", local_jco.display());
        return Ok(local_jco);
    }

    bail!(
        "ComponentizeJS (jco) not found.\n\
         \n\
         Expected at: {}\n\
         \n\
         To install, run:\n\
         \n\
         \x20 scripts/build-componentize-js.sh\n\
         \n\
         Or install globally:\n\
         \n\
         \x20 npm install -g @bytecodealliance/jco@1.16.1",
        jco_path.display()
    )
}

/// Walk upward from `project_path` to find the SDK root (where `build/` and `scripts/` live).
///
/// The SDK root is detected by the presence of `build/componentize-js/` or `scripts/build-componentize-js.sh`.
/// If no SDK root is found, falls back to the project path itself.
fn find_sdk_root(project_path: &Path) -> PathBuf {
    let mut candidate = project_path.to_path_buf();
    for _ in 0..10 {
        if candidate.join("build").join("componentize-js").exists()
            || candidate.join("scripts").join("build-componentize-js.sh").exists()
        {
            return candidate;
        }
        if !candidate.pop() {
            break;
        }
    }
    project_path.to_path_buf()
}

/// Generate the WarpGrid shim prelude that injects global objects.
///
/// This prelude is prepended to the user's handler source before componentization.
/// It provides:
/// - `globalThis.warpgrid.database` — connect/send/recv/close (from WIT imports)
/// - `globalThis.warpgrid.dns` — resolve (from WIT imports)
/// - `globalThis.warpgrid.fs` — readFile (from WIT imports)
/// - `globalThis.process.env` — environment variable access
fn generate_prelude(config: &WarpConfig) -> String {
    let shims = config.shims.as_ref();
    let db_enabled = shims.is_none_or(|s| s.database_proxy.unwrap_or(true));
    let dns_enabled = shims.is_none_or(|s| s.dns.unwrap_or(true));

    let mut prelude = String::from(
        "// ── WarpGrid Shim Prelude (auto-injected by warp pack --lang js) ──\n",
    );

    // process.env polyfill — always injected
    prelude.push_str(
        r#"
if (typeof globalThis.process === "undefined") {
  globalThis.process = {};
}
if (typeof globalThis.process.env === "undefined") {
  globalThis.process.env = {};
}
"#,
    );

    // warpgrid global namespace
    prelude.push_str(
        r#"
if (typeof globalThis.warpgrid === "undefined") {
  globalThis.warpgrid = {};
}
"#,
    );

    // Database shim bridge
    if db_enabled {
        prelude.push_str(
            r#"
// Database proxy shim — bridges WIT imports to warpgrid.database global
try {
  const dbProxy = await import("warpgrid:shim/database-proxy@0.1.0");
  globalThis.warpgrid.database = {
    connect: dbProxy.connect,
    send: dbProxy.send,
    recv: dbProxy.recv,
    close: dbProxy.close,
  };
} catch (_e) {
  // Shim not available — warpgrid.database will be undefined
}
"#,
        );
    }

    // DNS shim bridge
    if dns_enabled {
        prelude.push_str(
            r#"
// DNS shim — bridges WIT imports to warpgrid.dns global
try {
  const dnsShim = await import("warpgrid:shim/dns@0.1.0");
  globalThis.warpgrid.dns = {
    resolve: dnsShim.resolveAddress,
  };
} catch (_e) {
  // Shim not available — warpgrid.dns will be undefined
}
"#,
        );
    }

    // Filesystem shim bridge
    prelude.push_str(
        r#"
// Filesystem shim — bridges WIT imports to warpgrid.fs global
try {
  const fsShim = await import("warpgrid:shim/filesystem@0.1.0");
  globalThis.warpgrid.fs = {
    readFile: async (path, encoding) => {
      const handle = fsShim.openVirtual(path);
      const data = fsShim.readVirtual(handle, 1048576);
      fsShim.closeVirtual(handle);
      if (encoding === "utf-8" || encoding === "utf8") {
        return new TextDecoder().decode(new Uint8Array(data));
      }
      return new Uint8Array(data);
    },
  };
} catch (_e) {
  // Shim not available — warpgrid.fs will be undefined
}
"#,
    );

    prelude.push_str("// ── End WarpGrid Shim Prelude ──\n\n");
    prelude
}

/// Detect whether the handler source uses WarpGrid shim imports that require
/// the corresponding WIT interfaces in the world definition.
struct ShimUsage {
    uses_database: bool,
    uses_dns: bool,
    uses_filesystem: bool,
}

fn detect_shim_usage(source: &str) -> ShimUsage {
    ShimUsage {
        uses_database: source.contains("warpgrid:shim/database-proxy")
            || source.contains("warpgrid.database"),
        uses_dns: source.contains("warpgrid:shim/dns")
            || source.contains("warpgrid.dns"),
        uses_filesystem: source.contains("warpgrid:shim/filesystem")
            || source.contains("warpgrid.fs"),
    }
}

/// The main JS/TS packaging function invoked by `warp pack --lang js`.
pub fn pack_js(project_path: &Path, config: &WarpConfig) -> Result<PackResult> {
    // Validate project structure first (before toolchain checks) for better error messages
    let build = config
        .build
        .as_ref()
        .context("Missing [build] section in warp.toml")?;
    let entry_path = project_path.join(&build.entry);
    if !entry_path.is_file() {
        bail!(
            "Entry point not found: {}\n\
             Check [build] entry in warp.toml",
            entry_path.display()
        );
    }

    // Locate WIT directory — prefer project-local, fall back to src/wit/
    let wit_dir = resolve_wit_dir(project_path)?;

    // Now check the toolchain
    let sdk_root = find_sdk_root(project_path);
    let jco_path = find_jco(&sdk_root)?;

    info!("Packaging JS/TS handler: {}", entry_path.display());

    // Read the user's handler source
    let handler_source = fs::read_to_string(&entry_path)
        .with_context(|| format!("Failed to read {}", entry_path.display()))?;

    // Generate prelude and combine with handler
    let prelude = generate_prelude(config);
    let combined_source = format!("{prelude}{handler_source}");

    // Detect which shims are used
    let usage = detect_shim_usage(&combined_source);

    // Create output directory
    let dist_dir = project_path.join("dist");
    fs::create_dir_all(&dist_dir)?;

    // Write combined handler to a temp file in the project
    let combined_path = dist_dir.join(".handler-combined.js");
    fs::write(&combined_path, &combined_source)?;

    // Determine world name from WIT files or default
    let world_name = detect_world_name(&wit_dir).unwrap_or_else(|| "handler".to_string());

    info!(
        "Componentizing with world '{}', WIT dir: {}",
        world_name,
        wit_dir.display()
    );

    // Build jco command
    let output_path = dist_dir.join("handler.wasm");
    let mut cmd = Command::new(&jco_path);
    cmd.arg("componentize")
        .arg(&combined_path)
        .arg("--wit")
        .arg(&wit_dir)
        .arg("--world-name")
        .arg(&world_name)
        .arg("--enable")
        .arg("http")
        .arg("--enable")
        .arg("fetch-event")
        .arg("-o")
        .arg(&output_path);

    debug!("Running: {:?}", cmd);

    let output = cmd
        .output()
        .context("Failed to execute jco. Is Node.js installed?")?;

    // Clean up combined handler (ignore errors)
    let _ = fs::remove_file(&combined_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "ComponentizeJS compilation failed (exit code: {}).\n\n\
             Stderr:\n{}\n\n\
             Stdout:\n{}\n\n\
             Hint: Check that your handler uses the web-standard fetch event pattern:\n\
             \x20 addEventListener(\"fetch\", (event) => event.respondWith(...))\n\n\
             If using WarpGrid shim imports, ensure the WIT directory includes\n\
             the required interface definitions.",
            output.status.code().unwrap_or(-1),
            stderr,
            stdout
        );
    }

    if !output_path.is_file() {
        bail!(
            "Componentization produced no output at {}",
            output_path.display()
        );
    }

    // Compute size and SHA256
    let wasm_bytes = fs::read(&output_path)?;
    let size_bytes = wasm_bytes.len() as u64;
    let sha256 = hex::encode(Sha256::digest(&wasm_bytes));

    info!(
        "Compiled handler.wasm: {} bytes, sha256: {}",
        size_bytes, sha256
    );

    // Log shim injection summary
    if usage.uses_database {
        info!("Shim injected: warpgrid.database (database-proxy)");
    }
    if usage.uses_dns {
        info!("Shim injected: warpgrid.dns (dns)");
    }
    if usage.uses_filesystem {
        info!("Shim injected: warpgrid.fs (filesystem)");
    }

    Ok(PackResult {
        output_path: output_path.to_string_lossy().to_string(),
        size_bytes,
        sha256,
    })
}

/// Find the WIT directory for the project.
///
/// Checks in order:
/// 1. `<project>/wit/` — project-local WIT definitions
/// 2. `<project>/src/wit/` — source-nested WIT
fn resolve_wit_dir(project_path: &Path) -> Result<PathBuf> {
    let candidates = [
        project_path.join("wit"),
        project_path.join("src").join("wit"),
    ];

    for candidate in &candidates {
        if candidate.is_dir() {
            debug!("Using WIT directory: {}", candidate.display());
            return Ok(candidate.clone());
        }
    }

    bail!(
        "WIT directory not found.\n\
         \n\
         Expected at: {}/wit/\n\
         \n\
         Create a WIT world definition file (e.g., wit/handler.wit) that exports\n\
         wasi:http/incoming-handler@0.2.3 and imports any WarpGrid shim interfaces\n\
         your handler uses.\n\
         \n\
         Example:\n\
         \x20 package my:handler;\n\
         \x20 world handler {{\n\
         \x20   import wasi:http/types@0.2.3;\n\
         \x20   export wasi:http/incoming-handler@0.2.3;\n\
         \x20 }}",
        project_path.display()
    )
}

/// Try to detect the world name from WIT files in the directory.
///
/// Looks for `world <name> {` patterns in .wit files.
fn detect_world_name(wit_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(wit_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "wit")
            && let Ok(content) = fs::read_to_string(&path)
        {
            // Match `world <name> {` pattern
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("world ")
                    && let Some(name) = rest.split_whitespace().next()
                {
                    let name = name.trim_end_matches('{').trim();
                    if !name.is_empty() {
                        debug!("Detected world name '{}' from {}", name, path.display());
                        return Some(name.to_string());
                    }
                }
            }
        }
    }

    warn!("No world name detected in WIT files, using default 'handler'");
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper to create a minimal project structure
    fn create_test_project(
        lang: &str,
        entry: &str,
        handler_content: &str,
    ) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let project = dir.path().to_path_buf();

        // Create warp.toml
        let warp_toml = format!(
            r#"
[package]
name = "test-handler"
version = "0.1.0"

[build]
lang = "{lang}"
entry = "{entry}"
"#
        );
        fs::write(project.join("warp.toml"), warp_toml).unwrap();

        // Create handler source
        let entry_path = project.join(entry);
        if let Some(parent) = entry_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&entry_path, handler_content).unwrap();

        (dir, project)
    }

    fn create_minimal_wit(project: &Path) {
        let wit_dir = project.join("wit");
        fs::create_dir_all(&wit_dir).unwrap();
        fs::write(
            wit_dir.join("handler.wit"),
            r#"package test:handler;

world handler {
  import wasi:http/types@0.2.3;
  export wasi:http/incoming-handler@0.2.3;
}
"#,
        )
        .unwrap();
    }

    #[test]
    fn test_generate_prelude_contains_process_env() {
        let config = WarpConfig::scaffold("test", "js", "src/handler.js");
        let prelude = generate_prelude(&config);

        assert!(prelude.contains("globalThis.process"));
        assert!(prelude.contains("globalThis.process.env"));
        assert!(prelude.contains("WarpGrid Shim Prelude"));
    }

    #[test]
    fn test_generate_prelude_contains_warpgrid_global() {
        let config = WarpConfig::scaffold("test", "js", "src/handler.js");
        let prelude = generate_prelude(&config);

        assert!(prelude.contains("globalThis.warpgrid"));
        assert!(prelude.contains("warpgrid.database"));
        assert!(prelude.contains("warpgrid.dns"));
        assert!(prelude.contains("warpgrid.fs"));
    }

    #[test]
    fn test_generate_prelude_respects_disabled_db_shim() {
        let mut config = WarpConfig::scaffold("test", "js", "src/handler.js");
        config.shims = Some(warp_core::config::ShimsConfig {
            database_proxy: Some(false),
            dns: Some(true),
            timezone: None,
            dev_urandom: None,
            signals: None,
            threading: None,
        });
        let prelude = generate_prelude(&config);

        // Database shim import should NOT be present
        assert!(!prelude.contains("warpgrid:shim/database-proxy"));
        // DNS should still be present
        assert!(prelude.contains("warpgrid:shim/dns"));
    }

    #[test]
    fn test_generate_prelude_respects_disabled_dns_shim() {
        let mut config = WarpConfig::scaffold("test", "js", "src/handler.js");
        config.shims = Some(warp_core::config::ShimsConfig {
            database_proxy: Some(true),
            dns: Some(false),
            timezone: None,
            dev_urandom: None,
            signals: None,
            threading: None,
        });
        let prelude = generate_prelude(&config);

        // DNS shim import should NOT be present
        assert!(!prelude.contains("warpgrid:shim/dns"));
        // Database should still be present
        assert!(prelude.contains("warpgrid:shim/database-proxy"));
    }

    #[test]
    fn test_detect_shim_usage_database() {
        let source = r#"
const db = warpgrid.database.connect({ host: "localhost" });
"#;
        let usage = detect_shim_usage(source);
        assert!(usage.uses_database);
        assert!(!usage.uses_dns);
        assert!(!usage.uses_filesystem);
    }

    #[test]
    fn test_detect_shim_usage_wit_imports() {
        let source = r#"
import { connect } from "warpgrid:shim/database-proxy@0.1.0";
import { resolveAddress } from "warpgrid:shim/dns@0.1.0";
"#;
        let usage = detect_shim_usage(source);
        assert!(usage.uses_database);
        assert!(usage.uses_dns);
    }

    #[test]
    fn test_detect_shim_usage_none() {
        let source = r#"
addEventListener("fetch", (event) => {
  event.respondWith(new Response("hello"));
});
"#;
        let usage = detect_shim_usage(source);
        assert!(!usage.uses_database);
        assert!(!usage.uses_dns);
        assert!(!usage.uses_filesystem);
    }

    #[test]
    fn test_detect_world_name_from_wit() {
        let dir = TempDir::new().unwrap();
        let wit_dir = dir.path().join("wit");
        fs::create_dir_all(&wit_dir).unwrap();
        fs::write(
            wit_dir.join("handler.wit"),
            "package test:handler;\n\nworld my-custom-world {\n  export wasi:http/incoming-handler@0.2.3;\n}\n",
        )
        .unwrap();

        let name = detect_world_name(&wit_dir);
        assert_eq!(name, Some("my-custom-world".to_string()));
    }

    #[test]
    fn test_detect_world_name_no_wit_files() {
        let dir = TempDir::new().unwrap();
        let empty_dir = dir.path().join("empty");
        fs::create_dir_all(&empty_dir).unwrap();

        let name = detect_world_name(&empty_dir);
        assert_eq!(name, None);
    }

    #[test]
    fn test_find_jco_not_installed() {
        let dir = TempDir::new().unwrap();
        let result = find_jco(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ComponentizeJS (jco) not found"));
        assert!(err_msg.contains("scripts/build-componentize-js.sh"));
    }

    #[test]
    fn test_resolve_wit_dir_found() {
        let dir = TempDir::new().unwrap();
        let wit_dir = dir.path().join("wit");
        fs::create_dir_all(&wit_dir).unwrap();
        fs::write(wit_dir.join("handler.wit"), "world handler {}").unwrap();

        let result = resolve_wit_dir(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), wit_dir);
    }

    #[test]
    fn test_resolve_wit_dir_not_found() {
        let dir = TempDir::new().unwrap();
        let result = resolve_wit_dir(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("WIT directory not found"));
    }

    #[test]
    fn test_resolve_wit_dir_src_nested() {
        let dir = TempDir::new().unwrap();
        let wit_dir = dir.path().join("src").join("wit");
        fs::create_dir_all(&wit_dir).unwrap();
        fs::write(wit_dir.join("handler.wit"), "world handler {}").unwrap();

        let result = resolve_wit_dir(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), wit_dir);
    }

    #[test]
    fn test_pack_js_missing_entry() {
        let (_dir, project) = create_test_project("js", "src/handler.js", "");
        // Remove the handler file to trigger missing entry error
        fs::remove_file(project.join("src/handler.js")).unwrap();
        create_minimal_wit(&project);

        let config = WarpConfig::from_file(&project.join("warp.toml")).unwrap();
        let result = pack_js(&project, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Entry point not found"));
    }

    #[test]
    fn test_pack_js_missing_wit_dir() {
        let (_dir, project) = create_test_project(
            "js",
            "src/handler.js",
            r#"addEventListener("fetch", (e) => e.respondWith(new Response("ok")));"#,
        );
        // Don't create WIT dir

        let config = WarpConfig::from_file(&project.join("warp.toml")).unwrap();
        let result = pack_js(&project, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("WIT directory not found"));
    }

    #[test]
    fn test_pack_js_missing_jco() {
        let (_dir, project) = create_test_project(
            "js",
            "src/handler.js",
            r#"addEventListener("fetch", (e) => e.respondWith(new Response("ok")));"#,
        );
        create_minimal_wit(&project);

        let config = WarpConfig::from_file(&project.join("warp.toml")).unwrap();
        let result = pack_js(&project, &config);
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("jco") || err_msg.contains("ComponentizeJS"));
    }

    #[test]
    fn test_pack_routes_js_lang() {
        let (_dir, project) = create_test_project(
            "js",
            "src/handler.js",
            r#"addEventListener("fetch", (e) => e.respondWith(new Response("ok")));"#,
        );

        // This will fail (no jco) but it should reach the JS pipeline, not "Unsupported language"
        let result: Result<crate::PackResult> = crate::pack(&project);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(!err_msg.contains("Unsupported language"));
    }

    #[test]
    fn test_pack_routes_typescript_lang() {
        let (_dir, project) = create_test_project(
            "typescript",
            "src/handler.ts",
            r#"addEventListener("fetch", (e) => e.respondWith(new Response("ok")));"#,
        );

        // This will fail (no jco) but should reach JS pipeline
        let result: Result<crate::PackResult> = crate::pack(&project);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(!err_msg.contains("Unsupported language"));
    }

    #[test]
    fn test_find_sdk_root_from_subdir() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("build").join("componentize-js")).unwrap();
        fs::create_dir_all(root.join("apps").join("my-handler")).unwrap();

        let sdk_root = find_sdk_root(&root.join("apps").join("my-handler"));
        assert_eq!(sdk_root, root.to_path_buf());
    }

    #[test]
    fn test_find_sdk_root_fallback() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("standalone");
        fs::create_dir_all(&project).unwrap();

        let sdk_root = find_sdk_root(&project);
        assert_eq!(sdk_root, project);
    }

    // Integration test: full pipeline with real jco (if available)
    // This test is expensive (invokes Node.js + jco) and requires the toolchain.
    // Run with: cargo test -p warp-pack -- --ignored
    #[test]
    #[ignore]
    fn test_pack_js_end_to_end() {
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();

        let test_fixture = project_root.join("tests").join("fixtures").join("js-http-handler");
        if !test_fixture.exists() {
            eprintln!("Test fixture not found at {}", test_fixture.display());
            return;
        }

        // Create a temp project with warp.toml pointing to the fixture's handler
        let dir = TempDir::new().unwrap();
        let project = dir.path();

        let warp_toml = r#"
[package]
name = "test-js-handler"
version = "0.1.0"

[build]
lang = "js"
entry = "handler.js"
"#;
        fs::write(project.join("warp.toml"), warp_toml).unwrap();

        // Copy handler and wit from fixture
        fs::copy(
            test_fixture.join("handler.js"),
            project.join("handler.js"),
        )
        .unwrap();

        // Copy WIT directory
        let wit_src = test_fixture.join("wit");
        let wit_dst = project.join("wit");
        copy_dir_recursive(&wit_src, &wit_dst).unwrap();

        let config = WarpConfig::from_file(&project.join("warp.toml")).unwrap();
        let result = pack_js(project, &config);

        match result {
            Ok(pack_result) => {
                assert!(Path::new(&pack_result.output_path).exists());
                assert!(pack_result.size_bytes > 0);
                assert!(!pack_result.sha256.is_empty());
                assert_eq!(pack_result.sha256.len(), 64); // hex-encoded SHA256
            }
            Err(e) => {
                // If jco is not installed, the error should be actionable
                let msg: String = e.to_string();
                assert!(
                    msg.contains("jco") || msg.contains("ComponentizeJS"),
                    "Unexpected error: {msg}"
                );
            }
        }
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let dst_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir_recursive(&entry.path(), &dst_path)?;
            } else {
                fs::copy(entry.path(), dst_path)?;
            }
        }
        Ok(())
    }
}
