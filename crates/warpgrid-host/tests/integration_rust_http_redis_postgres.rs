//! US-703: Rust HTTP + Redis + Postgres integration test (T2).
//!
//! End-to-end test that extends T1 by adding a Redis caching layer with two
//! simultaneous proxied connections from a single Wasm module. The guest
//! component exercises a cache-aside pattern:
//!   - Cold cache: Redis GET miss → Postgres SELECT → Redis SET
//!   - Warm cache: Redis GET hit → return cached (skip Postgres)
//!   - TTL expiry: Redis DEL → re-query Postgres
//!   - Pool metrics: exactly 1 Postgres + 1 Redis connection
//!
//! Test stack:
//! ```text
//! Guest Component (Wasm)
//!   ↓ WIT import: dns.resolve-address
//! DnsHost → CachedDnsResolver → DnsResolver (service registry)
//!   ↓ WIT import: database-proxy.connect/send/recv/close
//! DbProxyHost → ConnectionPoolManager → TcpConnectionFactory → TCP
//!   ├── MockPostgresServer (port A)
//!   └── StatefulMockRedisServer (port B)
//! ```

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::config::ShimConfig;
use warpgrid_host::db_proxy::tcp::TcpConnectionFactory;
use warpgrid_host::db_proxy::PoolConfig;
use warpgrid_host::engine::WarpGridEngine;

// ── Postgres protocol helpers (same as T1) ──────────────────────────

fn pg_row_description(fields: &[(&str, i32)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(fields.len() as i16).to_be_bytes());
    for (name, type_oid) in fields {
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(&0i32.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&type_oid.to_be_bytes());
        body.extend_from_slice(&4i16.to_be_bytes());
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
    }
    let len = (4 + body.len()) as u32;
    let mut msg = vec![b'T'];
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&body);
    msg
}

fn pg_data_row(values: &[&str]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(values.len() as i16).to_be_bytes());
    for val in values {
        let bytes = val.as_bytes();
        body.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
        body.extend_from_slice(bytes);
    }
    let len = (4 + body.len()) as u32;
    let mut msg = vec![b'D'];
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&body);
    msg
}

fn pg_command_complete(tag: &str) -> Vec<u8> {
    let tag_bytes = format!("{tag}\0");
    let len = (4 + tag_bytes.len()) as u32;
    let mut msg = vec![b'C'];
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(tag_bytes.as_bytes());
    msg
}

fn pg_ready_for_query() -> Vec<u8> {
    vec![b'Z', 0, 0, 0, 5, b'I']
}

/// AuthenticationOk: 'R' + Int32(8) + Int32(0)
const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];

/// ReadyForQuery: 'Z' + Int32(5) + 'I' (idle)
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

// ── QueryTrackingMockPostgres ───────────────────────────────────────

/// Mock Postgres that tracks which queries it received.
/// This lets tests verify whether Postgres was queried (cold cache)
/// or not queried (warm cache).
struct QueryTrackingMockPostgres {
    addr: std::net::SocketAddr,
    query_count: Arc<Mutex<u32>>,
}

impl QueryTrackingMockPostgres {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");
        let query_count = Arc::new(Mutex::new(0u32));
        let count_clone = Arc::clone(&query_count);

        let select_resp = Self::build_select_response();

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let count = Arc::clone(&count_clone);
                let select_resp = select_resp.clone();
                std::thread::spawn(move || {
                    Self::handle_connection(&mut stream, &select_resp, &count);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr, query_count }
    }

    fn query_count(&self) -> u32 {
        *self.query_count.lock().unwrap()
    }

    fn build_select_response() -> Vec<u8> {
        let mut response = Vec::new();
        // Single-row response for "WHERE id = 1" queries.
        response.extend(pg_row_description(&[("id", 23), ("name", 1043)]));
        response.extend(pg_data_row(&["1", "alice"]));
        response.extend(pg_command_complete("SELECT 1"));
        response.extend(pg_ready_for_query());
        response
    }

