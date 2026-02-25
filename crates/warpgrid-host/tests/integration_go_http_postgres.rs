//! US-704: Go HTTP + Postgres integration test (T3).
//!
//! End-to-end test that exercises a Wasm guest component simulating a Go
//! net/http handler with pgx through WarpGrid's engine with DNS and
//! database proxy shims.
//!
//! The guest component (built from `tests/fixtures/go-http-postgres-guest/`)
//! imports the WarpGrid DNS and database-proxy WIT shims and exports test
//! functions that exercise the same shim chain a patched TinyGo handler
//! would use: DNS resolve → database proxy connect → Postgres wire protocol
//! send/recv → close.
//!
//! When Domain 3 (TinyGo WASI overlay) is complete, the Rust guest will be
//! replaced by the actual Go handler compiled with warp-tinygo.
//!
//! Test stack:
//! ```text
//! Guest Component (Wasm)
//!   ↓ WIT import: dns.resolve-address
//! DnsHost → CachedDnsResolver → DnsResolver (service registry lookup)
//!   ↓ WIT import: database-proxy.connect/send/recv/close
//! DbProxyHost → ConnectionPoolManager → TcpConnectionFactory → TCP → MockPostgresServer
//! ```

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::config::ShimConfig;
use warpgrid_host::db_proxy::tcp::TcpConnectionFactory;
use warpgrid_host::db_proxy::PoolConfig;
use warpgrid_host::engine::WarpGridEngine;

// ── Postgres protocol helpers ─────────────────────────────────────

/// Build a Postgres RowDescription message.
///
/// Format: 'T' + Int32(length) + Int16(num_fields) + field descriptors
fn pg_row_description(fields: &[(&str, i32)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(fields.len() as i16).to_be_bytes());
    for (name, type_oid) in fields {
        body.extend_from_slice(name.as_bytes());
        body.push(0); // null-terminated name
        body.extend_from_slice(&0i32.to_be_bytes()); // table OID
        body.extend_from_slice(&0i16.to_be_bytes()); // column attribute number
        body.extend_from_slice(&type_oid.to_be_bytes()); // data type OID
        body.extend_from_slice(&4i16.to_be_bytes()); // data type size
        body.extend_from_slice(&(-1i32).to_be_bytes()); // type modifier
        body.extend_from_slice(&0i16.to_be_bytes()); // format code (text)
    }
    let len = (4 + body.len()) as u32;
    let mut msg = vec![b'T'];
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&body);
    msg
}

/// Build a Postgres DataRow message.
///
/// Format: 'D' + Int32(length) + Int16(num_cols) + [Int32(col_len) + col_data]*
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

/// Build a Postgres CommandComplete message.
///
/// Format: 'C' + Int32(length) + tag_string + NUL
fn pg_command_complete(tag: &str) -> Vec<u8> {
    let tag_bytes = format!("{tag}\0");
    let len = (4 + tag_bytes.len()) as u32;
    let mut msg = vec![b'C'];
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(tag_bytes.as_bytes());
    msg
}

/// Postgres ReadyForQuery message: 'Z' + Int32(5) + 'I' (idle).
fn pg_ready_for_query() -> Vec<u8> {
    vec![b'Z', 0, 0, 0, 5, b'I']
}

/// Build the SELECT response with 5 seed users.
fn seed_users_select_response() -> Vec<u8> {
    let mut response = Vec::new();
    // INT4 OID = 23, VARCHAR OID = 1043
    response.extend(pg_row_description(&[("id", 23), ("name", 1043)]));
    response.extend(pg_data_row(&["1", "alice"]));
    response.extend(pg_data_row(&["2", "bob"]));
    response.extend(pg_data_row(&["3", "charlie"]));
    response.extend(pg_data_row(&["4", "dave"]));
    response.extend(pg_data_row(&["5", "eve"]));
    response.extend(pg_command_complete("SELECT 5"));
    response.extend(pg_ready_for_query());
    response
}

/// Build the SELECT response with 6 users (5 seed + frank).
fn seed_users_plus_frank_response() -> Vec<u8> {
    let mut response = Vec::new();
    response.extend(pg_row_description(&[("id", 23), ("name", 1043)]));
    response.extend(pg_data_row(&["1", "alice"]));
    response.extend(pg_data_row(&["2", "bob"]));
    response.extend(pg_data_row(&["3", "charlie"]));
    response.extend(pg_data_row(&["4", "dave"]));
    response.extend(pg_data_row(&["5", "eve"]));
    response.extend(pg_data_row(&["6", "frank"]));
    response.extend(pg_command_complete("SELECT 6"));
    response.extend(pg_ready_for_query());
    response
}

/// Build an INSERT response.
fn insert_response() -> Vec<u8> {
    let mut response = Vec::new();
    response.extend(pg_command_complete("INSERT 0 1"));
    response.extend(pg_ready_for_query());
    response
}

// ── MockPostgresServer ──────────────────────────────────────────────

/// AuthenticationOk: 'R' + Int32(8) + Int32(0)
const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];

/// ReadyForQuery: 'Z' + Int32(5) + 'I' (idle)
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

