//! Signal delivery shim.
//!
//! Provides lifecycle signal handling (SIGTERM, SIGHUP, SIGINT) for Wasm modules
//! via a non-blocking poll interface.
