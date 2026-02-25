//! Guest Wasm component simulating a Rust axum + sqlx handler for T1.
//!
//! This component imports DNS and database-proxy WIT shims and exports
//! test functions that exercise the same shim chain a Rust axum handler
//! compiled to wasm32-wasip2 would use: DNS resolve → database proxy
//! connect → Postgres wire protocol send/recv → close.
//!
//! When Domain 2 (wasi-libc patches) provides a complete Wasm compilation
//! path for axum + sqlx, this Rust guest will be replaced by the actual
//! axum handler compiled with the patched sysroot.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

wit_bindgen::generate!({
    path: "wit",
    world: "rust-http-postgres-test",
    generate_all,
});

use warpgrid::shim::database_proxy;
use warpgrid::shim::dns;

// ── Postgres wire protocol helpers ──────────────────────────────────

/// Build a Postgres v3.0 startup message.
///
/// Format: Int32(length) + Int32(196608) + "user\0<user>\0database\0<db>\0\0"
fn pg_startup_message(user: &str, database: &str) -> Vec<u8> {
    let mut params = Vec::new();
    params.extend_from_slice(b"user\0");
    params.extend_from_slice(user.as_bytes());
    params.push(0);
    params.extend_from_slice(b"database\0");
    params.extend_from_slice(database.as_bytes());
    params.push(0);
    params.push(0); // double null terminator

    let len = (4 + 4 + params.len()) as u32;
    let mut msg = Vec::with_capacity(len as usize);
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&196608u32.to_be_bytes()); // Protocol v3.0
    msg.extend_from_slice(&params);
    msg
}

/// Build a Postgres SimpleQuery message.
///
/// Format: 'Q' + Int32(length) + query_string + NUL
fn pg_simple_query(sql: &str) -> Vec<u8> {
    let query_with_null = format!("{sql}\0");
    let len = (4 + query_with_null.len()) as u32;
    let mut msg = Vec::with_capacity(1 + len as usize);
    msg.push(b'Q');
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(query_with_null.as_bytes());
    msg
}

// ── Shared helpers ──────────────────────────────────────────────────

const DB_HOST: &str = "db.test.warp.local";
const DB_USER: &str = "testuser";
const DB_NAME: &str = "testdb";
const AUTH_OK_LEN: usize = 9; // R + len(8) + 0
const READY_FOR_QUERY_LEN: usize = 6; // Z + len(5) + 'I'
const HANDSHAKE_RESPONSE_LEN: usize = AUTH_OK_LEN + READY_FOR_QUERY_LEN;

/// Resolve the database hostname and return the first IP address.
fn resolve_db_ip() -> Result<String, String> {
    let records = dns::resolve_address(DB_HOST)?;
    records
        .first()
        .map(|r| r.address.clone())
        .ok_or_else(|| format!("no addresses resolved for {DB_HOST}"))
}

/// Connect to the database via the proxy shim.
fn connect_to_db(ip: &str, port: u16) -> Result<u64, String> {
    database_proxy::connect(&database_proxy::ConnectConfig {
        host: String::from(ip),
        port,
        database: String::from(DB_NAME),
        user: String::from(DB_USER),
        password: None,
    })
}

/// Perform the Postgres startup handshake on a proxied connection.
/// Returns the handshake response bytes (AuthOk + ReadyForQuery).
fn do_handshake(handle: u64) -> Result<Vec<u8>, String> {
    let startup = pg_startup_message(DB_USER, DB_NAME);
    database_proxy::send(handle, &startup)?;

    // Read handshake response — may arrive in chunks over TCP.
    let mut response = Vec::new();
    for _ in 0..20 {
        let chunk = database_proxy::recv(handle, 1024)?;
        response.extend_from_slice(&chunk);
        if response.len() >= HANDSHAKE_RESPONSE_LEN {
            break;
        }
    }

    // Validate we got AuthenticationOk (first byte 'R').
    if response.is_empty() || response[0] != b'R' {
        return Err(format!(
            "expected AuthenticationOk (R), got {} bytes starting with {:?}",
            response.len(),
            response.first()
        ));
    }

    Ok(response)
}

/// Send a query and receive the response.
fn send_query(handle: u64, sql: &str) -> Result<Vec<u8>, String> {
    let query = pg_simple_query(sql);
    database_proxy::send(handle, &query)?;

    // Receive response — may arrive in chunks.
    let mut response = Vec::new();
    for _ in 0..20 {
        let chunk = database_proxy::recv(handle, 4096)?;
        if chunk.is_empty() {
            break;
        }
        response.extend_from_slice(&chunk);
        // Check if we got a ReadyForQuery message ('Z') signaling end of response.
        if response.len() >= 6 && response[response.len() - 6] == b'Z' {
            break;
        }
    }

    Ok(response)
}

// ── Guest exports ───────────────────────────────────────────────────

struct Component;

impl Guest for Component {
    /// Test DNS resolution: resolve "db.test.warp.local" and return the IP.
    fn test_resolve_db_host() -> Result<String, String> {
        resolve_db_ip()
    }

    /// Simulate GET /users: DNS → connect → handshake → SELECT → receive rows.
    fn test_get_users(port: u16) -> Result<Vec<u8>, String> {
        let ip = resolve_db_ip()?;
        let handle = connect_to_db(&ip, port)?;
        do_handshake(handle)?;

        let response = send_query(handle, "SELECT id, name FROM test_users ORDER BY id")?;
        database_proxy::close(handle)?;

        Ok(response)
    }

    /// Simulate POST /users then GET /users:
    /// connect → handshake → INSERT → ack → SELECT → rows.
    fn test_post_and_get_users(port: u16) -> Result<Vec<u8>, String> {
        let ip = resolve_db_ip()?;
        let handle = connect_to_db(&ip, port)?;
        do_handshake(handle)?;

        // INSERT a new user.
        let insert_response =
            send_query(handle, "INSERT INTO test_users (name) VALUES ('frank')")?;

        // Verify INSERT acknowledged (CommandComplete starts with 'C').
        if insert_response.is_empty() || insert_response[0] != b'C' {
            return Err(format!(
                "expected CommandComplete (C) for INSERT, got {} bytes starting with {:?}",
                insert_response.len(),
                insert_response.first()
            ));
        }

        // SELECT to verify the new user appears.
        let select_response =
            send_query(handle, "SELECT id, name FROM test_users ORDER BY id")?;
        database_proxy::close(handle)?;

        Ok(select_response)
    }

    /// Test invalid hostname: should fail at DNS resolution.
    fn test_invalid_db_host() -> Result<String, String> {
        match dns::resolve_address("nonexistent.warp.local") {
            Ok(records) if records.is_empty() => {
                Ok("no addresses resolved (empty result)".into())
            }
            Ok(_) => Err("expected DNS failure for nonexistent host, but got addresses".into()),
            Err(e) => Ok(e), // Return the error message as success.
        }
    }

    /// Full proxy round-trip: connect → handshake → send query → recv echo → close.
    /// Validates that axum's TCP data routes through the database proxy.
    fn test_proxy_roundtrip(port: u16) -> Result<Vec<u8>, String> {
        let ip = resolve_db_ip()?;
        let handle = connect_to_db(&ip, port)?;
        do_handshake(handle)?;

        // Send arbitrary data — the mock server echoes it back after handshake.
        let test_data = b"PROXY_ROUNDTRIP_TEST_DATA";
        database_proxy::send(handle, test_data)?;

        let received = database_proxy::recv(handle, 4096)?;
        database_proxy::close(handle)?;

        Ok(received)
    }
}

export!(Component);