    fn read_startup_message(stream: &mut std::net::TcpStream) -> Result<(), std::io::Error> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if !(8..=10_000).contains(&len) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid startup message length",
            ));
        }
        let mut payload = vec![0u8; len - 4];
        stream.read_exact(&mut payload)?;
        Ok(())
    }

    fn handle_connection(
        stream: &mut std::net::TcpStream,
        select_resp: &[u8],
        query_count: &Mutex<u32>,
    ) {
        if Self::read_startup_message(stream).is_err() {
            return;
        }

        if stream.write_all(&AUTH_OK).is_err() {
            return;
        }
        if stream.write_all(&READY_FOR_QUERY).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        loop {
            let mut type_buf = [0u8; 1];
            if stream.read_exact(&mut type_buf).is_err() {
                break;
            }

            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            let payload_len = len.saturating_sub(4);
            let mut payload = vec![0u8; payload_len];
            if payload_len > 0 && stream.read_exact(&mut payload).is_err() {
                break;
            }

            match type_buf[0] {
                b'Q' => {
                    *query_count.lock().unwrap() += 1;
                    if stream.write_all(select_resp).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
                b'X' => break,
                _ => {
                    // Echo unknown messages.
                    let mut echo = Vec::with_capacity(1 + 4 + payload_len);
                    echo.push(type_buf[0]);
                    echo.extend_from_slice(&len_buf);
                    echo.extend_from_slice(&payload);
                    if stream.write_all(&echo).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
            }
        }
    }
}

// ── StatefulMockRedisServer ─────────────────────────────────────────

/// A mock Redis server that maintains key-value state for cache-aside
/// pattern testing. Supports GET, SET (with EX), DEL, and PING commands.
struct StatefulMockRedisServer {
    addr: std::net::SocketAddr,
}

impl StatefulMockRedisServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            // Shared state across all connections to the same server instance.
            let store: Arc<Mutex<HashMap<String, String>>> =
                Arc::new(Mutex::new(HashMap::new()));

            while let Ok((mut stream, _)) = listener.accept() {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    Self::handle_connection(&mut stream, &store);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    fn handle_connection(
        stream: &mut std::net::TcpStream,
        store: &Mutex<HashMap<String, String>>,
    ) {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let received = &buf[..n];
                    let response = Self::process_command(received, store);
                    if stream.write_all(&response).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
            }
        }
    }

    /// Parse and dispatch a Redis RESP command.
    fn process_command(data: &[u8], store: &Mutex<HashMap<String, String>>) -> Vec<u8> {
        let parts = Self::parse_resp_array(data);
        if parts.is_empty() {
            // Handle inline PING.
            if data == b"PING\r\n" {
                return b"+PONG\r\n".to_vec();
            }
            return b"-ERR unknown command\r\n".to_vec();
        }

        let cmd = parts[0].to_uppercase();
        match cmd.as_str() {
            "PING" => b"+PONG\r\n".to_vec(),
            "GET" => {
                if parts.len() < 2 {
                    return b"-ERR wrong number of arguments for 'get'\r\n".to_vec();
                }
                let key = &parts[1];
                let guard = store.lock().unwrap();
                match guard.get(key.as_str()) {
                    Some(val) => Self::bulk_string(val.as_bytes()),
                    None => b"$-1\r\n".to_vec(), // null bulk string
                }
            }
            "SET" => {
                if parts.len() < 3 {
                    return b"-ERR wrong number of arguments for 'set'\r\n".to_vec();
                }
                let key = parts[1].clone();
                let value = parts[2].clone();
                // We ignore EX/PX/NX/XX for simplicity — just store the value.
                store.lock().unwrap().insert(key, value);
                b"+OK\r\n".to_vec()
            }
            "DEL" => {
                if parts.len() < 2 {
                    return b"-ERR wrong number of arguments for 'del'\r\n".to_vec();
                }
                let mut count = 0u32;
                let mut guard = store.lock().unwrap();
                for key in &parts[1..] {
                    if guard.remove(key.as_str()).is_some() {
                        count += 1;
                    }
                }
                format!(":{count}\r\n").into_bytes()
            }
            _ => b"-ERR unknown command\r\n".to_vec(),
        }
    }

    /// Parse a RESP array into a vec of string parts.
    ///
    /// Format: `*<count>\r\n$<len>\r\n<data>\r\n...`
    fn parse_resp_array(data: &[u8]) -> Vec<String> {
        if data.is_empty() || data[0] != b'*' {
            return Vec::new();
        }

        let mut pos = 1;
        // Read array count.
        let count_end = Self::find_crlf(data, pos);
        if count_end.is_none() {
            return Vec::new();
        }
        let count_end = count_end.unwrap();
        let count_str = std::str::from_utf8(&data[pos..count_end]).unwrap_or("0");
        let count: usize = count_str.parse().unwrap_or(0);
        pos = count_end + 2; // skip \r\n

        let mut parts = Vec::with_capacity(count);
        for _ in 0..count {
            if pos >= data.len() || data[pos] != b'$' {
                break;
            }
            pos += 1; // skip $
            let len_end = Self::find_crlf(data, pos);
            if len_end.is_none() {
                break;
            }
            let len_end = len_end.unwrap();
            let len_str = std::str::from_utf8(&data[pos..len_end]).unwrap_or("0");
            let len: usize = len_str.parse().unwrap_or(0);
            pos = len_end + 2; // skip \r\n

            if pos + len > data.len() {
                break;
            }
            let value = std::str::from_utf8(&data[pos..pos + len])
                .unwrap_or("")
                .to_string();
            parts.push(value);
            pos += len + 2; // skip data + \r\n
        }

        parts
    }

    /// Build a RESP bulk string response.
    fn bulk_string(data: &[u8]) -> Vec<u8> {
        let mut resp = format!("${}\r\n", data.len()).into_bytes();
        resp.extend_from_slice(data);
        resp.extend_from_slice(b"\r\n");
        resp
    }

    /// Find position of \r\n starting from `start`.
    fn find_crlf(data: &[u8], start: usize) -> Option<usize> {
        for i in start..data.len().saturating_sub(1) {
            if data[i] == b'\r' && data[i + 1] == b'\n' {
                return Some(i);
            }
        }
        None
    }
}

