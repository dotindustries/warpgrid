//! `warp dev` — Local development server with file watching and hot-reload.
//!
//! Dispatches to language-specific dev servers:
//! - `--lang bun`: Spawns Bun dev server from `@warpgrid/bun-sdk`
//!
//! The Bun dev server supports two modes:
//! - Default (Wasm): Compiles handler, serves via jco serve, watches and recompiles
//! - `--native`: Runs handler directly in Bun with hot-reload

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::info;
use warp_core::WarpConfig;

/// Run the `warp dev` command.
///
/// Reads `warp.toml` to detect language, then spawns the appropriate dev server.
pub fn dev(path: &str, port: u16, native: bool) -> Result<()> {
    let project_path = Path::new(path).canonicalize().unwrap_or_else(|_| Path::new(path).to_path_buf());

    // Try to read warp.toml for language detection
    let lang = detect_language(&project_path)?;

    info!("Starting dev server for {} project at {}", lang, project_path.display());

    match lang.as_str() {
        "bun" => dev_bun(&project_path, port, native),
        "rust" => bail!("Rust dev server not yet implemented."),
        "go" => bail!("Go dev server not yet implemented."),
        "typescript" => bail!("TypeScript dev server not yet implemented. Use --lang bun for Bun."),
        _ => bail!("Unsupported language for dev server: {lang}"),
    }
}

/// Detect language from warp.toml [build].lang field.
fn detect_language(project_path: &Path) -> Result<String> {
    let warp_toml = project_path.join("warp.toml");

    if warp_toml.is_file() {
        let config = WarpConfig::from_file(&warp_toml)?;
        if let Some(build) = &config.build {
            return Ok(build.lang.clone());
        }
    }

    // Auto-detect from project files
    if project_path.join("bunfig.toml").is_file() {
        return Ok("bun".to_string());
    }
    if project_path.join("package.json").is_file() {
        return Ok("bun".to_string());
    }
    if project_path.join("go.mod").is_file() {
        return Ok("go".to_string());
    }
    if project_path.join("Cargo.toml").is_file() {
        return Ok("rust".to_string());
    }

    bail!(
        "Cannot detect project language. Create a warp.toml with [build].lang or add bunfig.toml/package.json/go.mod."
    )
}

/// Launch the Bun dev server.
///
/// Resolves the `dev-cli.ts` entry point from the `@warpgrid/bun-sdk` package
/// and spawns it as a child process.
fn dev_bun(project_path: &Path, port: u16, native: bool) -> Result<()> {
    let dev_cli = resolve_dev_cli(project_path)?;

    info!("Using dev CLI: {}", dev_cli.display());

    let mut cmd = Command::new("bun");
    cmd.arg("run")
        .arg(&dev_cli)
        .arg("--port")
        .arg(port.to_string())
        .arg(project_path);

    if native {
        cmd.arg("--native");
    }

    // Inherit stdio so the user sees the dev server output
    let status = cmd
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context(
            "Failed to execute 'bun'. Is Bun installed? Install from https://bun.sh"
        )?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        bail!("Dev server exited with code {code}");
    }

    Ok(())
}

/// Resolve the path to `dev-cli.ts` from `@warpgrid/bun-sdk`.
///
/// Search order:
/// 1. `$WARPGRID_BUN_SDK_PATH/src/dev-cli.ts`
/// 2. `<project>/node_modules/@warpgrid/bun-sdk/src/dev-cli.ts`
/// 3. Walk up from project to find workspace `packages/warpgrid-bun-sdk/src/dev-cli.ts`
fn resolve_dev_cli(project_path: &Path) -> Result<std::path::PathBuf> {
    // 1. Environment variable
    if let Ok(sdk_path) = std::env::var("WARPGRID_BUN_SDK_PATH") {
        let p = Path::new(&sdk_path).join("src/dev-cli.ts");
        if p.is_file() {
            return Ok(p);
        }
    }

    // 2. Project-local node_modules
    let node_modules = project_path.join("node_modules/@warpgrid/bun-sdk/src/dev-cli.ts");
    if node_modules.is_file() {
        return Ok(node_modules);
    }

    // 3. Walk up to find workspace packages directory
    let mut dir = project_path.to_path_buf();
    for _ in 0..10 {
        let candidate = dir.join("packages/warpgrid-bun-sdk/src/dev-cli.ts");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }

    bail!(
        "Could not find @warpgrid/bun-sdk dev CLI. Install it with:\n  \
         bun add @warpgrid/bun-sdk\n  \
         Or set WARPGRID_BUN_SDK_PATH to the SDK directory."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_detect_language_from_warp_toml() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("warp.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nlang = \"bun\"\nentry = \"src/index.ts\"\n",
        ).unwrap();

        let lang = detect_language(dir.path()).unwrap();
        assert_eq!(lang, "bun");
    }

    #[test]
    fn test_detect_language_from_bunfig() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bunfig.toml"), "[install]\n").unwrap();

        let lang = detect_language(dir.path()).unwrap();
        assert_eq!(lang, "bun");
    }

    #[test]
    fn test_detect_language_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let lang = detect_language(dir.path()).unwrap();
        assert_eq!(lang, "bun");
    }

    #[test]
    fn test_detect_language_from_go_mod() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module test\n").unwrap();

        let lang = detect_language(dir.path()).unwrap();
        assert_eq!(lang, "go");
    }

    #[test]
    fn test_detect_language_missing() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect_language(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_dev_cli_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_dev_cli(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("@warpgrid/bun-sdk"));
    }
}
