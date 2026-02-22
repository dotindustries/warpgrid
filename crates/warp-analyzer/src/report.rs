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
