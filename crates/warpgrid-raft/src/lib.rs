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

pub mod log_store;
pub mod network;
pub mod state_machine;
pub mod typ;

pub use log_store::LogStore;
pub use network::{NetworkConnection, NetworkFactory};
pub use state_machine::StateMachine;
pub use typ::{Request, Response, TypeConfig, WarpGridRaft};
