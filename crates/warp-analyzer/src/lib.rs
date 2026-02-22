pub mod analyzers;
pub mod db;
pub mod report;

use anyhow::Result;
use std::path::Path;
use warp_core::{AnalysisReport, OverallVerdict};

/// Run a full analysis on a project directory or Dockerfile.
pub fn analyze(path: &Path) -> Result<AnalysisReport> {
    let language = analyzers::detect_language(path)?;

    tracing::info!(language = %language, "Detected project language");

    let deps = match language.as_str() {
        "rust" => analyzers::rust::analyze_cargo_toml(path)?,
        "go" => analyzers::go::analyze_go_mod(path)?,
        "typescript" => analyzers::typescript::analyze_package_json(path)?,
        _ => {
            tracing::warn!("Unsupported language: {language}");
            vec![]
        }
    };

    let (blockers, shim_items) = db::evaluate_dependencies(&deps, &language);

    let blocking_count = blockers.len();
    let shim_count = shim_items.len();
    let total = deps.len();
    let compatible = total - blocking_count - shim_count;

    let overall_verdict = if blocking_count == 0 && shim_count == 0 {
        OverallVerdict::Convertible
    } else if blocking_count == 0 {
        OverallVerdict::ConvertibleWithShims
    } else if compatible as f64 / total as f64 > 0.5 {
        OverallVerdict::PartiallyConvertible
    } else {
        OverallVerdict::NotConvertible
    };

    let config = warp_core::WarpConfig::scaffold(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-app"),
        &language,
        match language.as_str() {
            "rust" => "src/main.rs",
            "go" => "main.go",
            "typescript" => "src/index.ts",
            _ => "src/main",
        },
    );

    Ok(AnalysisReport {
        project_name: path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string(),
        language,
        overall_verdict,
        dependencies: deps,
        blockers,
        shim_items,
        estimated_wasm_size_mb: None,
        suggested_config: config.to_toml_string().ok(),
    })
}