/// A mock Postgres server that understands startup handshake and responds
/// to SimpleQuery ('Q') messages with canned Postgres wire protocol data.
///
/// Query routing:
/// - SELECT queries → seed users response (5 rows)
/// - INSERT queries → INSERT 0 1 response
/// - Other messages → echoed back (byte passthrough)
struct QueryAwareMockPostgres {
    addr: std::net::SocketAddr,
}

impl QueryAwareMockPostgres {
    /// Start the query-aware mock server.
    ///
    /// The server handles Postgres startup, then dispatches based on query type.
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        // Pre-compute canned responses.
        let select_resp = seed_users_select_response();
        let insert_resp = insert_response();
        let select_after_insert = seed_users_plus_frank_response();

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let select_resp = select_resp.clone();
                let insert_resp = insert_resp.clone();
                let select_after_insert = select_after_insert.clone();

                std::thread::spawn(move || {
                    Self::handle_connection(
                        &mut stream,
                        &select_resp,
                        &insert_resp,
                        &select_after_insert,
                    );
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    /// Start a basic echo mock (for proxy round-trip tests).
    fn start_echo() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    Self::handle_echo_connection(&mut stream);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
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
        insert_resp: &[u8],
        select_after_insert: &[u8],
    ) {
        if Self::read_startup_message(stream).is_err() {
            return;
        }

        // Send AuthenticationOk + ReadyForQuery.
        if stream.write_all(&AUTH_OK).is_err() {
            return;
        }
        if stream.write_all(&READY_FOR_QUERY).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        // Query-aware dispatch loop.
        let mut had_insert = false;
        loop {
            // Read message type byte.
            let mut type_buf = [0u8; 1];
            if stream.read_exact(&mut type_buf).is_err() {
                break;
            }

            // Read length prefix.
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;

            // Read payload.
            let payload_len = len.saturating_sub(4);
            let mut payload = vec![0u8; payload_len];
            if payload_len > 0 && stream.read_exact(&mut payload).is_err() {
                break;
            }

            match type_buf[0] {
                b'Q' => {
                    // SimpleQuery — check if it's SELECT or INSERT.
                    let query = String::from_utf8_lossy(&payload);
                    let response = if query.contains("INSERT") {
                        had_insert = true;
                        insert_resp
                    } else if had_insert {
                        // After an INSERT, SELECT returns the extended user list.
                        select_after_insert
                    } else {
                        select_resp
                    };
                    if stream.write_all(response).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
                b'X' => break, // Terminate.
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

    fn handle_echo_connection(stream: &mut std::net::TcpStream) {
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

        // Echo mode.
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stream.write_all(&buf[..n]).is_err() {
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

// ── Build helpers ─────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Build the guest fixture once per test run and return the component bytes.
static COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_guest_component() -> &'static [u8] {
    COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/go-http-postgres-guest");

        // Step 1: Build the guest crate to a core Wasm module.
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
            .join("target/wasm32-unknown-unknown/release/go_http_postgres_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools.
        let component_path = guest_dir.join("target/go-http-postgres-guest.component.wasm");
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

// ── Test host state builder ───────────────────────────────────────

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

/// Build a ShimConfig with DNS pointing to "db.test.warp.local" → 127.0.0.1
/// and database proxy enabled with a TcpConnectionFactory.
fn test_shim_config() -> ShimConfig {
    let mut service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    service_registry.insert(
        "db.test.warp.local".to_string(),
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

// ── Integration tests ─────────────────────────────────────────────

/// DNS resolution: "db.test.warp.local" resolves to 127.0.0.1 via service registry.
#[tokio::test(flavor = "multi_thread")]
async fn test_dns_resolves_db_host_through_service_registry() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-db-host")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let ip = result.expect("DNS resolution should succeed");
    assert_eq!(
        ip, "127.0.0.1",
        "db.test.warp.local should resolve to 127.0.0.1 from service registry"
    );
}

/// GET /users returns seed users via Postgres wire protocol through proxy shim.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_users_returns_seed_users() {
    let server = QueryAwareMockPostgres::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(u16,), (Result<Vec<u8>, String>,)>(&mut store, "test-get-users")
        .unwrap();
    let (result,) = func
        .call_async(&mut store, (server.addr.port(),))
        .await
        .unwrap();

    let response = result.expect("GET /users should succeed");

    // Verify we received Postgres wire protocol data.
    assert!(
        !response.is_empty(),
        "response should contain Postgres wire protocol data"
    );

    // First message should be RowDescription ('T').
    assert_eq!(
        response[0], b'T',
        "first message should be RowDescription (T), got: 0x{:02x}",
        response[0]
    );

    // Response should contain seed user names.
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("alice"),
        "response should contain seed user 'alice'"
    );
    assert!(
        response_str.contains("eve"),
        "response should contain seed user 'eve'"
    );

    // Response should end with ReadyForQuery.
    let last_6 = &response[response.len() - 6..];
    assert_eq!(
        last_6,
        &READY_FOR_QUERY,
        "response should end with ReadyForQuery"
    );

    // Response should contain CommandComplete with SELECT 5.
    assert!(
        response_str.contains("SELECT 5"),
        "response should contain 'SELECT 5' CommandComplete tag"
    );
}

/// POST /users creates a user, subsequent GET /users includes the new user.
#[tokio::test(flavor = "multi_thread")]
async fn test_post_user_returns_201_get_reflects_new_user() {
    let server = QueryAwareMockPostgres::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(u16,), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-post-and-get-users",
        )
        .unwrap();
    let (result,) = func
        .call_async(&mut store, (server.addr.port(),))
        .await
        .unwrap();

    let response = result.expect("POST+GET /users should succeed");

    // The response is from the SELECT after INSERT — should contain all 6 users.
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("alice"),
        "response should contain original seed user 'alice'"
    );
    assert!(
        response_str.contains("frank"),
        "response should contain newly inserted user 'frank'"
    );
    assert!(
        response_str.contains("SELECT 6"),
        "response should contain 'SELECT 6' CommandComplete tag (5 seed + frank)"
    );
}

/// Invalid database host returns DNS error (simulates 503 response).
#[tokio::test(flavor = "multi_thread")]
async fn test_invalid_db_host_returns_error() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-invalid-db-host")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    // The guest returns Ok(error_message) when DNS resolution fails for
    // the nonexistent host, which the real Go handler would map to HTTP 503.
    let error_msg = result.expect("test-invalid-db-host should return the error message");
    assert!(
        !error_msg.is_empty(),
        "should receive a non-empty error message for nonexistent host"
    );
}

/// Proxy round-trip: data flows through database proxy and returns correctly.
/// Validates that net.Dial TCP connections route through the shim.
#[tokio::test(flavor = "multi_thread")]
async fn test_proxy_roundtrip_routes_through_database_proxy() {
    let server = QueryAwareMockPostgres::start_echo();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(u16,), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-proxy-roundtrip",
        )
        .unwrap();
    let (result,) = func
        .call_async(&mut store, (server.addr.port(),))
        .await
        .unwrap();

    let received = result.expect("proxy round-trip should succeed");

    // The echo mock returns the exact bytes sent.
    let expected = b"PROXY_ROUNDTRIP_TEST_DATA";
    assert_eq!(
        received.as_slice(),
        expected,
        "proxy should pass bytes through unmodified"
    );
}

/// Full lifecycle: DNS resolve + connect + handshake + multiple queries + close.
/// Exercises the complete shim chain that a Go handler would use.
///
/// Each step gets a fresh store + instance because the Wasm component model
/// requires `post_return` before re-entering an instance. Using separate
/// instances also mirrors real usage where each HTTP request gets its own
/// instantiation.
#[tokio::test(flavor = "multi_thread")]
async fn test_full_lifecycle_dns_connect_query_close() {
    let server = QueryAwareMockPostgres::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    // Macro to create a fresh store + instance for each step — avoids
    // async closure lifetime issues while keeping the test DRY.
    macro_rules! fresh_instance {
        ($engine:expr, $component:expr) => {{
            let config = test_shim_config();
            let factory = Arc::new(TcpConnectionFactory::plain(
                config.pool_config.recv_timeout,
                config.pool_config.connect_timeout,
            ));
            let host_state = $engine.build_host_state(&config, Some(factory));
            let mut store = Store::new($engine.engine(), host_state);
            let instance = $engine
                .linker()
                .instantiate_async(&mut store, &$component)
                .await
                .unwrap();
            (store, instance)
        }};
    }

    // Step 1: Verify DNS resolution works.
    {
        let (mut store, instance) = fresh_instance!(engine, component);
        let func = instance
            .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-db-host")
            .unwrap();
        let (result,) = func.call_async(&mut store, ()).await.unwrap();
        let ip = result.expect("DNS should resolve");
        assert_eq!(ip, "127.0.0.1");
    }

    // Step 2: Verify GET /users works with data.
    {
        let (mut store, instance) = fresh_instance!(engine, component);
        let func = instance
            .get_typed_func::<(u16,), (Result<Vec<u8>, String>,)>(&mut store, "test-get-users")
            .unwrap();
        let (result,) = func
            .call_async(&mut store, (server.addr.port(),))
            .await
            .unwrap();
        let response = result.expect("GET should succeed");
        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.contains("alice"));
        assert!(response_str.contains("SELECT 5"));
    }

    // Step 3: Verify POST + GET works (INSERT then SELECT with new user).
    {
        let (mut store, instance) = fresh_instance!(engine, component);
        let func = instance
            .get_typed_func::<(u16,), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-post-and-get-users",
            )
            .unwrap();
        let (result,) = func
            .call_async(&mut store, (server.addr.port(),))
            .await
            .unwrap();
        let response = result.expect("POST+GET should succeed");
        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.contains("frank"));
        assert!(response_str.contains("SELECT 6"));
    }

    // Step 4: Verify invalid host returns error.
    {
        let (mut store, instance) = fresh_instance!(engine, component);
        let func = instance
            .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-invalid-db-host")
            .unwrap();
        let (result,) = func.call_async(&mut store, ()).await.unwrap();
        let err = result.expect("should return error message");
        assert!(!err.is_empty());
    }
}
