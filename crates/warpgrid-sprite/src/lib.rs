//! warpgrid-sprite — lightweight Linux VM lifecycle management.
//!
//! This crate provides the execution engine for WarpGrid's "sprite" primitive:
//! lightweight KVM micro-VMs for running Claude Code sessions on bare metal.
//!
//! # Components
//!
//! - **`hypervisor`** — Trait abstraction over Cloud Hypervisor / Firecracker
//! - **`pool`** — Warm VM pool for instant sprite creation
//! - **`vsock`** — Host↔guest communication protocol over VM sockets
//! - **`manager`** — Top-level sprite lifecycle orchestration
//! - **`error`** — Error types

pub mod error;
pub mod hypervisor;
pub mod manager;
pub mod pool;
pub mod vsock;

pub use error::{SpriteError, SpriteResult};
pub use hypervisor::{Hypervisor, VmConfig, VmHandle, VmStatus, VirtioFsMount};
pub use manager::{SpriteManager, SpriteManagerConfig};
pub use pool::{PoolConfig, SpritePool};
pub use vsock::{SpriteMessage, VsockStream};
