//! Compatibility database — resolves dependency verdicts.

use serde::Deserialize;
use std::collections::HashMap;
use warp_core::{Blocker, DependencyVerdict, ShimItem, Verdict};

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
        ("github.com/go-redis/redis", "shim_compatible", Some("TCP sockets"), None, Some("database_proxy")),
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

/// Evaluate a list of dependencies against the compat DB.
pub fn evaluate_dependencies(
    deps: &[DependencyVerdict],
    _language: &str,
) -> (Vec<Blocker>, Vec<ShimItem>) {
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
