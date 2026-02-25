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
    world: "t5-db-proxy-test",
    generate_all,
});

struct Component;

/// Build a Postgres v3.0 startup message.
fn build_startup_message(database: &str, user: &str) -> Vec<u8> {
    // Format: len(i32) + protocol(i32=196608) + "user\0{user}\0database\0{database}\0\0"
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
fn build_query_message(sql: &str) -> Vec<u8> {
    let sql_bytes = sql.as_bytes();
    // 'Q' + len(i32) + sql + '\0'
    let msg_len = (4 + sql_bytes.len() + 1) as i32;

    let mut buf = Vec::with_capacity(1 + msg_len as usize);
    buf.push(b'Q');
    buf.extend_from_slice(&msg_len.to_be_bytes());
    buf.extend_from_slice(sql_bytes);
    buf.push(0); // null terminator
    buf
}

/// Receive data from the database proxy, accumulating until we see a ReadyForQuery ('Z') marker.
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
        // ReadyForQuery is: 'Z' + len(4 bytes) + status(1 byte)
        if all_data.len() >= 6 {
            for i in 0..all_data.len().saturating_sub(5) {
                if all_data[i] == b'Z' {
                    // Verify it looks like a valid ReadyForQuery
                    if i + 5 < all_data.len() {
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

impl Guest for Component {
    fn test_db_connect(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<String, String> {
        let config = warpgrid::shim::database_proxy::ConnectConfig {
            host,
            port,
            database: database.clone(),
            user: user.clone(),
            password: None,
        };

        let handle = warpgrid::shim::database_proxy::connect(&config)?;

        // Perform Postgres startup handshake
        let startup = build_startup_message(&database, &user);
        warpgrid::shim::database_proxy::send(handle, &startup)?;

        // Read startup response (AuthOk + ReadyForQuery)
        let _response = recv_until_ready(handle)?;

        Ok(format!("{handle}"))
    }

    fn test_db_query(handle_str: String, sql: String) -> Result<Vec<u8>, String> {
        let handle: u64 = handle_str
            .parse()
            .map_err(|e| format!("invalid handle: {e}"))?;

        let query_msg = build_query_message(&sql);
        warpgrid::shim::database_proxy::send(handle, &query_msg)?;

        recv_until_ready(handle)
    }

    fn test_db_close(handle_str: String) -> Result<String, String> {
        let handle: u64 = handle_str
            .parse()
            .map_err(|e| format!("invalid handle: {e}"))?;

        warpgrid::shim::database_proxy::close(handle)?;
        Ok("closed".into())
    }

    fn test_full_lifecycle(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<Vec<u8>, String> {
        // Connect and handshake
        let config = warpgrid::shim::database_proxy::ConnectConfig {
            host,
            port,
            database: database.clone(),
            user: user.clone(),
            password: None,
        };

        let handle = warpgrid::shim::database_proxy::connect(&config)?;

        // Startup handshake
        let startup = build_startup_message(&database, &user);
        warpgrid::shim::database_proxy::send(handle, &startup)?;
        let _startup_response = recv_until_ready(handle)?;

        // Query: SELECT id, name, email FROM test_users ORDER BY id
        let query = build_query_message("SELECT id, name, email FROM test_users ORDER BY id");
        warpgrid::shim::database_proxy::send(handle, &query)?;
        let query_response = recv_until_ready(handle)?;

        // Close connection
        warpgrid::shim::database_proxy::close(handle)?;

        Ok(query_response)
    }
}

export!(Component);
