//! Individual shim implementations.
//! Each shim is a Wasmtime host function registered at runtime.

pub mod database;
pub mod dns;
pub mod filesystem;

// Phase 2: These will contain actual Wasmtime host function implementations.
