//! Guest Wasm component for MySQL + Redis integration tests (US-117).
//!
//! This component imports DNS and database-proxy WIT shims and exports
//! test functions that exercise:
//!   - MySQL wire protocol CRUD (CREATE TABLE, INSERT, SELECT, DROP TABLE)
//!   - Redis RESP CRUD (SET, GET, DEL)
//!   - Connection pool reuse for both protocols
//!
//! Both MySQL and Redis connections use the same `database-proxy` WIT
//! interface. The guest sends raw wire protocol bytes for each — MySQL
//! wire protocol to the MySQL server and Redis RESP to the Redis server.
//! The host proxy is protocol-agnostic (pure byte passthrough).

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
    world: "mysql-redis-integration-test",
    generate_all,
});

use warpgrid::shim::database_proxy;
use warpgrid::shim::dns;

// ── Constants ──────────────────────────────────────────────────────

const MYSQL_HOST: &str = "mysql.test.warp.local";
const REDIS_HOST: &str = "redis.test.warp.local";
const MYSQL_USER: &str = "testuser";
const MYSQL_DB: &str = "testdb";

// ── DNS helper ─────────────────────────────────────────────────────

fn resolve_ip(hostname: &str) -> Result<String, String> {
    let records = dns::resolve_address(hostname)?;
    records
        .first()
        .map(|r| r.address.clone())
        .ok_or_else(|| format!("no addresses resolved for {hostname}"))
}

// ── MySQL connection helper ────────────────────────────────────────

fn connect_mysql(ip: &str, port: u16) -> Result<u64, String> {
    database_proxy::connect(&database_proxy::ConnectConfig {
        host: String::from(ip),
        port,
        database: String::from(MYSQL_DB),
        user: String::from(MYSQL_USER),
        password: None,
    })
}

// ── Redis connection helper ────────────────────────────────────────

fn connect_redis(ip: &str, port: u16) -> Result<u64, String> {
    database_proxy::connect(&database_proxy::ConnectConfig {
        host: String::from(ip),
        port,
        database: String::new(),
        user: String::new(),
        password: None,
    })
}

// ── MySQL wire protocol helpers ────────────────────────────────────

/// Read the MySQL server greeting (first packet sent after TCP connect).
/// Returns the greeting bytes.
fn mysql_read_greeting(handle: u64) -> Result<Vec<u8>, String> {
    let mut response = Vec::new();
    for _ in 0..20 {
        let chunk = database_proxy::recv(handle, 4096)?;
        if chunk.is_empty() {
            break;
        }
        response.extend_from_slice(&chunk);
        // MySQL packet: 3-byte length + 1 sequence byte + payload.
        // Once we have the full packet, stop.
        if response.len() >= 4 {
            let pkt_len = (response[0] as usize)
                | ((response[1] as usize) << 8)
                | ((response[2] as usize) << 16);
            if response.len() >= 4 + pkt_len {
                break;
            }
        }
    }
    if response.is_empty() {
        return Err("no greeting received from MySQL server".into());
    }
    Ok(response)
}

/// Build a MySQL client handshake response packet.
/// Minimal handshake: capability flags + max packet size + charset + reserved + username.
fn mysql_handshake_response(seq: u8) -> Vec<u8> {
    let mut payload = Vec::new();

    // Capability flags (4 bytes): CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION
    let caps: u32 = 0x0000_0200 | 0x0000_8000;
    payload.extend_from_slice(&caps.to_le_bytes());

    // Max packet size (4 bytes)
    payload.extend_from_slice(&(16_777_216u32).to_le_bytes());

    // Character set: utf8_general_ci = 33
    payload.push(33);

    // Reserved 23 bytes of zeros
    payload.extend_from_slice(&[0u8; 23]);

    // Username (null-terminated)
    payload.extend_from_slice(MYSQL_USER.as_bytes());
    payload.push(0);

    // Auth response length (0 = no password)
    payload.push(0);

    // Build the full packet: 3-byte length + 1-byte sequence + payload
    let len = payload.len();
    let mut pkt = Vec::with_capacity(4 + len);
    pkt.push((len & 0xFF) as u8);
    pkt.push(((len >> 8) & 0xFF) as u8);
    pkt.push(((len >> 16) & 0xFF) as u8);
    pkt.push(seq);
    pkt.extend_from_slice(&payload);
    pkt
}

