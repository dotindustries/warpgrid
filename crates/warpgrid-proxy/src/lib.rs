//! WarpGrid service mesh — L4/L7 proxy, DNS resolution, TLS termination.
//!
//! This crate provides the service mesh infrastructure for routing
//! traffic between services in the WarpGrid cluster.
//!
//! # Components
//!
//! - **`router`** — Request routing with round-robin backend selection
//! - **`dns`** — Internal DNS resolver for service discovery
//! - **`tls`** — TLS termination with SNI-based certificate resolution
//! - **`sync`** — State store → proxy synchronization

pub mod dns;
pub mod router;
pub mod sync;
pub mod tls;

pub use dns::{DnsRecord, DnsResolver};
pub use router::{Backend, Router};
pub use sync::{ProxySync, SyncStats};
pub use tls::{TlsCert, TlsTerminator};