// ── Build helpers ───────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

static COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_guest_component() -> &'static [u8] {
    COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/rust-http-redis-postgres-guest");

        let status = Command::new("cargo")
            .args([
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .current_dir(&guest_dir)
            .status()
            .expect("failed to run cargo build for guest fixture");
        assert!(
            status.success(),
            "guest fixture build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/rust_http_redis_postgres_guest.wasm");

        let component_path =
            guest_dir.join("target/rust-http-redis-postgres-guest.component.wasm");
        let status = Command::new("wasm-tools")
            .args([
                "component",
                "new",
                core_wasm_path.to_str().unwrap(),
                "-o",
                component_path.to_str().unwrap(),
            ])
            .status()
            .expect("failed to run wasm-tools component new");
        assert!(
            status.success(),
            "wasm-tools component new failed with exit code {:?}",
            status.code()
        );

        std::fs::read(&component_path).expect("failed to read compiled component")
    })
}

// ── Test host state builder ─────────────────────────────────────────

fn pool_config() -> PoolConfig {
    PoolConfig {
        max_size: 10,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(30),
        connect_timeout: Duration::from_millis(500),
        recv_timeout: Duration::from_secs(5),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(30),
    }
}

/// Build a ShimConfig with DNS entries for both "db.test.warp.local" and
/// "cache.test.warp.local" resolving to 127.0.0.1.
fn test_shim_config() -> ShimConfig {
    let mut service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    service_registry.insert(
        "db.test.warp.local".to_string(),
        vec!["127.0.0.1".parse().unwrap()],
    );
    service_registry.insert(
        "cache.test.warp.local".to_string(),
        vec!["127.0.0.1".parse().unwrap()],
    );

    ShimConfig {
        filesystem: true,
        dns: true,
        signals: true,
        database_proxy: true,
        threading: true,
        service_registry,
        pool_config: pool_config(),
        ..ShimConfig::default()
    }
}

// ── Helper: create engine + store + instance ────────────────────────

async fn fresh_instance(
    engine: &WarpGridEngine,
    component: &Component,
) -> (
    Store<warpgrid_host::engine::HostState>,
    wasmtime::component::Instance,
) {
    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, component)
        .await
        .unwrap();

    (store, instance)
}

// ── Integration tests ───────────────────────────────────────────────

/// Cold cache: Redis GET returns nil, Postgres queried, Redis SET called.
#[tokio::test(flavor = "multi_thread")]
async fn test_cold_cache_queries_postgres_and_caches() {
    let pg_server = QueryTrackingMockPostgres::start();
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16, u16), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-cold-cache",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (pg_server.addr.port(), redis_server.addr.port()),
        )
        .await
        .unwrap();

    let response = result.expect("cold cache test should succeed");

    // Verify Postgres was queried.
    assert_eq!(
        pg_server.query_count(),
        1,
        "cold cache should trigger exactly 1 Postgres query"
    );

    // Verify the response contains Postgres wire protocol data.
    assert!(
        !response.is_empty(),
        "Postgres response should not be empty"
    );

    // First message should be RowDescription ('T').
    assert_eq!(
        response[0], b'T',
        "first message should be RowDescription (T), got: 0x{:02x}",
        response[0]
    );

    // Response should contain seed user data.
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("alice"),
        "response should contain seed user 'alice'"
    );

    // Response should end with ReadyForQuery.
    let last_6 = &response[response.len() - 6..];
    assert_eq!(
        last_6, &READY_FOR_QUERY,
        "response should end with ReadyForQuery"
    );
}

