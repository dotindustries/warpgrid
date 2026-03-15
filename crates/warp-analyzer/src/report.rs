//! Human-readable report formatting.

use warp_core::AnalysisReport;

pub fn format_report(report: &AnalysisReport) -> String {
    let mut out = String::new();

    out.push_str(&format!("\n╔══════════════════════════════════════════╗\n"));
    out.push_str(&format!("║  WarpGrid Compatibility Analysis         ║\n"));
    out.push_str(&format!("╠══════════════════════════════════════════╣\n"));
    out.push_str(&format!("║  Project:  {:<29}║\n", report.project_name));
    out.push_str(&format!("║  Language: {:<29}║\n", report.language));
    out.push_str(&format!("║  Verdict:  {:<29}║\n", report.overall_verdict.label()));
    out.push_str(&format!("╚══════════════════════════════════════════╝\n\n"));

    let total = report.dependencies.len();
    let blocking = report.blockers.len();
    let shim = report.shim_items.len();
    let compat = total.saturating_sub(blocking + shim);

    out.push_str(&format!("Dependencies ({total} total):\n"));
    out.push_str(&format!("  ✅ {compat} fully compatible\n"));
    out.push_str(&format!("  ⚠️  {shim} compatible via shim layer\n"));
    out.push_str(&format!("  ❌ {blocking} require changes\n\n"));

    // Bun-specific: show a compatibility table for each dependency
    if report.language == "bun" && !report.dependencies.is_empty() {
        format_bun_compat_table(&mut out, report);
    }

    if !report.blockers.is_empty() {
        out.push_str("❌ BLOCKERS:\n\n");
        for (i, b) in report.blockers.iter().enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, b.dependency));
            out.push_str(&format!("     Reason: {}\n", b.reason));
            out.push_str(&format!("     Fix:    {}\n", b.fix));
            if let Some(h) = b.effort_hours {
                out.push_str(&format!("     Effort: ~{h} hours\n"));
            }
            out.push('\n');
        }
    }

    if !report.shim_items.is_empty() {
        out.push_str("⚠️  SHIM-COMPATIBLE (no code changes needed):\n\n");
        for s in &report.shim_items {
            out.push_str(&format!("  • {} → {} shim\n", s.name, s.shim));
        }
        out.push('\n');
    }

    if let Some(config) = &report.suggested_config {
        out.push_str("SUGGESTED warp.toml:\n\n");
        for line in config.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }

    out
}

/// Render a Bun compatibility table: package | version | status | notes
fn format_bun_compat_table(out: &mut String, report: &AnalysisReport) {
    use std::collections::HashMap;

    // Load the Bun compat DB to get status details
    let bun_db: HashMap<String, BunTableEntry> = load_bun_table_entries();

    // Build blocker set for quick lookup
    let blocker_set: std::collections::HashSet<&str> = report
        .blockers
        .iter()
        .map(|b| b.dependency.as_str())
        .collect();

    out.push_str("Bun Compatibility Table:\n\n");
    out.push_str("  ┌──────────────────────────┬──────────┬──────────────┬─────────────────────────┐\n");
    out.push_str("  │ Package                  │ Version  │ Status       │ Notes                   │\n");
    out.push_str("  ├──────────────────────────┼──────────┼──────────────┼─────────────────────────┤\n");

    for dep in &report.dependencies {
        let version = dep.version.as_deref().unwrap_or("-");
        let (status, notes) = if let Some(entry) = bun_db.get(&dep.name) {
            if entry.status == "pass" {
                ("✅ pass", format!("bundle:{} componentize:{}", ok_str(entry.bundle_ok), ok_str(entry.componentize_ok)))
            } else {
                ("❌ fail", format!("bundle:{} componentize:{}", ok_str(entry.bundle_ok), ok_str(entry.componentize_ok)))
            }
        } else if blocker_set.contains(dep.name.as_str()) {
            ("❌ fail", "blocked".to_string())
        } else {
            ("⚪ unknown", "not in compat-db".to_string())
        };

        out.push_str(&format!(
            "  │ {:<24} │ {:<8} │ {:<12} │ {:<23} │\n",
            truncate(&dep.name, 24),
            truncate(version, 8),
            status,
            truncate(&notes, 23),
        ));
    }

    out.push_str("  └──────────────────────────┴──────────┴──────────────┴─────────────────────────┘\n\n");
}

fn ok_str(ok: bool) -> &'static str {
    if ok { "ok" } else { "FAIL" }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max - 1])
    } else {
        s.to_string()
    }
}

/// Minimal struct for table display purposes.
#[derive(serde::Deserialize)]
struct BunTableEntry {
    #[allow(dead_code)]
    name: String,
    status: String,
    bundle_ok: bool,
    componentize_ok: bool,
}

#[derive(serde::Deserialize)]
struct BunTableFile {
    results: Vec<BunTableEntry>,
}

fn load_bun_table_entries() -> std::collections::HashMap<String, BunTableEntry> {
    const BUN_JSON: &str = include_str!("../../../compat-db/bun/results.json");
    let file: BunTableFile = serde_json::from_str(BUN_JSON).expect("invalid results.json");
    file.results.into_iter().map(|e| (e.name.clone(), e)).collect()
}
