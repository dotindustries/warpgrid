//! warpgrid-metrics — observability for WarpGrid deployments.
//!
//! Tracks per-deployment request metrics (RPS, latency, error rate),
//! persists periodic snapshots to the state store, and provides
//! Prometheus-compatible text exposition.
//!
//! # Architecture
//!
//! ```text
//! MetricsCollector
//!   ├── record_request() ← called per HTTP request
//!   ├── snapshot() → persists MetricsSnapshot to StateStore
//!   └── run() → periodic snapshot loop
//!
//! Prometheus exposition
//!   └── render_prometheus() → text/plain for /metrics endpoint
//! ```

pub mod collector;
pub mod prometheus;

pub use collector::MetricsCollector;
pub use prometheus::render_prometheus;
