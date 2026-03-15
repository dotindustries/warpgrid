//! Integration tests for Bun analysis pipeline.

use std::path::Path;
use warp_core::OverallVerdict;

/// Test full analysis of the t5-bun-http-postgres fixture using --lang bun override.
#[test]
fn test_analyze_t5_bun_fixture_with_lang_override() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-apps/t5-bun-http-postgres");

    let report = warp_analyzer::analyze(&fixture, Some("bun")).unwrap();

    assert_eq!(report.language, "bun");
    assert!(!report.dependencies.is_empty(), "Should find dependencies");

    // hono should be detected as a dependency
    let hono = report.dependencies.iter().find(|d| d.name == "hono");
    assert!(hono.is_some(), "Should find hono dependency");

    // hono is "pass" in results.json — should not be a blocker
    assert!(
        !report.blockers.iter().any(|b| b.dependency == "hono"),
        "hono should not be a blocker"
    );
}

/// Test that auto-detection works when bunfig.toml is present.
#[test]
fn test_analyze_bun_project_with_bunfig() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("bunfig.toml"), "[install]\n").unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies": {"hono": "4.7.4", "zod": "3.24.2"}}"#,
    )
    .unwrap();

    let report = warp_analyzer::analyze(tmp.path(), None).unwrap();

    assert_eq!(report.language, "bun");
    assert_eq!(report.dependencies.len(), 2);
    assert!(report.blockers.is_empty(), "hono and zod should both pass");
    assert!(
        matches!(report.overall_verdict, OverallVerdict::Convertible),
        "All-passing Bun project should be Convertible, got {:?}",
        report.overall_verdict
    );
}

/// Test that a failing dependency produces a blocker.
#[test]
fn test_bun_failing_dep_produces_blocker() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("bunfig.toml"), "").unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies": {"marked": "15.0.6"}}"#,
    )
    .unwrap();

    let report = warp_analyzer::analyze(tmp.path(), None).unwrap();

    assert_eq!(report.language, "bun");
    assert_eq!(report.blockers.len(), 1);
    assert_eq!(report.blockers[0].dependency, "marked");
    // 1 dep, 1 blocker, 0 compatible → NotConvertible
    assert!(
        matches!(report.overall_verdict, OverallVerdict::NotConvertible),
        "Fully-blocked Bun project should be NotConvertible, got {:?}",
        report.overall_verdict
    );
}

/// Test that exit code 1 logic works — blockers present means failure.
#[test]
fn test_bun_report_has_blockers_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("bunfig.toml"), "").unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies": {"marked": "15.0.6"}}"#,
    )
    .unwrap();

    let report = warp_analyzer::analyze(tmp.path(), None).unwrap();
    let has_blockers = !report.blockers.is_empty();
    assert!(has_blockers, "Should have blockers (exit code 1 scenario)");
}

/// Test the report table format for Bun projects.
#[test]
fn test_bun_report_contains_compat_table() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("bunfig.toml"), "").unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies": {"hono": "4.7.4"}}"#,
    )
    .unwrap();

    let report = warp_analyzer::analyze(tmp.path(), None).unwrap();
    let formatted = warp_analyzer::report::format_report(&report);

    assert!(formatted.contains("Bun Compatibility Table"), "Should contain Bun table header");
    assert!(formatted.contains("hono"), "Should contain hono in table");
    assert!(formatted.contains("pass"), "Should show pass status for hono");
}

/// Test JSON output is valid and parseable.
#[test]
fn test_bun_json_output() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("bunfig.toml"), "").unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies": {"hono": "4.7.4"}}"#,
    )
    .unwrap();

    let report = warp_analyzer::analyze(tmp.path(), None).unwrap();
    let json = serde_json::to_string_pretty(&report).unwrap();

    // Verify it's valid JSON by parsing it back
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["language"], "bun");
    assert!(parsed["dependencies"].as_array().unwrap().len() > 0);
}

/// Test --lang bun override on a project without bunfig.toml.
#[test]
fn test_lang_override_without_bunfig() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No bunfig.toml — only package.json
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"dependencies": {"hono": "4.7.4"}}"#,
    )
    .unwrap();

    // Without override, detected as typescript
    let report_ts = warp_analyzer::analyze(tmp.path(), None).unwrap();
    assert_eq!(report_ts.language, "typescript");

    // With override, analyzed as bun
    let report_bun = warp_analyzer::analyze(tmp.path(), Some("bun")).unwrap();
    assert_eq!(report_bun.language, "bun");
}
