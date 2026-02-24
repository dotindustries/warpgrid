//! Go packaging via TinyGo wasip2.
//!
//! Pipeline:
//! 1. Locate TinyGo binary at `build/tinygo/bin/tinygo` or `$WARPGRID_TINYGO_PATH`
//! 2. Locate entry point from `warp.toml` build.entry
//! 3. Invoke `tinygo build -target=wasip2 -o <output>` to produce a Wasm component
//! 4. Validate output, compute size + SHA256, return PackResult
//!
//! Requires:
//! - TinyGo 0.34+ (installed via scripts/build-tinygo.sh)
//! - Go 1.22+ (for TinyGo's Go frontend)

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

use crate::PackResult;

/// Locate the TinyGo binary.
///
/// Search order:
/// 1. `$WARPGRID_TINYGO_PATH` environment variable
/// 2. `build/tinygo/bin/tinygo` relative to SDK root
/// 3. `tinygo` on `$PATH`
fn find_tinygo(project_path: &Path) -> Result<PathBuf> {
    // 1. Environment variable
    if let Ok(path) = std::env::var("WARPGRID_TINYGO_PATH") {
        let tinygo = PathBuf::from(&path);
        if tinygo.is_file() {
            debug!("Found TinyGo at {} (from WARPGRID_TINYGO_PATH)", tinygo.display());
            return Ok(tinygo);
        }
    }

    // 2. SDK-relative path
    let sdk_root = find_sdk_root(project_path);
    let sdk_tinygo = sdk_root.join("build").join("tinygo").join("bin").join("tinygo");
    if sdk_tinygo.is_file() {
        debug!("Found TinyGo at {} (SDK-relative)", sdk_tinygo.display());
        return Ok(sdk_tinygo);
    }

    // 3. System PATH
    if let Ok(output) = Command::new("which").arg("tinygo").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                debug!("Found TinyGo at {} (system PATH)", path);
                return Ok(PathBuf::from(path));
            }
        }
    }

    bail!(
        "TinyGo not found.\n\
         \n\
         Expected at: {}\n\
         \n\
         To install, run:\n\
         \n\
         \x20 scripts/build-tinygo.sh\n\
         \n\
         Or set WARPGRID_TINYGO_PATH to point to your TinyGo binary.\n\
         \n\
         TinyGo 0.34+ is required for wasip2 target support.",
        sdk_tinygo.display()
    )
}

/// Walk upward from `project_path` to find the SDK root.
fn find_sdk_root(project_path: &Path) -> PathBuf {
    let mut candidate = project_path.to_path_buf();
    for _ in 0..10 {
        if candidate.join("build").join("tinygo").exists()
            || candidate.join("scripts").join("build-tinygo.sh").exists()
        {
            return candidate;
        }
        if !candidate.pop() {
            break;
        }
    }
    project_path.to_path_buf()
}

/// The main Go packaging function invoked by `warp pack --lang go`.
pub fn pack_go(project_path: &Path, config: &warp_core::WarpConfig) -> Result<PackResult> {
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

    // Locate TinyGo
    let tinygo = find_tinygo(project_path)?;

    info!("Packaging Go handler: {}", entry_path.display());

    // Create output directory
    let dist_dir = project_path.join("dist");
    fs::create_dir_all(&dist_dir)?;

    let output_path = dist_dir.join("handler.wasm");

    // Build with TinyGo wasip2 target
    info!("Compiling with TinyGo wasip2 target...");
    let mut cmd = Command::new(&tinygo);
    cmd.arg("build")
        .arg("-target=wasip2")
        .arg("-o")
        .arg(&output_path)
        .arg(&entry_path)
        .current_dir(project_path);

    debug!("Running: {:?}", cmd);

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute TinyGo at {}", tinygo.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "TinyGo compilation failed (exit code: {}).\n\n\
             Stderr:\n{}\n\n\
             Stdout:\n{}\n\n\
             Hint: Ensure your Go code compiles with standard Go first:\n\
             \x20 go build ./...\n\n\
             Then verify TinyGo wasip2 compatibility:\n\
             \x20 tinygo build -target=wasip2 -o /dev/null {}",
            output.status.code().unwrap_or(-1),
            stderr,
            stdout,
            entry_path.display()
        );
    }

    if !output_path.is_file() {
        bail!(
            "TinyGo produced no output at {}",
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

    Ok(PackResult {
        output_path: output_path.to_string_lossy().to_string(),
        size_bytes,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_go_project(entry: &str, handler_content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let project = dir.path().to_path_buf();

        let warp_toml = format!(
            r#"
[package]
name = "test-go-handler"
version = "0.1.0"

[build]
lang = "go"
entry = "{entry}"
"#
        );
        fs::write(project.join("warp.toml"), warp_toml).unwrap();

        let entry_path = project.join(entry);
        if let Some(parent) = entry_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&entry_path, handler_content).unwrap();

        // Create go.mod
        fs::write(
            project.join("go.mod"),
            "module test-handler\n\ngo 1.22\n",
        )
        .unwrap();

        (dir, project)
    }

    #[test]
    fn test_pack_go_missing_entry() {
        let (_dir, project) = create_go_project("main.go", "package main");
        fs::remove_file(project.join("main.go")).unwrap();

        let config = warp_core::WarpConfig::from_file(&project.join("warp.toml")).unwrap();
        let result = pack_go(&project, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Entry point not found"));
    }

    #[test]
    fn test_find_tinygo_not_installed() {
        let dir = TempDir::new().unwrap();
        // Temporarily clear the env var to avoid interference
        let prev = std::env::var("WARPGRID_TINYGO_PATH").ok();
        // SAFETY: test is single-threaded, no concurrent env access
        unsafe { std::env::remove_var("WARPGRID_TINYGO_PATH") };

        let result = find_tinygo(dir.path());
        // Restore env var if it existed
        if let Some(val) = prev {
            // SAFETY: test is single-threaded, no concurrent env access
            unsafe { std::env::set_var("WARPGRID_TINYGO_PATH", val) };
        }

        // Result depends on whether tinygo is installed system-wide
        // If not found, error should mention scripts/build-tinygo.sh
        if let Err(e) = result {
            assert!(e.to_string().contains("TinyGo not found") || e.to_string().contains("tinygo"));
        }
    }

    #[test]
    fn test_find_sdk_root() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(root.join("scripts").join("build-tinygo.sh"), "#!/bin/bash").unwrap();
        fs::create_dir_all(root.join("test-apps").join("my-handler")).unwrap();

        let sdk_root = find_sdk_root(&root.join("test-apps").join("my-handler"));
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

    #[test]
    fn test_pack_routes_go_lang() {
        let (_dir, project) = create_go_project(
            "main.go",
            "package main\n\nfunc main() {}\n",
        );

        let result: Result<crate::PackResult> = crate::pack(&project);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Should reach Go pipeline, not "Unsupported language"
        assert!(!err_msg.contains("Unsupported language"));
    }
}
