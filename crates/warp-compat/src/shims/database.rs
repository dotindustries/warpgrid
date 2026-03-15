//! Database proxy shim — transparent TCP interception for Postgres/MySQL/Redis.
//!
//! The actual database proxy implementation lives in `warpgrid-host::db_proxy`.
//! This shim serves as the compatibility layer marker.
//!
//! # Async I/O (US-506)
//!
//! The database proxy supports async I/O via `tokio::net::TcpStream`. When an
//! `AsyncConnectionFactory` is provided to the engine, connections use non-blocking
//! I/O that releases the pool mutex during send/recv operations, enabling concurrent
//! query execution across multiple connections within a single Wasm instance.
//!
//! The sync path is preserved as a fallback — if no async factory is configured,
//! the proxy uses blocking `std::net::TcpStream` with `block_in_place`.

pub struct DatabaseShim;

impl DatabaseShim {
    pub fn new() -> Self { Self }
}
