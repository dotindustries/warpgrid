use std::path::Path;

pub fn analyze(path: &str, format: &str) -> anyhow::Result<()> {
    let project_path = Path::new(path);
    let report = warp_analyzer::analyze(project_path)?;

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => {
            println!("{}", warp_analyzer::report::format_report(&report));
        }
    }

    Ok(())
}

pub fn init(path: &str) -> anyhow::Result<()> {
    let project_path = Path::new(path);
    let report = warp_analyzer::analyze(project_path)?;

    if let Some(config) = &report.suggested_config {
        let output = project_path.join("warp.toml");
        std::fs::write(&output, config)?;
        println!("✓ Generated {}", output.display());
    }

    Ok(())
}
