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
    world: "notification-service",
    generate_all,
});

use warpgrid::shim::database_proxy;
use warpgrid::shim::filesystem;

struct Component;

/// Parse Redis config from virtual filesystem.
/// Config format: "host port" (space-separated).
fn read_redis_config() -> Result<database_proxy::ConnectConfig, String> {
    let handle = filesystem::open_virtual("/etc/warpgrid/redis.conf")?;
    let data = filesystem::read_virtual(handle, 4096)?;
    filesystem::close_virtual(handle)?;

    let text = core::str::from_utf8(&data).map_err(|e| format!("invalid utf8: {e}"))?;
    let parts: Vec<&str> = text.trim().split(' ').collect();
    if parts.len() < 2 {
        return Err(format!("invalid redis config: expected 2 parts, got {}", parts.len()));
    }

    let port: u16 = parts[1]
        .parse()
        .map_err(|e| format!("invalid port: {e}"))?;

    Ok(database_proxy::ConnectConfig {
        host: String::from(parts[0]),
        port,
        database: String::new(),
        user: String::new(),
        password: None,
    })
}

/// Build a Redis RESP array command from parts.
fn redis_command(parts: &[&str]) -> Vec<u8> {
    let mut cmd = Vec::new();
    // Array header: *<count>\r\n
    cmd.push(b'*');
    let count_str = format!("{}", parts.len());
    cmd.extend_from_slice(count_str.as_bytes());
    cmd.extend_from_slice(b"\r\n");

    for part in parts {
        // Bulk string: $<len>\r\n<data>\r\n
        cmd.push(b'$');
        let len_str = format!("{}", part.len());
        cmd.extend_from_slice(len_str.as_bytes());
        cmd.extend_from_slice(b"\r\n");
        cmd.extend_from_slice(part.as_bytes());
        cmd.extend_from_slice(b"\r\n");
    }
    cmd
}

impl Guest for Component {
    /// Enqueues notifications to Redis via RPUSH.
    ///
    /// POST /notify — pushes notification body to Redis list, returns 202
    fn handle_request(
        _method: String,
        _path: String,
        body: String,
    ) -> Result<(u16, String, String), String> {
        let config = read_redis_config()?;
        let handle = database_proxy::connect(&config)?;

        // Build RPUSH command: RPUSH notifications <body>
        let cmd = redis_command(&["RPUSH", "notifications", &body]);
        database_proxy::send(handle, &cmd)?;

        // Read response (mock server echoes non-PING commands).
        let resp = database_proxy::recv(handle, 4096)?;
        let resp_str = String::from_utf8(resp.into())
            .unwrap_or_else(|_| String::from("<binary>"));

        database_proxy::close(handle)?;
        Ok((202, format!("notification_enqueued:{resp_str}"), String::new()))
    }
}

export!(Component);
