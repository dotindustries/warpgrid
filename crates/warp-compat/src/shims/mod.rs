//! Individual shim implementations.
//! Each shim is a Wasmtime host function registered at runtime.

pub mod filesystem;
pub mod dns;
pub mod database;

// Phase 2: These will contain actual Wasmtime host function implementations.
