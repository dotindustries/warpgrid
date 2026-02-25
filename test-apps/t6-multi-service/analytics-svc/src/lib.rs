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
    path: "../wit",
    world: "analytics-service",
    generate_all,
});

use warpgrid::shim::database_proxy;
use warpgrid::shim::filesystem;

struct Component;

/// Parse DB config from virtual filesystem.
/// Config format: "host port database user" (space-separated).
fn read_db_config() -> Result<database_proxy::ConnectConfig, String> {
    let handle = filesystem::open_virtual("/etc/warpgrid/db.conf")?;
    let data = filesystem::read_virtual(handle, 4096)?;
    filesystem::close_virtual(handle)?;

    let text = core::str::from_utf8(&data).map_err(|e| format!("invalid utf8: {e}"))?;
    let parts: Vec<&str> = text.trim().split(' ').collect();
    if parts.len() < 4 {
        return Err(format!("invalid db config: expected 4 parts, got {}", parts.len()));
    }

    let port: u16 = parts[1]
        .parse()
        .map_err(|e| format!("invalid port: {e}"))?;

    Ok(database_proxy::ConnectConfig {
        host: String::from(parts[0]),
        port,
        database: String::from(parts[2]),
        user: String::from(parts[3]),
        password: None,
    })
}

/// Build a Postgres v3.0 startup message.
fn pg_startup_message(user: &str, database: &str) -> Vec<u8> {
    let mut params = Vec::new();
    params.extend_from_slice(b"user\0");
    params.extend_from_slice(user.as_bytes());
    params.push(0);
    params.extend_from_slice(b"database\0");
    params.extend_from_slice(database.as_bytes());
    params.push(0);
    params.push(0); // Terminator.

    let length = (4 + 4 + params.len()) as u32;
    let mut msg = Vec::with_capacity(length as usize);
    msg.extend_from_slice(&length.to_be_bytes());
    msg.extend_from_slice(&196608u32.to_be_bytes()); // Protocol v3.0
    msg.extend_from_slice(&params);
    msg
}

/// Build a Postgres SimpleQuery message: 'Q' + length + query\0
fn pg_simple_query(query: &str) -> Vec<u8> {
    let query_bytes = query.as_bytes();
    let length = (4 + query_bytes.len() + 1) as u32;
    let mut msg = Vec::with_capacity(1 + length as usize);
    msg.push(b'Q');
    msg.extend_from_slice(&length.to_be_bytes());
    msg.extend_from_slice(query_bytes);
    msg.push(0);
    msg
}

/// Perform Postgres startup handshake on a connection handle.
fn do_pg_handshake(handle: u64, user: &str, database: &str) -> Result<Vec<u8>, String> {
    let startup = pg_startup_message(user, database);
    database_proxy::send(handle, &startup)?;

    let mut response = Vec::new();
    for _ in 0..10 {
        let chunk = database_proxy::recv(handle, 1024)?;
        response.extend_from_slice(&chunk);
        if response.len() >= 15 {
            break;
        }
    }
    Ok(response)
}

impl Guest for Component {
    /// Records analytics events to Postgres.
    ///
    /// POST /analytics/event — inserts an event row, returns 201
    fn handle_request(
        _method: String,
        _path: String,
        body: String,
    ) -> Result<(u16, String, String), String> {
        let config = read_db_config()?;
        let handle = database_proxy::connect(&config)?;

        // Perform Postgres startup handshake.
        let handshake_resp = do_pg_handshake(handle, &config.user, &config.database)?;
        if handshake_resp.is_empty() || handshake_resp[0] != b'R' {
            database_proxy::close(handle)?;
            return Ok((503, String::from("postgres handshake failed"), String::new()));
        }

        // Insert analytics event.
        let query = format!(
            "INSERT INTO test_analytics (event_type, payload) VALUES ('page_view', '{body}')"
        );
        let msg = pg_simple_query(&query);
        database_proxy::send(handle, &msg)?;

        let resp = database_proxy::recv(handle, 4096)?;
        let resp_str = String::from_utf8(resp.into())
            .unwrap_or_else(|_| String::from("<binary>"));

        database_proxy::close(handle)?;
        Ok((201, format!("event_recorded:{resp_str}"), String::new()))
    }
}

export!(Component);
