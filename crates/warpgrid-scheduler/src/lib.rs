//! warpgrid-scheduler — deployment scheduling and load balancing.
//!
//! Maps `DeploymentSpec` (from `warpgrid-state`) to `InstancePool`s
//! (from `warp-runtime`). The scheduler:
//!
//! - Creates and tears down instance pools for deployments
//! - Persists instance state records to the state store
//! - Provides round-robin load balancing across instances
//! - Supports manual scaling (scale-up / scale-down)
//!
//! # Architecture
//!
//! ```text
//! Scheduler
//!   ├── StateStore (read DeploymentSpec, write InstanceState)
//!   ├── Runtime (create InstancePool per deployment)
//!   └── Per-deployment slot
//!       ├── InstancePool (warm instances)
//!       └── RoundRobinBalancer (lock-free index selection)
//! ```

pub mod error;
pub mod load_balancer;
pub mod scheduler;

pub use error::{SchedulerError, SchedulerResult};
pub use load_balancer::RoundRobinBalancer;
pub use scheduler::Scheduler;
