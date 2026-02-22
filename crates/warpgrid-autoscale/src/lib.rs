//! warpgrid-autoscale — metrics-driven instance scaling.
//!
//! Reads `MetricsSnapshot` from the state store, compares against
//! `ScalingConfig.target_value`, and emits scaling decisions. Supports
//! scale-to-zero on zero traffic and cooldown windows to prevent thrashing.
//!
//! # Scaling Algorithm
//!
//! ```text
//! current_value = latest metric (rps, latency_p99, error_rate, memory)
//! target        = scaling.target_value
//!
//! if current > target * 1.1:
//!     desired = ceil(current_instances * (current / target))
//!     ScaleTo(min(desired, max_instances))
//!
//! if current < target * 0.5 and instances > min:
//!     desired = ceil(current_instances * (current / target))
//!     ScaleTo(max(desired, min_instances))
//!
//! if rps == 0 and min_instances == 0:
//!     ScaleTo(0)  // scale-to-zero
//! ```
//!
//! Cooldown windows (`scale_up_window`, `scale_down_window`) prevent
//! rapid oscillation.

pub mod scaler;

pub use scaler::{Autoscaler, ScaleDecision};
