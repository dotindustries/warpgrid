//! US-705: T4 TypeScript HTTP + Postgres integration test.
//!
//! These tests validate the WarpGrid database proxy shim path that the
//! TypeScript handler would use. A Rust guest component simulates the
//! same operations as `src/handler.js`:
//!
//! 1. Connect to Postgres through the database proxy
//! 2. Perform the startup handshake
//! 3. Execute SELECT queries
//! 4. Close the connection
//!
//! The mock Postgres server returns realistic DataRow messages so the
//! guest can parse query results.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::db_proxy::host::DbProxyHost;
use warpgrid_host::db_proxy::tcp::TcpConnectionFactory;
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig};
use warpgrid_host::engine::{HostState, WarpGridEngine};

// ── Postgres protocol constants ─────────────────────────────────────

/// AuthenticationOk: 'R' + Int32(8) + Int32(0)
const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];

/// ReadyForQuery: 'Z' + Int32(5) + 'I' (idle)
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

/// CommandComplete: 'C' + len + tag
fn command_complete(tag: &str) -> Vec<u8> {
    let tag_bytes = tag.as_bytes();
    let len = (4 + tag_bytes.len() + 1) as i32;
    let mut buf = Vec::with_capacity(1 + len as usize);
    buf.push(b'C');
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(tag_bytes);
    buf.push(0);
    buf
}

/// Build a RowDescription message for (id, name, email) columns.
fn row_description_users() -> Vec<u8> {
    let columns = [
        ("id", 23_i32, 4_i16),       // int4, 4 bytes
        ("name", 25_i32, -1_i16),     // text, variable
        ("email", 25_i32, -1_i16),    // text, variable
    ];

    let mut fields = Vec::new();
    for (name, type_oid, type_len) in &columns {
        fields.extend_from_slice(name.as_bytes());
        fields.push(0); // null terminator
        fields.extend_from_slice(&0_i32.to_be_bytes()); // table OID
        fields.extend_from_slice(&0_i16.to_be_bytes()); // column attr number
        fields.extend_from_slice(&type_oid.to_be_bytes()); // type OID
        fields.extend_from_slice(&type_len.to_be_bytes()); // type length
        fields.extend_from_slice(&(-1_i32).to_be_bytes()); // type modifier
        fields.extend_from_slice(&0_i16.to_be_bytes()); // format code (text)
    }

    let field_count = columns.len() as i16;
    let len = (4 + 2 + fields.len()) as i32;
    let mut buf = Vec::with_capacity(1 + len as usize);
    buf.push(b'T'); // RowDescription
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&field_count.to_be_bytes());
    buf.extend_from_slice(&fields);
    buf
}

/// Build a DataRow message from text field values.
fn data_row(fields: &[&str]) -> Vec<u8> {
    let field_count = fields.len() as i16;
    let mut field_data = Vec::new();
    for field in fields {
        let bytes = field.as_bytes();
        field_data.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
        field_data.extend_from_slice(bytes);
    }

    let len = (4 + 2 + field_data.len()) as i32;
    let mut buf = Vec::with_capacity(1 + len as usize);
    buf.push(b'D'); // DataRow
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&field_count.to_be_bytes());
    buf.extend_from_slice(&field_data);
    buf
}

/// Seed users matching test-infra/seed.sql (5 rows).
const SEED_USERS: [(&str, &str, &str); 5] = [
    ("1", "Alice Johnson", "alice@example.com"),
    ("2", "Bob Smith", "bob@example.com"),
    ("3", "Carol Williams", "carol@example.com"),
    ("4", "Dave Brown", "dave@example.com"),
    ("5", "Eve Davis", "eve@example.com"),
];

// ── MockPostgresServer ──────────────────────────────────────────────

/// A mock Postgres server that handles startup handshake and responds
/// to simple queries with canned test_users data.
struct MockPostgresServer {
    addr: std::net::SocketAddr,
}

