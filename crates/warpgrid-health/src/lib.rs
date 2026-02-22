//! warpgrid-health — health checking and self-healing for WarpGrid.
//!
//! Provides HTTP health probes, exponential backoff, and automatic
//! instance state updates. The health monitor runs a background task
//! per deployment that periodically probes the configured endpoint.
//!
//! # Architecture
//!
//! ```text
//! HealthMonitor
//!   ├── Per-deployment background task
//!   │   ├── HealthTracker (consecutive failures, backoff)
//!   │   ├── http_probe() → ProbeResult
//!   │   └── Update InstanceState in StateStore
//!   └── Optional HealthCallback for scheduler notification
//! ```
//!
//! # Self-Healing
//!
//! When an instance crosses the `unhealthy_threshold`, its `InstanceState`
//! is updated to `Unhealthy`. The scheduler can register a callback to
//! replace unhealthy instances automatically.
//!
//! Exponential backoff (1s → 60s) prevents hammering unhealthy instances.
//! A single successful probe resets the backoff and restores `Healthy`.

pub mod checker;
pub mod monitor;

pub use checker::{HealthTracker, ProbeResult};
pub use monitor::HealthMonitor;
