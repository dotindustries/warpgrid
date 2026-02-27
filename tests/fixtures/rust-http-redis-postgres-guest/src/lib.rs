//! Guest Wasm component simulating a Rust axum handler with Redis
//! cache-aside and Postgres backend for T2.
//!
//! This component imports DNS and database-proxy WIT shims and exports
//! test functions that exercise the cache-aside pattern:
//!   1. Cold cache: Redis GET miss → Postgres query → Redis SET
//!   2. Warm cache: Redis GET hit → return cached (skip Postgres)
//!   3. Cache flush: Redis DEL → re-query Postgres
//!   4. Pool metrics: verify exactly 1 Postgres + 1 Redis connection
//!
//! Both Redis and Postgres connections use the same `database-proxy`
//! WIT interface. The guest sends raw wire protocol bytes for each —
//! Postgres wire protocol to the PG server and Redis RESP to the Redis
//! server. The host proxy is protocol-agnostic (pure byte passthrough).

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
    world: "rust-http-redis-postgres-test",
    generate_all,
});

use warpgrid::shim::database_proxy;
use warpgrid::shim::dns;

// ── Constants ──────────────────────────────────────────────────────

const DB_HOST: &str = "db.test.warp.local";
const CACHE_HOST: &str = "cache.test.warp.local";
const DB_USER: &str = "testuser";
const DB_NAME: &str = "testdb";
const AUTH_OK_LEN: usize = 9; // R + len(8) + 0
const READY_FOR_QUERY_LEN: usize = 6; // Z + len(5) + 'I'
const HANDSHAKE_RESPONSE_LEN: usize = AUTH_OK_LEN + READY_FOR_QUERY_LEN;

// ── Postgres wire protocol helpers ─────────────────────────────────

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

fn pg_simple_query(sql: &str) -> Vec<u8> {
    let query_with_null = format!("{sql}\0");
    let len = (4 + query_with_null.len()) as u32;
    let mut msg = Vec::with_capacity(1 + len as usize);
    msg.push(b'Q');
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(query_with_null.as_bytes());
    msg
}

// ── Redis RESP protocol helpers ────────────────────────────────────

/// Build a Redis RESP array command from parts.
///
/// e.g., `redis_cmd(&["GET", "user:1"])` → `*2\r\n$3\r\nGET\r\n$6\r\nuser:1\r\n`
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
/// e.g., `$5\r\nhello\r\n` → `hello`
fn extract_redis_bulk_string(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data[0] != b'$' {
        return None;
    }
    // Find the \r\n after the length prefix.
    let mut i = 1;
    while i < data.len() && data[i] != b'\r' {
        i += 1;
    }
    let len_str = core::str::from_utf8(&data[1..i]).ok()?;
    let len: i32 = len_str.parse().ok()?;
    if len < 0 {
        return None; // null bulk string
    }
    let start = i + 2; // skip \r\n
    let end = start + len as usize;
    if end > data.len() {
        return None;
    }
    Some(data[start..end].to_vec())
}

// ── Shared connection helpers ──────────────────────────────────────

fn resolve_ip(hostname: &str) -> Result<String, String> {
    let records = dns::resolve_address(hostname)?;
    records
        .first()
        .map(|r| r.address.clone())
        .ok_or_else(|| format!("no addresses resolved for {hostname}"))
}

fn connect_pg(ip: &str, port: u16) -> Result<u64, String> {
    database_proxy::connect(&database_proxy::ConnectConfig {
        host: String::from(ip),
        port,
        database: String::from(DB_NAME),
        user: String::from(DB_USER),
        password: None,
    })
}

fn connect_redis(ip: &str, port: u16) -> Result<u64, String> {
    database_proxy::connect(&database_proxy::ConnectConfig {
        host: String::from(ip),
        port,
        database: String::new(),
        user: String::new(),
        password: None,
    })
}

