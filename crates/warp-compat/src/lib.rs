//! warp-compat — the POSIX compatibility shim layer.
//!
//! Host functions implemented in the WarpGrid node agent that bridge
//! POSIX expectations to WASI. Shims are opt-in per deployment.
//!
//! Phase 2 implementation. This is a structural stub.

pub mod shims;

/// Shim configuration loaded from a deployment spec.
#[derive(Debug, Clone, Default)]
pub struct ShimConfig {
    pub timezone: bool,
    pub dev_urandom: bool,
    pub dns: bool,
    pub threading: Option<String>,
    pub signals: bool,
    pub database_proxy: bool,
}