/// Perform MySQL handshake: read greeting, send handshake response, read OK.
fn mysql_handshake(handle: u64) -> Result<(), String> {
    // Read server greeting
    let greeting = mysql_read_greeting(handle)?;
    if greeting.len() < 5 {
        return Err(format!("greeting too short: {} bytes", greeting.len()));
    }

    // The greeting's sequence number is 0, so our response should be 1
    let response = mysql_handshake_response(1);
    database_proxy::send(handle, &response)?;

    // Read auth result (OK or ERR packet)
    let auth_result = mysql_read_packet(handle)?;
    if auth_result.len() < 5 {
        return Err(format!("auth result too short: {} bytes", auth_result.len()));
    }
    // Payload starts at byte 4; first payload byte is the status indicator
    let status = auth_result[4];
    if status == 0xFF {
        // ERR packet
        return Err("MySQL authentication failed (ERR packet)".into());
    }
    if status != 0x00 {
        return Err(format!("unexpected auth result status: 0x{:02x}", status));
    }
    Ok(())
}

/// Read a single MySQL packet (3-byte length + 1-byte seq + payload).
fn mysql_read_packet(handle: u64) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    for _ in 0..20 {
        let chunk = database_proxy::recv(handle, 4096)?;
        if chunk.is_empty() {
            break;
        }
        buf.extend_from_slice(&chunk);
        if buf.len() >= 4 {
            let pkt_len = (buf[0] as usize)
                | ((buf[1] as usize) << 8)
                | ((buf[2] as usize) << 16);
            if buf.len() >= 4 + pkt_len {
                break;
            }
        }
    }
    if buf.is_empty() {
        return Err("no MySQL packet received".into());
    }
    Ok(buf)
}

/// Build a COM_QUERY packet (command byte 0x03 + SQL text).
fn mysql_com_query(sql: &str, seq: u8) -> Vec<u8> {
    let payload_len = 1 + sql.len(); // 1 for command byte
    let mut pkt = Vec::with_capacity(4 + payload_len);
    pkt.push((payload_len & 0xFF) as u8);
    pkt.push(((payload_len >> 8) & 0xFF) as u8);
    pkt.push(((payload_len >> 16) & 0xFF) as u8);
    pkt.push(seq);
    pkt.push(0x03); // COM_QUERY
    pkt.extend_from_slice(sql.as_bytes());
    pkt
}

/// Send a COM_QUERY and read the complete response.
/// For result sets, reads until we get an EOF marker after the data rows.
fn mysql_query(handle: u64, sql: &str) -> Result<Vec<u8>, String> {
    let query = mysql_com_query(sql, 0);
    database_proxy::send(handle, &query)?;

    // Read response — may be multiple packets
    let mut response = Vec::new();
    for _ in 0..50 {
        let chunk = database_proxy::recv(handle, 8192)?;
        if chunk.is_empty() {
            break;
        }
        response.extend_from_slice(&chunk);

        // Check if we have a complete response:
        // OK packet (0x00) or ERR packet (0xFF) as first payload byte
        if response.len() >= 5 {
            let first_payload = response[4];
            if first_payload == 0x00 || first_payload == 0xFF {
                // Single-packet response (OK or ERR)
                let pkt_len = (response[0] as usize)
                    | ((response[1] as usize) << 8)
                    | ((response[2] as usize) << 16);
                if response.len() >= 4 + pkt_len {
                    break;
                }
            } else {
                // Result set — look for the final EOF packet (0xFE marker)
                // A result set ends with an EOF packet after the row data.
                // We look for the pattern: pkt_len(3) + seq(1) + 0xFE
                if has_final_eof(&response) {
                    break;
                }
            }
        }
    }
    Ok(response)
}

