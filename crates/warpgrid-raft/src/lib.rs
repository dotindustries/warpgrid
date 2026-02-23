// openraft's StorageError is 224 bytes by design — allow it.
#![allow(clippy::result_large_err)]

//! WarpGrid Raft consensus — openraft + redb state machine.
//!
//! This crate provides the Raft consensus layer for the WarpGrid
//! orchestrator. It uses openraft for the protocol implementation
//! and redb for durable log and state machine storage.
//!
//! # Architecture
//!
//! - **`typ`** — Type configuration (`TypeConfig`, `Request`, `Response`)
//! - **`log_store`** — Raft log storage backed by redb
//! - **`state_machine`** — State machine that applies committed entries
//! - **`network`** — gRPC network transport for inter-node Raft RPCs
//! - **`server`** — gRPC server that handles incoming Raft RPCs
//! - **`node_map`** — Bidirectional String ↔ u64 node ID mapping

pub mod log_store;
pub mod network;
pub mod node_map;
pub mod server;
pub mod state_machine;
pub mod typ;

/// Generated protobuf types and gRPC service stubs.
pub mod proto {
    tonic::include_proto!("warpgrid.raft");
}

pub use log_store::LogStore;
pub use network::{NetworkConnection, NetworkFactory};
pub use node_map::NodeIdMap;
pub use server::RaftGrpcServer;
pub use state_machine::StateMachine;
pub use typ::{Request, Response, TypeConfig, WarpGridRaft};
