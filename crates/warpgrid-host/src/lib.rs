//! warpgrid-host — Wasmtime host function shims for WarpGrid.
//!
//! Provides host-side implementations of WarpGrid's custom WIT interfaces:
//! - **filesystem**: Virtual file map (`/etc/resolv.conf`, `/dev/urandom`, timezone data)
//! - **dns**: Service-discovery-aware DNS resolution
//! - **signals**: Lifecycle signal delivery (SIGTERM, SIGHUP, SIGINT)
//! - **db_proxy**: Wire-protocol-level database connection pooling (Postgres, MySQL, Redis)
//! - **threading**: Threading model declaration and compatibility checks
//! - **config**: ShimConfig parsing from deployment specs
//! - **engine**: Top-level WarpGridEngine that wires everything together

pub mod bindings;
pub mod config;
pub mod db_proxy;
pub mod dns;
pub mod engine;
pub mod filesystem;
pub mod signals;
pub mod threading;