/// Check if the response buffer contains a final EOF packet.
/// EOF packets have payload starting with 0xFE and are 5 bytes of payload
/// (0xFE + 2 warnings + 2 status flags).
fn has_final_eof(data: &[u8]) -> bool {
    // Walk through MySQL packets looking for EOF markers.
    // We need at least 2 EOF packets for a result set (after columns, after rows).
    let mut pos = 0;
    let mut eof_count = 0;
    while pos + 4 <= data.len() {
        let pkt_len = (data[pos] as usize)
            | ((data[pos + 1] as usize) << 8)
            | ((data[pos + 2] as usize) << 16);
        if pos + 4 + pkt_len > data.len() {
            break; // incomplete packet
        }
        if pkt_len > 0 && data[pos + 4] == 0xFE && pkt_len <= 5 {
            eof_count += 1;
            if eof_count >= 2 {
                return true;
            }
        }
        pos += 4 + pkt_len;
    }
    false
}

/// Check if a MySQL response is an OK packet.
fn is_mysql_ok(data: &[u8]) -> bool {
    data.len() >= 5 && data[4] == 0x00
}

/// Check if a MySQL response is an ERR packet.
fn is_mysql_err(data: &[u8]) -> bool {
    data.len() >= 5 && data[4] == 0xFF
}

/// Extract text data from a MySQL result set response.
/// Parses through the column definitions and row data to extract string values.
fn extract_mysql_row_data(data: &[u8]) -> Result<String, String> {
    // Skip past the column count packet and column definition packets and first EOF
    // to find the row data packets.
    let mut pos = 0;
    let mut eof_count = 0;
    let mut rows = Vec::new();

    while pos + 4 <= data.len() {
        let pkt_len = (data[pos] as usize)
            | ((data[pos + 1] as usize) << 8)
            | ((data[pos + 2] as usize) << 16);
        if pos + 4 + pkt_len > data.len() {
            break;
        }

        let payload_start = pos + 4;
        let payload = &data[payload_start..payload_start + pkt_len];

        if pkt_len > 0 && payload[0] == 0xFE && pkt_len <= 5 {
            eof_count += 1;
            if eof_count >= 2 {
                break; // end of result set
            }
        } else if eof_count == 1 && pkt_len > 0 && payload[0] != 0xFE {
            // This is a row data packet (after first EOF = after column defs)
            // Each column value is length-encoded string
            let mut col_pos = 0;
            let mut col_values = Vec::new();
            while col_pos < payload.len() {
                if payload[col_pos] == 0xFB {
                    // NULL value
                    col_values.push("NULL".into());
                    col_pos += 1;
                } else {
                    // Length-encoded string
                    let (len, bytes_read) = read_lenenc_int(&payload[col_pos..]);
                    col_pos += bytes_read;
                    if col_pos + len <= payload.len() {
                        if let Ok(s) = core::str::from_utf8(&payload[col_pos..col_pos + len]) {
                            col_values.push(String::from(s));
                        }
                        col_pos += len;
                    } else {
                        break;
                    }
                }
            }
            rows.push(col_values.join(","));
        }

        pos += 4 + pkt_len;
    }

    if rows.is_empty() {
        return Err("no row data found in result set".into());
    }
    Ok(rows.join(";"))
}

/// Read a length-encoded integer from a byte slice.
/// Returns (value, bytes_consumed).
fn read_lenenc_int(data: &[u8]) -> (usize, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    match data[0] {
        0..=0xFB => (data[0] as usize, 1), // Note: 0xFB is actually NULL but handled above
        0xFC => {
            if data.len() >= 3 {
                let val = (data[1] as usize) | ((data[2] as usize) << 8);
                (val, 3)
            } else {
                (0, 1)
            }
        }
        0xFD => {
            if data.len() >= 4 {
                let val = (data[1] as usize) | ((data[2] as usize) << 8) | ((data[3] as usize) << 16);
                (val, 4)
            } else {
                (0, 1)
            }
        }
        _ => {
            // 0xFE = 8-byte integer, but unlikely for our test data
            (0, 1)
        }
    }
}

