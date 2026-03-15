//! Compatibility database — resolves dependency verdicts.

use serde::Deserialize;
use std::collections::HashMap;
use warp_core::{Blocker, DependencyVerdict, ShimItem};

/// Bun compat-db results.json embedded at compile time.
const BUN_RESULTS_JSON: &str = include_str!("../../../../compat-db/bun/results.json");

#[derive(Debug, Deserialize)]
struct CompatEntry {
    name: String,
    verdict: String,
    reason: Option<String>,
    alternative: Option<String>,
    shim: Option<String>,
    migration_guide: Option<String>,
}

/// Built-in compatibility rules (shipped with the binary).
/// In production, this loads from compat-db/ TOML files.
fn builtin_rules() -> HashMap<String, CompatEntry> {
    let mut rules = HashMap::new();

    // Rust ecosystem
    let rust_rules = vec![
        ("tokio", "compatible", None, None, None),
        ("serde", "compatible", None, None, None),
        ("axum", "compatible", None, None, None),
        ("hyper", "compatible", None, None, None),
        ("reqwest", "compatible", None, None, None),
        ("tracing", "compatible", None, None, None),
        ("sqlx", "compatible", None, None, None),
        ("rustls", "compatible", None, None, None),
        ("openssl-sys", "incompatible", Some("FFI to native OpenSSL"), Some("rustls"), None),
        ("openssl", "incompatible", Some("FFI to native OpenSSL"), Some("rustls"), None),
        ("libz-sys", "incompatible", Some("FFI to native zlib"), Some("flate2 with rust backend"), None),
        ("ring", "incompatible", Some("Contains platform-specific assembly"), Some("aws-lc-rs or rustls"), None),
        ("nix", "incompatible", Some("Direct Unix syscall wrappers"), None, None),
        ("libc", "shim_compatible", None, None, Some("filesystem")),
    ];
    for (name, verdict, reason, alt, shim) in rust_rules {
        rules.insert(name.to_string(), CompatEntry {
            name: name.to_string(),
            verdict: verdict.to_string(),
            reason: reason.map(String::from),
            alternative: alt.map(String::from),
            shim: shim.map(String::from),
            migration_guide: None,
        });
    }

    // Go ecosystem
    let go_rules = vec![
        ("github.com/gin-gonic/gin", "incompatible", Some("Uses net/http extensively"), Some("TinyGo-compatible HTTP framework"), None),
        ("github.com/lib/pq", "shim_compatible", Some("Raw TCP via net.Dial"), None, Some("database_proxy")),
        ("github.com/jackc/pgx", "shim_compatible", Some("TCP sockets"), None, Some("database_proxy")),
        ("github.com/go-sql-driver/mysql", "shim_compatible", Some("TCP sockets via net.Dial"), None, Some("database_proxy")),
        ("github.com/redis/go-redis/v9", "shim_compatible", Some("TCP sockets via net.Dial"), None, Some("database_proxy")),
    ];
    for (name, verdict, reason, alt, shim) in go_rules {
        rules.insert(name.to_string(), CompatEntry {
            name: name.to_string(),
            verdict: verdict.to_string(),
            reason: reason.map(String::from),
            alternative: alt.map(String::from),
            shim: shim.map(String::from),
            migration_guide: None,
        });
    }

    // TypeScript ecosystem
    let ts_rules = vec![
        ("express", "compatible", None, None, None),
        ("hono", "compatible", None, None, None),
        ("sharp", "incompatible", Some("Native C++ binding (libvips)"), Some("wasm-vips"), None),
        ("bcrypt", "incompatible", Some("Native C binding"), Some("bcryptjs"), None),
        ("better-sqlite3", "incompatible", Some("Native C binding"), Some("sql.js"), None),
        ("pg", "shim_compatible", Some("TCP sockets"), None, Some("database_proxy")),
    ];
    for (name, verdict, reason, alt, shim) in ts_rules {
        rules.insert(name.to_string(), CompatEntry {
            name: name.to_string(),
            verdict: verdict.to_string(),
            reason: reason.map(String::from),
            alternative: alt.map(String::from),
            shim: shim.map(String::from),
            migration_guide: None,
        });
    }

    rules
}

