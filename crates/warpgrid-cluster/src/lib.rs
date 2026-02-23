//! warpgrid-cluster — multi-node clustering for WarpGrid.
//!
//! Provides the gRPC service and client for node-to-node communication,
//! heartbeat-based health tracking, mTLS for security, and cluster
//! membership management.
//!
//! # Architecture
//!
//! ```text
//! Control Plane (leader)
//!   ├── ClusterServer (gRPC)
//!   │   ├── Join() → assigns node_id, returns membership
//!   │   ├── Heartbeat() → updates node state, returns commands
//!   │   └── Leave() → drains node, removes from membership
//!   └── MembershipManager
//!       ├── Tracks node status (Ready, Draining, Left)
//!       ├── Detects dead nodes (missed heartbeats)
//!       └── Persists to StateStore
//!
//! Agent Node
//!   └── NodeAgent
//!       ├── Connects to control plane via gRPC
//!       ├── Sends periodic heartbeats
//!       └── Executes commands from control plane
//! ```

pub mod agent;
pub mod membership;
pub mod server;
pub mod tls;

/// Generated protobuf types and gRPC service stubs.
pub mod proto {
    tonic::include_proto!("warpgrid.cluster");
}

pub use agent::NodeAgent;
pub use membership::MembershipManager;
pub use server::ClusterServer;