impl MockPostgresServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    Self::handle_connection(&mut stream);
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

    fn handle_connection(stream: &mut std::net::TcpStream) {
        if Self::read_startup_message(stream).is_err() {
            return;
        }

        // Send AuthenticationOk + ReadyForQuery
        if stream.write_all(&AUTH_OK).is_err() {
            return;
        }
        if stream.write_all(&READY_FOR_QUERY).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        // Query handling loop
        let mut buf = [0u8; 4096];
        loop {
            // Read message type byte
            let n = match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };

            if buf[0] == b'Q' {
                // Simple query — extract SQL from the message
                // Format: 'Q' + Int32(len) + sql + '\0'
                let sql_end = buf[5..n].iter().position(|&b| b == 0).unwrap_or(n - 5);
                let sql = std::str::from_utf8(&buf[5..5 + sql_end]).unwrap_or("");

                let response = Self::handle_query(sql);
                if stream.write_all(&response).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            } else if buf[0] == b'X' {
                // Terminate
                break;
            }
        }
    }

    fn handle_query(sql: &str) -> Vec<u8> {
        let sql_lower = sql.to_lowercase();
        let mut response = Vec::new();

        if sql_lower.contains("select") && sql_lower.contains("test_users") {
            // Return seed users
            response.extend_from_slice(&row_description_users());
            for (id, name, email) in &SEED_USERS {
                response.extend_from_slice(&data_row(&[id, name, email]));
            }
            response.extend_from_slice(&command_complete("SELECT 5"));
        } else if sql_lower.starts_with("insert") {
            // Return a single inserted row
            response.extend_from_slice(&row_description_users());
            response.extend_from_slice(&data_row(&["6", "Test User", "test@example.com"]));
            response.extend_from_slice(&command_complete("INSERT 0 1"));
        } else {
            // Unknown query — return empty result
            response.extend_from_slice(&command_complete("SELECT 0"));
        }

        response.extend_from_slice(&READY_FOR_QUERY);
        response
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

/// Build the T4 DB proxy guest fixture once per test run.
static COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_guest_component() -> &'static [u8] {
    COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/t4-db-proxy-guest");

        // Step 1: Build the guest crate to a core Wasm module
        let status = Command::new("cargo")
            .args([
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .current_dir(&guest_dir)
            .status()
            .expect("failed to run cargo build for t4-db-proxy-guest");
        assert!(
            status.success(),
            "t4-db-proxy-guest build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/t4_db_proxy_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/t4-db-proxy-guest.component.wasm");
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

fn test_pool_config() -> PoolConfig {
    PoolConfig {
        max_size: 10,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(30),
        connect_timeout: Duration::from_millis(2000),
        recv_timeout: Duration::from_secs(5),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(5),
    }
}

fn test_host_state(pool_manager: Arc<ConnectionPoolManager>) -> HostState {
    let runtime_handle = tokio::runtime::Handle::current();
    HostState {
        filesystem: None,
        dns: None,
        db_proxy: Some(DbProxyHost::new(pool_manager, runtime_handle)),
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

// ── Integration Tests ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_t4_db_connect_and_handshake() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let factory = Arc::new(TcpConnectionFactory::plain(
        Duration::from_secs(5),
        Duration::from_millis(2000),
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(test_pool_config(), factory));
    let host_state = test_host_state(pool_manager);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Call test-db-connect: connect to mock Postgres, perform handshake
    let func = instance
        .get_typed_func::<(String, u16, String, String), (Result<String, String>,)>(
            &mut store,
            "test-db-connect",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                mock_pg.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();

    let handle_str = result.expect("connect should succeed");
    let handle: u64 = handle_str.parse().expect("handle should be a number");
    assert!(handle > 0, "handle should be positive, got {handle}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_t4_full_lifecycle_select_users() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let factory = Arc::new(TcpConnectionFactory::plain(
        Duration::from_secs(5),
        Duration::from_millis(2000),
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(test_pool_config(), factory));
    let host_state = test_host_state(pool_manager);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Call test-full-lifecycle: connect → handshake → SELECT test_users → close
    let func = instance
        .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-full-lifecycle",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                mock_pg.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();

    let response_bytes = result.expect("full lifecycle should succeed");

    // Verify response contains DataRow messages with seed user data.
    // The mock server sends RowDescription + 5 DataRows + CommandComplete + ReadyForQuery.
    assert!(
        !response_bytes.is_empty(),
        "query response should not be empty"
    );

    // Check for RowDescription marker ('T')
    assert_eq!(
        response_bytes[0], b'T',
        "first message should be RowDescription ('T'), got {:?}",
        response_bytes[0] as char
    );

    // Check that response contains seed user names (as byte substrings)
    let response_str = String::from_utf8_lossy(&response_bytes);
    assert!(
        response_str.contains("Alice Johnson"),
        "response should contain 'Alice Johnson'"
    );
    assert!(
        response_str.contains("Eve Davis"),
        "response should contain 'Eve Davis'"
    );
    assert!(
        response_str.contains("alice@example.com"),
        "response should contain 'alice@example.com'"
    );

    // Check for ReadyForQuery at the end
    let len = response_bytes.len();
    assert!(len >= 6, "response too short for ReadyForQuery");
    assert_eq!(
        response_bytes[len - 6],
        b'Z',
        "response should end with ReadyForQuery ('Z')"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_t4_query_then_close() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let factory = Arc::new(TcpConnectionFactory::plain(
        Duration::from_secs(5),
        Duration::from_millis(2000),
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(test_pool_config(), factory));
    let host_state = test_host_state(pool_manager);
    let mut store = Store::new(engine.engine(), host_state);

    // The Wasm Component Model's may_enter flag prevents calling multiple
    // exports on the same instance without a full post-return cycle.
    // We re-instantiate per call but share the same store (HostState),
    // so connection handles and pool state persist across calls.

    // Step 1: Connect
    let inst1 = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let connect_func = inst1
        .get_typed_func::<(String, u16, String, String), (Result<String, String>,)>(
            &mut store,
            "test-db-connect",
        )
        .unwrap();

    let (result,) = connect_func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                mock_pg.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();

    let handle_str = result.expect("connect should succeed");

    // Step 2: Query (fresh instance, same store → same handle table)
    let inst2 = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let query_func = inst2
        .get_typed_func::<(String, String), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-db-query",
        )
        .unwrap();

    let (query_result,) = query_func
        .call_async(
            &mut store,
            (
                handle_str.clone(),
                "SELECT id, name, email FROM test_users ORDER BY id".to_string(),
            ),
        )
        .await
        .unwrap();

    let response = query_result.expect("query should succeed");
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("Bob Smith"),
        "query response should contain 'Bob Smith'"
    );

    // Step 3: Close (fresh instance, same store)
    let inst3 = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let close_func = inst3
        .get_typed_func::<(String,), (Result<String, String>,)>(&mut store, "test-db-close")
        .unwrap();

    let (close_result,) = close_func
        .call_async(&mut store, (handle_str,))
        .await
        .unwrap();

    let msg = close_result.expect("close should succeed");
    assert_eq!(msg, "closed");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_t4_connection_returned_to_pool_on_close() {
    // Verifies that closing a connection via the database proxy shim
    // returns it to the pool (idle) rather than destroying it. This
    // proves the pool lifecycle works: connect → use → close → idle.
    //
    // Note: full pool *reuse* (connect → close → reconnect) requires
    // protocol-aware pooling (the server doesn't expect a second
    // handshake on a reused TCP socket). That belongs in a higher-level
    // Postgres-aware pool layer, not the raw TCP pool tested here.

    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let factory = Arc::new(TcpConnectionFactory::plain(
        Duration::from_secs(5),
        Duration::from_millis(2000),
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(test_pool_config(), factory));
    let pm_clone = pool_manager.clone();
    let host_state = test_host_state(pool_manager);
    let mut store = Store::new(engine.engine(), host_state);

    let key = warpgrid_host::db_proxy::PoolKey::new(
        "127.0.0.1",
        mock_pg.addr.port(),
        "testdb",
        "testuser",
    );

    // Before: pool empty
    let stats = pm_clone.stats(&key).await;
    assert_eq!(stats.total, 0, "pool should start empty");

    // Connect
    let inst1 = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let connect_func = inst1
        .get_typed_func::<(String, u16, String, String), (Result<String, String>,)>(
            &mut store,
            "test-db-connect",
        )
        .unwrap();
    let (result,) = connect_func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                mock_pg.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();
    let handle = result.expect("connect should succeed");

    // While connected: 1 active, 0 idle
    let stats = pm_clone.stats(&key).await;
    assert_eq!(stats.total, 1, "pool should have 1 connection while active");
    assert_eq!(stats.active, 1, "connection should be active");

    // Close — returns to pool as idle
    let inst2 = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let close_func = inst2
        .get_typed_func::<(String,), (Result<String, String>,)>(&mut store, "test-db-close")
        .unwrap();
    close_func
        .call_async(&mut store, (handle,))
        .await
        .unwrap()
        .0
        .expect("close should succeed");

    // After close: 1 total, 0 active, 1 idle — connection was pooled
    let stats = pm_clone.stats(&key).await;
    assert_eq!(
        stats.total, 1,
        "after close, pool should still have 1 total connection"
    );
    assert_eq!(
        stats.idle, 1,
        "after close, connection should be idle (pooled, not destroyed)"
    );
    assert_eq!(
        stats.active, 0,
        "after close, no connections should be active"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_t4_db_proxy_not_raw_tcp() {
    // This test verifies acceptance criterion #5:
    // "warpgrid.database.connect() invoked (not raw TCP)"
    //
    // The guest component uses warpgrid:shim/database-proxy WIT imports
    // to connect. The pool manager tracks all connections. If the guest
    // had used raw TCP, the pool manager would have no record.

    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let factory = Arc::new(TcpConnectionFactory::plain(
        Duration::from_secs(5),
        Duration::from_millis(2000),
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(test_pool_config(), factory));
    let pm_clone = pool_manager.clone();
    let host_state = test_host_state(pool_manager);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let key = warpgrid_host::db_proxy::PoolKey::new(
        "127.0.0.1",
        mock_pg.addr.port(),
        "testdb",
        "testuser",
    );

    // Before connect: pool should have no connections
    let stats_before = pm_clone.stats(&key).await;
    assert_eq!(stats_before.total, 0, "pool should start empty");

    // Connect through shim
    let func = instance
        .get_typed_func::<(String, u16, String, String), (Result<String, String>,)>(
            &mut store,
            "test-db-connect",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                mock_pg.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();
    result.expect("connect should succeed");

    // After connect: pool should have 1 active connection
    // This PROVES the connection went through the database proxy shim,
    // not raw TCP — if it were raw TCP, pool stats would still be 0.
    let stats_after = pm_clone.stats(&key).await;
    assert_eq!(
        stats_after.total, 1,
        "pool should have 1 connection — proves DB proxy shim was used (not raw TCP)"
    );
    assert_eq!(
        stats_after.active, 1,
        "pool should have 1 active connection"
    );
}
