//! warpgrid-sprite-storage — object storage, caching, and checkpoint/restore for sprites.
//!
//! This crate manages the persistent storage layer for sprite VMs:
//!
//! - **`object_store`** — S3-compatible client for MinIO/SeaweedFS
//! - **`cache`** — NVMe read-through cache for chunk data
//! - **`checkpoint`** — Checkpoint/restore: snapshot metadata + flush chunks

pub mod cache;
pub mod checkpoint;
pub mod object_store;

pub use cache::{CacheConfig, ChunkCache};
pub use checkpoint::{CheckpointId, CheckpointInfo, CheckpointManager};
pub use object_store::{ObjectStore, ObjectStoreConfig, S3ObjectStore};
