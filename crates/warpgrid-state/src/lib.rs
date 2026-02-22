//! warpgrid-state — embedded state store for WarpGrid.
//!
//! Backed by [redb](https://docs.rs/redb), provides persistent and in-memory
//! state management for deployments, instances, nodes, services, and metrics.
//!
//! # Architecture
//!
//! All domain types are JSON-serialized into redb's `&[u8]` value columns.
//! Composite keys (`{namespace}/{name}`, `{deployment_id}:{index}`) enable
//! efficient prefix scans for related records.
//!
//! The `StateStore` is `Clone` + `Send` + `Sync` (backed by `Arc<Database>`)
//! and can be shared across async tasks.

pub mod error;
pub mod store;
pub mod tables;
pub mod types;

pub use error::{StateError, StateResult};
pub use store::StateStore;
pub use types::*;