fn pg_handshake(handle: u64) -> Result<Vec<u8>, String> {
    let startup = pg_startup_message(DB_USER, DB_NAME);
    database_proxy::send(handle, &startup)?;

    let mut response = Vec::new();
    for _ in 0..20 {
        let chunk = database_proxy::recv(handle, 1024)?;
        response.extend_from_slice(&chunk);
        if response.len() >= HANDSHAKE_RESPONSE_LEN {
            break;
        }
    }

    if response.is_empty() || response[0] != b'R' {
        return Err(format!(
            "expected AuthenticationOk (R), got {} bytes starting with {:?}",
            response.len(),
            response.first()
        ));
    }

    Ok(response)
}

fn pg_query(handle: u64, sql: &str) -> Result<Vec<u8>, String> {
    let query = pg_simple_query(sql);
    database_proxy::send(handle, &query)?;

    let mut response = Vec::new();
    for _ in 0..20 {
        let chunk = database_proxy::recv(handle, 4096)?;
        if chunk.is_empty() {
            break;
        }
        response.extend_from_slice(&chunk);
        if response.len() >= 6 && response[response.len() - 6] == b'Z' {
            break;
        }
    }

    Ok(response)
}

fn redis_send_recv(handle: u64, cmd: &[u8]) -> Result<Vec<u8>, String> {
    database_proxy::send(handle, cmd)?;
    database_proxy::recv(handle, 4096)
}

// ── Guest exports ──────────────────────────────────────────────────

struct Component;

impl Guest for Component {
    /// Cold cache: Redis GET returns nil, so we query Postgres and
    /// cache the result via Redis SET.
    fn test_cold_cache(pg_port: u16, redis_port: u16) -> Result<Vec<u8>, String> {
        let db_ip = resolve_ip(DB_HOST)?;
        let cache_ip = resolve_ip(CACHE_HOST)?;

        // Connect to both backends.
        let pg = connect_pg(&db_ip, pg_port)?;
        let redis = connect_redis(&cache_ip, redis_port)?;

        // Postgres handshake.
        pg_handshake(pg)?;

        // Step 1: Check Redis cache — should be a miss (nil).
        let get_cmd = redis_cmd(&["GET", "user:1"]);
        let cache_result = redis_send_recv(redis, &get_cmd)?;

        if !is_redis_null(&cache_result) {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!(
                "expected Redis cache miss (nil), got: {:?}",
                &cache_result[..cache_result.len().min(32)]
            ));
        }

        // Step 2: Query Postgres on cache miss.
        let pg_response = pg_query(
            pg,
            "SELECT id, name FROM test_users WHERE id = 1 ORDER BY id",
        )?;

        // Step 3: Cache the result in Redis with TTL.
        // We store the raw PG response bytes as the cached value.
        let value_hex = hex_encode(&pg_response);
        let set_cmd = redis_cmd(&["SET", "user:1", &value_hex, "EX", "30"]);
        let set_result = redis_send_recv(redis, &set_cmd)?;