/// A single entry from `compat-db/bun/results.json`.
#[derive(Debug, Deserialize)]
struct BunCompatResult {
    name: String,
    #[allow(dead_code)]
    version: String,
    status: String,
    bundle_ok: bool,
    componentize_ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_stage: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// Top-level structure of `results.json`.
#[derive(Debug, Deserialize)]
struct BunResultsFile {
    results: Vec<BunCompatResult>,
}

/// Load Bun compat data from the embedded `results.json`.
fn bun_compat_rules() -> HashMap<String, BunCompatResult> {
    let file: BunResultsFile =
        serde_json::from_str(BUN_RESULTS_JSON).expect("invalid compat-db/bun/results.json");
    file.results
        .into_iter()
        .map(|r| (r.name.clone(), r))
        .collect()
}

/// Evaluate a list of dependencies against the compat DB.
///
/// When `language` is `"bun"`, uses the Bun-specific compat DB (`results.json`).
/// For all other languages, uses the built-in rule set.
pub fn evaluate_dependencies(
    deps: &[DependencyVerdict],
    language: &str,
) -> (Vec<Blocker>, Vec<ShimItem>) {
    if language == "bun" {
        return evaluate_bun_dependencies(deps);
    }

    let rules = builtin_rules();
    let mut blockers = Vec::new();
    let mut shim_items = Vec::new();

    for dep in deps {
        if let Some(entry) = rules.get(&dep.name) {
            match entry.verdict.as_str() {
                "incompatible" => {
                    blockers.push(Blocker {
                        dependency: dep.name.clone(),
                        reason: entry.reason.clone().unwrap_or_default(),
                        fix: entry.alternative.as_ref()
                            .map(|a| format!("Replace with: {a}"))
                            .unwrap_or_else(|| "No known alternative".to_string()),
                        effort_hours: Some(2.0),
                        location: None,
                    });
                }
                "shim_compatible" => {
                    shim_items.push(ShimItem {
                        name: dep.name.clone(),
                        shim: entry.shim.clone().unwrap_or_default(),
                        description: entry.reason.clone().unwrap_or_default(),
                    });
                }
                _ => {} // compatible or unknown
            }
        }
    }

    (blockers, shim_items)
}

/// Evaluate Bun dependencies against `compat-db/bun/results.json`.
///
/// Maps `status: "pass"` → compatible (no action), any other status → blocker.
fn evaluate_bun_dependencies(deps: &[DependencyVerdict]) -> (Vec<Blocker>, Vec<ShimItem>) {
    let bun_rules = bun_compat_rules();
    let mut blockers = Vec::new();

    for dep in deps {
        if let Some(entry) = bun_rules.get(&dep.name) {
            if entry.status != "pass" {
                // Build a descriptive reason from the result fields
                let mut notes = Vec::new();
                if !entry.bundle_ok {
                    notes.push("bundle failed".to_string());
                }
                if !entry.componentize_ok {
                    notes.push("componentize failed".to_string());
                }
                if let Some(stage) = &entry.error_stage {
                    notes.push(format!("failed at {stage} stage"));
                }

                let reason = if let Some(err) = &entry.error {
                    // Truncate long error messages
                    let truncated = if err.len() > 120 {
                        format!("{}…", &err[..120])
                    } else {
                        err.clone()
                    };
                    truncated
                } else {
                    notes.join("; ")
                };

                blockers.push(Blocker {
                    dependency: dep.name.clone(),
                    reason,
                    fix: "Check compat-db/bun/results.json for details".to_string(),
                    effort_hours: None,
                    location: None,
                });
            }
        }
        // Dependencies not in the DB are treated as unknown (not blocked)
    }

    // Bun compat DB doesn't have shim entries — all are pass/fail
    (blockers, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use warp_core::Verdict;

    fn make_dep(name: &str) -> DependencyVerdict {
        DependencyVerdict {
            name: name.to_string(),
            version: Some("1.0.0".to_string()),
            verdict: Verdict::Unknown,
        }
    }

    #[test]
    fn test_bun_compat_rules_loads_results_json() {
        let rules = bun_compat_rules();
        assert!(!rules.is_empty(), "Bun compat rules should not be empty");
        assert!(rules.contains_key("hono"), "Should contain hono");
        assert!(rules.contains_key("marked"), "Should contain marked (failing)");
    }

    #[test]
    fn test_bun_passing_dep_is_not_blocked() {
        let deps = vec![make_dep("hono")];
        let (blockers, shims) = evaluate_dependencies(&deps, "bun");
        assert!(blockers.is_empty(), "hono should not be a blocker");
        assert!(shims.is_empty());
    }

    #[test]
    fn test_bun_failing_dep_is_blocked() {
        let deps = vec![make_dep("marked")];
        let (blockers, _) = evaluate_dependencies(&deps, "bun");
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].dependency, "marked");
        assert!(!blockers[0].reason.is_empty());
    }

    #[test]
    fn test_bun_unknown_dep_is_not_blocked() {
        let deps = vec![make_dep("some-unknown-package")];
        let (blockers, shims) = evaluate_dependencies(&deps, "bun");
        assert!(blockers.is_empty(), "Unknown deps should not be blocked");
        assert!(shims.is_empty());
    }

    #[test]
    fn test_bun_mixed_deps() {
        let deps = vec![
            make_dep("hono"),      // pass
            make_dep("zod"),       // pass
            make_dep("marked"),    // fail
            make_dep("unknown"),   // not in DB
        ];
        let (blockers, _) = evaluate_dependencies(&deps, "bun");
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].dependency, "marked");
    }

    #[test]
    fn test_non_bun_language_uses_builtin_rules() {
        let deps = vec![make_dep("openssl-sys")];
        let (blockers, _) = evaluate_dependencies(&deps, "rust");
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].dependency, "openssl-sys");
    }

    #[test]
    fn test_bun_results_json_status_mapping() {
        let rules = bun_compat_rules();

        // Verify pass entries
        let hono = rules.get("hono").unwrap();
        assert_eq!(hono.status, "pass");
        assert!(hono.bundle_ok);
        assert!(hono.componentize_ok);

        // Verify fail entry
        let marked = rules.get("marked").unwrap();
        assert!(marked.status.starts_with("fail"));
        assert!(!marked.componentize_ok);
        assert!(marked.error.is_some());
    }
}
