//! Threading model declaration shim.
//!
//! Allows guest modules to declare their threading expectations
//! (cooperative, parallel-required) so the host can warn about
//! incompatibilities.
//!
//! The [`host`] submodule provides the WIT `Host` trait implementation
//! that validates and stores the declared threading model.

pub mod host;

pub use host::ThreadingHost;
