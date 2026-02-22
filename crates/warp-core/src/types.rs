//! Shared types used across WarpGrid crates.

use serde::{Deserialize, Serialize};

/// Compatibility verdict for a dependency or workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    /// Fully compatible with WASI P2.
    Compatible,
    /// Works via the shim layer with zero code changes.
    ShimCompatible { shim: String },
    /// Incompatible, but a known alternative exists.
    Incompatible { reason: String, alternative: Option<String> },
    /// Fundamentally incompatible, no workaround.
    Blocked { reason: String },
    /// Unknown — not yet in the compatibility database.
    Unknown,
}

impl Verdict {
    pub fn is_blocking(&self) -> bool {
        matches!(self, Verdict::Incompatible { .. } | Verdict::Blocked { .. })
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Verdict::Compatible => "✅",
            Verdict::ShimCompatible { .. } => "⚠️",
            Verdict::Incompatible { .. } => "❌",
            Verdict::Blocked { .. } => "🚫",
            Verdict::Unknown => "❓",
        }
    }
}

/// Analysis report for a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub project_name: String,
    pub language: String,
    pub overall_verdict: OverallVerdict,
    pub dependencies: Vec<DependencyVerdict>,
    pub blockers: Vec<Blocker>,
    pub shim_items: Vec<ShimItem>,
    pub estimated_wasm_size_mb: Option<f64>,
    pub suggested_config: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverallVerdict {
    Convertible,
    ConvertibleWithShims,
    PartiallyConvertible,
    NotConvertible,
}

impl OverallVerdict {
    pub fn label(&self) -> &'static str {
        match self {
            OverallVerdict::Convertible => "CONVERTIBLE",
            OverallVerdict::ConvertibleWithShims => "CONVERTIBLE WITH SHIMS",
            OverallVerdict::PartiallyConvertible => "PARTIALLY CONVERTIBLE",
            OverallVerdict::NotConvertible => "NOT CONVERTIBLE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyVerdict {
    pub name: String,
    pub version: Option<String>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocker {
    pub dependency: String,
    pub reason: String,
    pub fix: String,
    pub effort_hours: Option<f64>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShimItem {
    pub name: String,
    pub shim: String,
    pub description: String,
}