// ── Redis RESP protocol helpers ────────────────────────────────────

/// Build a Redis RESP array command from parts.
fn redis_cmd(parts: &[&str]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(b'*');
    buf.extend_from_slice(format!("{}", parts.len()).as_bytes());
    buf.extend_from_slice(b"\r\n");
    for part in parts {
        buf.push(b'$');
        buf.extend_from_slice(format!("{}", part.len()).as_bytes());
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(part.as_bytes());
        buf.extend_from_slice(b"\r\n");
    }
    buf
}

/// Check if a Redis RESP response is a null bulk string ($-1\r\n).
fn is_redis_null(data: &[u8]) -> bool {
    data.starts_with(b"$-1\r\n")
}

/// Extract the bulk string value from a RESP bulk string response.
fn extract_redis_bulk_string(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data[0] != b'$' {
        return None;
    }
    let mut i = 1;
    while i < data.len() && data[i] != b'\r' {
        i += 1;
    }
    let len_str = core::str::from_utf8(&data[1..i]).ok()?;
    let len: i32 = len_str.parse().ok()?;
    if len < 0 {
        return None;
    }
    let start = i + 2;
    let end = start + len as usize;
    if end > data.len() {
        return None;
    }
    Some(data[start..end].to_vec())
}

fn redis_send_recv(handle: u64, cmd: &[u8]) -> Result<Vec<u8>, String> {
    database_proxy::send(handle, cmd)?;
    database_proxy::recv(handle, 4096)
}

// ── Guest exports ──────────────────────────────────────────────────

struct Component;

impl Guest for Component {
    /// MySQL CRUD: CREATE TABLE → INSERT → SELECT → DROP TABLE.
    fn test_mysql_crud(mysql_port: u16) -> Result<String, String> {
        let ip = resolve_ip(MYSQL_HOST)?;
        let handle = connect_mysql(&ip, mysql_port)?;

        // Perform handshake
        mysql_handshake(handle)?;

        // CREATE TABLE
        let create_resp = mysql_query(handle, "CREATE TABLE test_items (id INT, name VARCHAR(255))")?;
        if is_mysql_err(&create_resp) {
            database_proxy::close(handle)?;
            return Err("CREATE TABLE returned ERR".into());
        }
        if !is_mysql_ok(&create_resp) {
            database_proxy::close(handle)?;
            return Err(format!("CREATE TABLE unexpected response: {:?}", &create_resp[..create_resp.len().min(32)]));
        }

        // INSERT
        let insert_resp = mysql_query(handle, "INSERT INTO test_items (id, name) VALUES (1, 'widget')")?;
        if is_mysql_err(&insert_resp) {
            database_proxy::close(handle)?;
            return Err("INSERT returned ERR".into());
        }
        if !is_mysql_ok(&insert_resp) {
            database_proxy::close(handle)?;
            return Err(format!("INSERT unexpected response: {:?}", &insert_resp[..insert_resp.len().min(32)]));
        }

        // SELECT
        let select_resp = mysql_query(handle, "SELECT id, name FROM test_items")?;
        if is_mysql_err(&select_resp) {
            database_proxy::close(handle)?;
            return Err("SELECT returned ERR".into());
        }

        let row_data = extract_mysql_row_data(&select_resp)?;

        // DROP TABLE
        let drop_resp = mysql_query(handle, "DROP TABLE test_items")?;
        if is_mysql_err(&drop_resp) {
            database_proxy::close(handle)?;
            return Err("DROP TABLE returned ERR".into());
        }

        database_proxy::close(handle)?;

        Ok(row_data)
    }

