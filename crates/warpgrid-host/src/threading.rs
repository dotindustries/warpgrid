//! Threading model declaration shim.
//!
//! Allows guest modules to declare their threading expectations
//! (single-threaded, cooperative, parallel-required) so the host
//! can warn about incompatibilities.