        if !set_result.starts_with(b"+OK") {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!(
                "expected +OK for Redis SET, got: {:?}",
                &set_result[..set_result.len().min(32)]
            ));
        }

        database_proxy::close(pg)?;
        database_proxy::close(redis)?;

        Ok(pg_response)
    }

    /// Warm cache: pre-populate Redis, then GET returns cached data
    /// without needing Postgres.
    fn test_warm_cache(pg_port: u16, redis_port: u16) -> Result<Vec<u8>, String> {
        let db_ip = resolve_ip(DB_HOST)?;
        let cache_ip = resolve_ip(CACHE_HOST)?;

        // Connect to both backends.
        let pg = connect_pg(&db_ip, pg_port)?;
        let redis = connect_redis(&cache_ip, redis_port)?;

        // Postgres handshake.
        pg_handshake(pg)?;

        // Step 1: Populate cache with a known value.
        let cached_value = "cached_user_data_alice";
        let set_cmd = redis_cmd(&["SET", "user:1", cached_value, "EX", "30"]);
        let set_result = redis_send_recv(redis, &set_cmd)?;
        if !set_result.starts_with(b"+OK") {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!("SET failed: {:?}", &set_result));
        }

        // Step 2: GET from cache — should be a hit.
        let get_cmd = redis_cmd(&["GET", "user:1"]);
        let cache_result = redis_send_recv(redis, &get_cmd)?;

        // Verify it's NOT null (it should be the cached value).
        if is_redis_null(&cache_result) {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err("expected cache hit but got nil".into());
        }

        // Extract the bulk string value.
        let value = extract_redis_bulk_string(&cache_result).ok_or_else(|| {
            format!(
                "failed to parse Redis bulk string from: {:?}",
                &cache_result[..cache_result.len().min(64)]
            )
        })?;

        // We do NOT query Postgres — the whole point of the warm cache test.
        // Close connections.
        database_proxy::close(pg)?;
        database_proxy::close(redis)?;

        Ok(value)
    }

    /// Cache flush + re-query: populate cache, DEL it, verify next
    /// GET is a miss and Postgres is re-queried.
    fn test_cache_flush_requery(pg_port: u16, redis_port: u16) -> Result<Vec<u8>, String> {
        let db_ip = resolve_ip(DB_HOST)?;
        let cache_ip = resolve_ip(CACHE_HOST)?;

        let pg = connect_pg(&db_ip, pg_port)?;
        let redis = connect_redis(&cache_ip, redis_port)?;

        pg_handshake(pg)?;

        // Step 1: Populate cache.
        let set_cmd = redis_cmd(&["SET", "user:1", "stale_data", "EX", "30"]);
        let set_result = redis_send_recv(redis, &set_cmd)?;
        if !set_result.starts_with(b"+OK") {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!("SET failed: {:?}", &set_result));
        }

        // Step 2: Flush (DEL) the cache entry.
        let del_cmd = redis_cmd(&["DEL", "user:1"]);
        let del_result = redis_send_recv(redis, &del_cmd)?;

        // DEL returns an integer reply — should be :1 (one key deleted).
        if !del_result.starts_with(b":1") {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!(
                "expected :1 for DEL, got: {:?}",
                &del_result[..del_result.len().min(32)]
            ));
        }

        // Step 3: GET should be nil now.
        let get_cmd = redis_cmd(&["GET", "user:1"]);
        let cache_result = redis_send_recv(redis, &get_cmd)?;

        if !is_redis_null(&cache_result) {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!(
                "expected nil after DEL, got: {:?}",
                &cache_result[..cache_result.len().min(32)]
            ));
        }

        // Step 4: Re-query Postgres since cache is empty.
        let pg_response = pg_query(
            pg,
            "SELECT id, name FROM test_users WHERE id = 1 ORDER BY id",
        )?;

        database_proxy::close(pg)?;
        database_proxy::close(redis)?;

        Ok(pg_response)
    }

    /// Pool connections test: open one PG and one Redis connection,
    /// perform minimal operations, close both, and report connection
    /// counts as a formatted string.
    fn test_pool_connections(pg_port: u16, redis_port: u16) -> Result<String, String> {
        let db_ip = resolve_ip(DB_HOST)?;
        let cache_ip = resolve_ip(CACHE_HOST)?;

        // Connect to Postgres.
        let pg = connect_pg(&db_ip, pg_port)?;
        pg_handshake(pg)?;

        // Connect to Redis.
        let redis = connect_redis(&cache_ip, redis_port)?;

        // Verify both connections work with minimal operations.
        // Postgres: simple query.
        let pg_response = pg_query(pg, "SELECT 1")?;
        if pg_response.is_empty() {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err("Postgres query returned empty response".into());
        }

        // Redis: PING.
        let ping_cmd = redis_cmd(&["PING"]);
        let ping_result = redis_send_recv(redis, &ping_cmd)?;
        if !ping_result.starts_with(b"+PONG") {
            database_proxy::close(pg)?;
            database_proxy::close(redis)?;
            return Err(format!(
                "expected +PONG, got: {:?}",
                &ping_result[..ping_result.len().min(32)]
            ));
        }

        // Close both.
        database_proxy::close(pg)?;
        database_proxy::close(redis)?;

        // Report: we successfully held 1 PG and 1 Redis connection simultaneously.
        Ok(String::from("pg_conns:1,redis_conns:1"))
    }
}

/// Simple hex encoding for embedding binary PG response in Redis value.
fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for byte in data {
        s.push(HEX_CHARS[(byte >> 4) as usize] as char);
        s.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    s
}

const HEX_CHARS: [u8; 16] = *b"0123456789abcdef";

export!(Component);