    /// Redis CRUD: SET → GET → DEL.
    fn test_redis_crud(redis_port: u16) -> Result<String, String> {
        let ip = resolve_ip(REDIS_HOST)?;
        let handle = connect_redis(&ip, redis_port)?;

        // SET
        let set_cmd = redis_cmd(&["SET", "test_key", "hello_redis"]);
        let set_result = redis_send_recv(handle, &set_cmd)?;
        if !set_result.starts_with(b"+OK") {
            database_proxy::close(handle)?;
            return Err(format!(
                "expected +OK for SET, got: {:?}",
                &set_result[..set_result.len().min(32)]
            ));
        }

        // GET
        let get_cmd = redis_cmd(&["GET", "test_key"]);
        let get_result = redis_send_recv(handle, &get_cmd)?;
        if is_redis_null(&get_result) {
            database_proxy::close(handle)?;
            return Err("expected value for GET but got nil".into());
        }
        let value = extract_redis_bulk_string(&get_result).ok_or_else(|| {
            format!(
                "failed to parse Redis bulk string: {:?}",
                &get_result[..get_result.len().min(64)]
            )
        })?;
        let value_str = core::str::from_utf8(&value)
            .map_err(|e| format!("GET value not UTF-8: {e}"))?;
        let result = String::from(value_str);

        // DEL
        let del_cmd = redis_cmd(&["DEL", "test_key"]);
        let del_result = redis_send_recv(handle, &del_cmd)?;
        if !del_result.starts_with(b":1") {
            database_proxy::close(handle)?;
            return Err(format!(
                "expected :1 for DEL, got: {:?}",
                &del_result[..del_result.len().min(32)]
            ));
        }

        database_proxy::close(handle)?;

        Ok(result)
    }

    /// MySQL pool reuse: connect, handshake, query, close, reconnect, handshake, query, close.
    fn test_mysql_pool_reuse(mysql_port: u16) -> Result<String, String> {
        let ip = resolve_ip(MYSQL_HOST)?;

        // First connection
        let h1 = connect_mysql(&ip, mysql_port)?;
        mysql_handshake(h1)?;
        let resp1 = mysql_query(h1, "SELECT 1")?;
        if is_mysql_err(&resp1) {
            database_proxy::close(h1)?;
            return Err("first SELECT 1 returned ERR".into());
        }
        database_proxy::close(h1)?;

        // Second connection (should reuse pool)
        let h2 = connect_mysql(&ip, mysql_port)?;
        mysql_handshake(h2)?;
        let resp2 = mysql_query(h2, "SELECT 2")?;
        if is_mysql_err(&resp2) {
            database_proxy::close(h2)?;
            return Err("second SELECT 2 returned ERR".into());
        }
        database_proxy::close(h2)?;

        Ok(String::from("mysql_pool_reuse:ok"))
    }

    /// Redis pool reuse: connect, SET, close, reconnect, GET (verify value), close.
    fn test_redis_pool_reuse(redis_port: u16) -> Result<String, String> {
        let ip = resolve_ip(REDIS_HOST)?;

        // First connection: SET a value
        let h1 = connect_redis(&ip, redis_port)?;
        let set_cmd = redis_cmd(&["SET", "pool_key", "pool_value"]);
        let set_result = redis_send_recv(h1, &set_cmd)?;
        if !set_result.starts_with(b"+OK") {
            database_proxy::close(h1)?;
            return Err(format!("SET failed: {:?}", &set_result));
        }
        database_proxy::close(h1)?;

        // Second connection: GET the value (server maintains state)
        let h2 = connect_redis(&ip, redis_port)?;
        let get_cmd = redis_cmd(&["GET", "pool_key"]);
        let get_result = redis_send_recv(h2, &get_cmd)?;
        if is_redis_null(&get_result) {
            database_proxy::close(h2)?;
            return Err("expected value for GET after pool reuse but got nil".into());
        }
        let value = extract_redis_bulk_string(&get_result).ok_or_else(|| {
            format!(
                "failed to parse GET result: {:?}",
                &get_result[..get_result.len().min(64)]
            )
        })?;
        let value_str = core::str::from_utf8(&value)
            .map_err(|e| format!("GET value not UTF-8: {e}"))?;
        if value_str != "pool_value" {
            database_proxy::close(h2)?;
            return Err(format!("expected 'pool_value', got '{value_str}'"));
        }
        database_proxy::close(h2)?;

        Ok(String::from("redis_pool_reuse:ok"))
    }
}

export!(Component);
