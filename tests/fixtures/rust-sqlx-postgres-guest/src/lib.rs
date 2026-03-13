#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

wit_bindgen::generate!({
    path: "wit",
    world: "rust-sqlx-postgres-test",
    generate_all,
});

struct Component;

// ── Postgres wire protocol helpers ──────────────────────────────────

/// Build a Postgres v3.0 startup message.
///
/// Format: len(i32) + protocol(i32=196608) + "user\0{user}\0database\0{database}\0\0"
fn build_startup_message(database: &str, user: &str) -> Vec<u8> {
    let params = format!("user\0{user}\0database\0{database}\0\0");
    let params_bytes = params.as_bytes();
    let total_len = (4 + 4 + params_bytes.len()) as i32;

    let mut buf = Vec::with_capacity(total_len as usize);
    buf.extend_from_slice(&total_len.to_be_bytes());
    buf.extend_from_slice(&196608_i32.to_be_bytes()); // protocol 3.0
    buf.extend_from_slice(params_bytes);
    buf
}

/// Build a Postgres simple query message ('Q').
///
/// Format: 'Q' + Int32(len) + sql + '\0'
fn build_query_message(sql: &str) -> Vec<u8> {
    let sql_bytes = sql.as_bytes();
    let msg_len = (4 + sql_bytes.len() + 1) as i32;

    let mut buf = Vec::with_capacity(1 + msg_len as usize);
    buf.push(b'Q');
    buf.extend_from_slice(&msg_len.to_be_bytes());
    buf.extend_from_slice(sql_bytes);
    buf.push(0); // null terminator
    buf
}

/// Receive data from the database proxy, accumulating until we see a ReadyForQuery ('Z') marker.
///
/// ReadyForQuery format: 'Z' + Int32(5) + status(1 byte)
fn recv_until_ready(handle: u64) -> Result<Vec<u8>, String> {
    let mut all_data: Vec<u8> = Vec::new();
    let max_iterations = 100;

    for _ in 0..max_iterations {
        let chunk = warpgrid::shim::database_proxy::recv(handle, 65536)?;
        if chunk.is_empty() {
            break;
        }
        all_data.extend_from_slice(&chunk);

        // Check for ReadyForQuery marker ('Z' = 0x5A)
        if all_data.len() >= 6 {
            for i in 0..all_data.len().saturating_sub(5) {
                if all_data[i] == b'Z' {
                    if i + 5 <= all_data.len() {
                        let len = i32::from_be_bytes([
                            all_data[i + 1],
                            all_data[i + 2],
                            all_data[i + 3],
                            all_data[i + 4],
                        ]);
                        if len == 5 {
                            return Ok(all_data);
                        }
                    }
                }
            }
        }
    }

    Ok(all_data)
}

/// Connect and perform Postgres startup handshake.
/// Returns the connection handle on success.
fn connect_and_handshake(
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Result<u64, String> {
    let config = warpgrid::shim::database_proxy::ConnectConfig {
        host: String::from(host),
        port,
        database: String::from(database),
        user: String::from(user),
        password: None,
    };

    let handle = warpgrid::shim::database_proxy::connect(&config)?;

    // Send startup handshake
    let startup = build_startup_message(database, user);
    warpgrid::shim::database_proxy::send(handle, &startup)?;

    // Read AuthOk + ReadyForQuery
    let _response = recv_until_ready(handle)?;

    Ok(handle)
}

/// Send a query and receive the full response up to ReadyForQuery.
fn send_query(handle: u64, sql: &str) -> Result<Vec<u8>, String> {
    let query_msg = build_query_message(sql);
    warpgrid::shim::database_proxy::send(handle, &query_msg)?;
    recv_until_ready(handle)
}

// ── Guest exports ───────────────────────────────────────────────────

impl Guest for Component {
    /// Full DDL lifecycle: CREATE TABLE → INSERT rows → SELECT → DROP TABLE.
    ///
    /// Returns the raw SELECT response bytes for host-side wire protocol verification.
    fn test_create_insert_select_drop(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<Vec<u8>, String> {
        let handle = connect_and_handshake(&host, port, &database, &user)?;

        // CREATE TABLE
        let _create_resp = send_query(
            handle,
            "CREATE TABLE test_users (id SERIAL PRIMARY KEY, name TEXT NOT NULL, email TEXT)",
        )?;

        // INSERT two rows
        let _insert1 = send_query(
            handle,
            "INSERT INTO test_users (name, email) VALUES ('alice', 'alice@test.com')",
        )?;
        let _insert2 = send_query(
            handle,
            "INSERT INTO test_users (name, email) VALUES ('bob', 'bob@test.com')",
        )?;

        // SELECT — capture response for host-side verification
        let select_response = send_query(
            handle,
            "SELECT id, name, email FROM test_users ORDER BY id",
        )?;

        // DROP TABLE
        let _drop_resp = send_query(handle, "DROP TABLE test_users")?;

        // Close connection
        warpgrid::shim::database_proxy::close(handle)?;

        Ok(select_response)
    }

    /// Multiple queries on the same connection handle to verify connection reuse.
    ///
    /// Executes SELECT 1, SELECT 2, SELECT 3 sequentially and returns all response data.
    fn test_connection_reuse(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<Vec<u8>, String> {
        let handle = connect_and_handshake(&host, port, &database, &user)?;

        let mut all_responses: Vec<u8> = Vec::new();

        // Execute 3 queries on the same handle
        for i in 1..=3 {
            let sql = format!("SELECT {i}");
            let response = send_query(handle, &sql)?;
            all_responses.extend_from_slice(&response);
        }

        warpgrid::shim::database_proxy::close(handle)?;

        Ok(all_responses)
    }

    /// Close and reconnect to verify pooled connection reuse.
    ///
    /// 1. Connect, handshake, query, close
    /// 2. Reconnect, handshake, query, close
    /// Returns both query responses concatenated.
    fn test_reconnect_after_close(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<Vec<u8>, String> {
        let mut all_responses: Vec<u8> = Vec::new();

        // First connection cycle
        let h1 = connect_and_handshake(&host, port, &database, &user)?;
        let resp1 = send_query(h1, "SELECT 'first_connection'")?;
        all_responses.extend_from_slice(&resp1);
        warpgrid::shim::database_proxy::close(h1)?;

        // Second connection cycle — should reuse pooled connection
        let h2 = connect_and_handshake(&host, port, &database, &user)?;
        let resp2 = send_query(h2, "SELECT 'second_connection'")?;
        all_responses.extend_from_slice(&resp2);
        warpgrid::shim::database_proxy::close(h2)?;

        Ok(all_responses)
    }
}

export!(Component);