/// Warm cache: Redis GET returns cached data, Postgres NOT queried.
#[tokio::test(flavor = "multi_thread")]
async fn test_warm_cache_skips_postgres() {
    let pg_server = QueryTrackingMockPostgres::start();
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16, u16), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-warm-cache",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (pg_server.addr.port(), redis_server.addr.port()),
        )
        .await
        .unwrap();

    let cached_value = result.expect("warm cache test should succeed");

    // Postgres should NOT have been queried — cache hit.
    // Note: query_count may be 0 because the guest does the Postgres
    // handshake but does NOT send a SimpleQuery message. The handshake
    // exchanges startup + auth messages which are NOT counted as queries
    // by our mock.
    assert_eq!(
        pg_server.query_count(),
        0,
        "warm cache should NOT trigger any Postgres query"
    );

    // Cached value should be the string we put in via SET.
    let cached_str = String::from_utf8_lossy(&cached_value);
    assert_eq!(
        cached_str, "cached_user_data_alice",
        "cached value should match what was SET"
    );
}

/// TTL expiry: after cache flush (DEL), Postgres is re-queried.
#[tokio::test(flavor = "multi_thread")]
async fn test_cache_flush_triggers_postgres_requery() {
    let pg_server = QueryTrackingMockPostgres::start();
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16, u16), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-cache-flush-requery",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (pg_server.addr.port(), redis_server.addr.port()),
        )
        .await
        .unwrap();

    let response = result.expect("cache flush requery test should succeed");

    // Postgres should have been re-queried after cache flush.
    assert_eq!(
        pg_server.query_count(),
        1,
        "after cache flush, Postgres should be re-queried exactly once"
    );

    // Response should contain valid Postgres wire protocol data.
    assert!(
        !response.is_empty(),
        "Postgres re-query response should not be empty"
    );
    assert_eq!(
        response[0], b'T',
        "first message should be RowDescription (T)"
    );

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("alice"),
        "re-query response should contain seed user 'alice'"
    );
}

/// Pool metrics: exactly 1 Postgres + 1 Redis connection active simultaneously.
#[tokio::test(flavor = "multi_thread")]
async fn test_pool_connections_one_pg_one_redis() {
    let pg_server = QueryTrackingMockPostgres::start();
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16, u16), (Result<String, String>,)>(
            &mut store,
            "test-pool-connections",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (pg_server.addr.port(), redis_server.addr.port()),
        )
        .await
        .unwrap();

    let metrics = result.expect("pool connections test should succeed");

    // Verify the formatted connection count string.
    assert_eq!(
        metrics, "pg_conns:1,redis_conns:1",
        "should report exactly 1 Postgres and 1 Redis connection"
    );

    // Also verify Postgres was queried (SELECT 1 sanity check).
    assert_eq!(
        pg_server.query_count(),
        1,
        "pool test should issue exactly 1 Postgres query"
    );
}

/// Full lifecycle: cold cache → warm cache → flush → re-query.
/// Exercises the complete cache-aside pattern across multiple instances.
#[tokio::test(flavor = "multi_thread")]
async fn test_full_cache_aside_lifecycle() {
    let pg_server = QueryTrackingMockPostgres::start();
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    // Step 1: Cold cache — Postgres queried, result cached in Redis.
    {
        let (mut store, instance) = fresh_instance(&engine, &component).await;
        let func = instance
            .get_typed_func::<(u16, u16), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-cold-cache",
            )
            .unwrap();
        let (result,) = func
            .call_async(
                &mut store,
                (pg_server.addr.port(), redis_server.addr.port()),
            )
            .await
            .unwrap();
        result.expect("cold cache step should succeed");
        assert_eq!(pg_server.query_count(), 1, "step 1: one PG query");
    }

    // Step 2: Warm cache — Postgres NOT queried (cache hit).
    {
        let (mut store, instance) = fresh_instance(&engine, &component).await;
        let func = instance
            .get_typed_func::<(u16, u16), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-warm-cache",
            )
            .unwrap();
        let (result,) = func
            .call_async(
                &mut store,
                (pg_server.addr.port(), redis_server.addr.port()),
            )
            .await
            .unwrap();
        result.expect("warm cache step should succeed");
        // Postgres query count should still be 1 from step 1.
        assert_eq!(pg_server.query_count(), 1, "step 2: still one PG query (cached)");
    }

    // Step 3: Cache flush + re-query — Postgres queried again.
    {
        let (mut store, instance) = fresh_instance(&engine, &component).await;
        let func = instance
            .get_typed_func::<(u16, u16), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-cache-flush-requery",
            )
            .unwrap();
        let (result,) = func
            .call_async(
                &mut store,
                (pg_server.addr.port(), redis_server.addr.port()),
            )
            .await
            .unwrap();
        result.expect("cache flush step should succeed");
        assert_eq!(pg_server.query_count(), 2, "step 3: two PG queries total");
    }
}
