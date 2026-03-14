//! US-114: Database proxy Postgres integration test.
//!
//! End-to-end test that compiles a Rust guest component to Wasm and runs
//! Postgres queries (CREATE TABLE, INSERT, SELECT, DROP TABLE) through
//! WarpGrid's database proxy shim with a stateful mock Postgres server.
//!
//! The guest component (built from `tests/fixtures/rust-sqlx-postgres-guest/`)
//! imports the WarpGrid database-proxy WIT shim and exports test functions
//! that exercise the full DDL lifecycle, connection reuse, and reconnect.
//!
//! Test stack:
//! ```text
//! Guest Component (Wasm)
//!   ↓ WIT import: database-proxy.connect/send/recv/close
//! DbProxyHost → ConnectionPoolManager → TcpConnectionFactory → TCP → StatefulMockPostgresServer
//! ```

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
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

// ── Postgres protocol constants ─────────────────────────────────────

/// AuthenticationOk: 'R' + Int32(8) + Int32(0)
const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];

/// ReadyForQuery: 'Z' + Int32(5) + 'I' (idle)
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

// ── StatefulMockPostgresServer ──────────────────────────────────────

/// A row in the mock database: (id, name, email).
#[derive(Clone)]
struct MockRow {
    id: i32,
    name: String,
    email: String,
}

/// Per-connection state for the stateful mock Postgres server.
struct ConnectionState {
    /// Tables: table_name → rows
    tables: HashMap<String, Vec<MockRow>>,
    /// Auto-increment counter per table
    next_id: HashMap<String, i32>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            tables: HashMap::new(),
            next_id: HashMap::new(),
        }
    }
}

/// A mock Postgres server that maintains per-connection table state.
///
/// Understands Postgres v3.0 startup handshake and routes SimpleQuery
/// messages based on SQL content to produce realistic wire protocol
/// responses.
struct StatefulMockPostgresServer {
    addr: std::net::SocketAddr,
}

impl StatefulMockPostgresServer {
    /// Start the stateful mock server on a random port.
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

        let mut state = ConnectionState::new();

        // Query dispatch loop.
        loop {
            // Read message type byte.
            let mut type_buf = [0u8; 1];
            if stream.read_exact(&mut type_buf).is_err() {
                break;
            }

            // Handle re-handshake: when a pooled connection is reused, the guest
            // sends a new startup message. The startup message begins with a
            // 4-byte length (MSB is 0x00 for reasonable lengths), not a message
            // type byte. Detect this before reading the standard len/payload.
            if type_buf[0] == 0x00 {
                // We've read the first byte of the startup length field.
                // Read the remaining 3 bytes of the length.
                let mut len_rest = [0u8; 3];
                if stream.read_exact(&mut len_rest).is_err() {
                    break;
                }
                let startup_len =
                    u32::from_be_bytes([0x00, len_rest[0], len_rest[1], len_rest[2]]) as usize;
                // Read the rest of the startup message (startup_len - 4 bytes for length field).
                let remaining = startup_len.saturating_sub(4);
                let mut discard = vec![0u8; remaining];
                if remaining > 0 && stream.read_exact(&mut discard).is_err() {
                    break;
                }
                // Reset connection state for the new session.
                state = ConnectionState::new();
                // Re-send AuthOk + ReadyForQuery.
                if stream.write_all(&AUTH_OK).is_err() {
                    break;
                }
                if stream.write_all(&READY_FOR_QUERY).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
                continue;
            }

            // Read length prefix (standard Postgres message format).
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
                    // SimpleQuery — extract SQL (null-terminated).
                    let sql = String::from_utf8_lossy(&payload)
                        .trim_end_matches('\0')
                        .to_string();
                    let sql_upper = sql.to_uppercase();

                    let response = Self::dispatch_query(&sql, &sql_upper, &mut state);
                    if stream.write_all(&response).is_err() {
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

    /// Route a SQL query to the appropriate handler and produce a wire protocol response.
    fn dispatch_query(sql: &str, sql_upper: &str, state: &mut ConnectionState) -> Vec<u8> {
        if sql_upper.starts_with("CREATE TABLE") {
            Self::handle_create_table(sql, state)
        } else if sql_upper.starts_with("INSERT INTO") {
            Self::handle_insert(sql, state)
        } else if sql_upper.starts_with("DROP TABLE") {
            Self::handle_drop_table(sql_upper, state)
        } else if sql_upper.starts_with("SELECT") && sql_upper.contains("FROM") {
            Self::handle_select_from(sql_upper, state)
        } else if sql_upper.starts_with("SELECT") {
            Self::handle_select_expr(sql)
        } else {
            // Unknown query — return CommandComplete with generic tag.
            let mut resp = Vec::new();
            resp.extend(pg_command_complete("OK"));
            resp.extend(pg_ready_for_query());
            resp
        }
    }

    /// Handle CREATE TABLE — register a new empty table in state.
    fn handle_create_table(sql: &str, state: &mut ConnectionState) -> Vec<u8> {
        // Extract table name: "CREATE TABLE <name> (...)"
        let table_name = Self::extract_table_name_after(sql, "CREATE TABLE");
        state.tables.insert(table_name.clone(), Vec::new());
        state.next_id.insert(table_name, 1);

        let mut resp = Vec::new();
        resp.extend(pg_command_complete("CREATE TABLE"));
        resp.extend(pg_ready_for_query());
        resp
    }

    /// Handle INSERT INTO — add a row to the named table.
    fn handle_insert(sql: &str, state: &mut ConnectionState) -> Vec<u8> {
        // Extract table name: "INSERT INTO <name> ..."
        let table_name = Self::extract_table_name_after(sql, "INSERT INTO");

        // Extract VALUES from the SQL.
        let (name_val, email_val) = Self::extract_insert_values(sql);

        let id = state.next_id.get(&table_name).copied().unwrap_or(1);
        state.next_id.insert(table_name.clone(), id + 1);

        let row = MockRow {
            id,
            name: name_val,
            email: email_val,
        };
        state
            .tables
            .entry(table_name)
            .or_default()
            .push(row);

        let mut resp = Vec::new();
        resp.extend(pg_command_complete("INSERT 0 1"));
        resp.extend(pg_ready_for_query());
        resp
    }

    /// Handle SELECT ... FROM <table> — return RowDescription + DataRows + CommandComplete.
    fn handle_select_from(sql_upper: &str, state: &ConnectionState) -> Vec<u8> {
        // Extract table name after FROM
        let table_name = Self::extract_table_name_after(sql_upper, "FROM");

        let rows = state.tables.get(&table_name).cloned().unwrap_or_default();
        let row_count = rows.len();

        let mut resp = Vec::new();
        // INT4 OID = 23, TEXT OID = 25
        resp.extend(pg_row_description(&[("id", 23), ("name", 25), ("email", 25)]));

        for row in &rows {
            resp.extend(pg_data_row(&[
                &row.id.to_string(),
                &row.name,
                &row.email,
            ]));
        }

        resp.extend(pg_command_complete(&format!("SELECT {row_count}")));
        resp.extend(pg_ready_for_query());
        resp
    }

    /// Handle SELECT <expr> (no FROM clause) — return a single-column result.
    fn handle_select_expr(sql: &str) -> Vec<u8> {
        // Extract the expression after "SELECT "
        let expr = sql
            .strip_prefix("SELECT ")
            .or_else(|| sql.strip_prefix("select "))
            .unwrap_or("?")
            .trim()
            .trim_matches('\'');

        let mut resp = Vec::new();
        // VARCHAR OID = 1043
        resp.extend(pg_row_description(&[("?column?", 1043)]));
        resp.extend(pg_data_row(&[expr]));
        resp.extend(pg_command_complete("SELECT 1"));
        resp.extend(pg_ready_for_query());
        resp
    }

    /// Handle DROP TABLE — remove the table from state.
    fn handle_drop_table(sql_upper: &str, state: &mut ConnectionState) -> Vec<u8> {
        let table_name = Self::extract_table_name_after(sql_upper, "DROP TABLE");
        state.tables.remove(&table_name);
        state.next_id.remove(&table_name);

        let mut resp = Vec::new();
        resp.extend(pg_command_complete("DROP TABLE"));
        resp.extend(pg_ready_for_query());
        resp
    }

    /// Extract a table name that follows a keyword (e.g., "FROM", "CREATE TABLE", "INSERT INTO").
    fn extract_table_name_after(sql: &str, keyword: &str) -> String {
        let upper = sql.to_uppercase();
        let keyword_upper = keyword.to_uppercase();
        if let Some(idx) = upper.find(&keyword_upper) {
            let after = &sql[idx + keyword.len()..].trim_start();
            // Take the first word (stop at space, paren, semicolon, or end).
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            // Normalize to lowercase for consistent lookup.
            name.to_lowercase()
        } else {
            "unknown".to_string()
        }
    }

    /// Extract (name, email) values from an INSERT statement.
    ///
    /// Parses: `INSERT INTO ... VALUES ('name_val', 'email_val')`
    fn extract_insert_values(sql: &str) -> (String, String) {
        let upper = sql.to_uppercase();
        if let Some(vals_idx) = upper.find("VALUES") {
            let after_values = &sql[vals_idx + 6..]; // skip "VALUES"
            // Find the content inside parentheses.
            if let (Some(open), Some(close)) = (after_values.find('('), after_values.find(')')) {
                let inner = &after_values[open + 1..close];
                let parts: Vec<&str> = inner.split(',').collect();
                let name = parts
                    .first()
                    .map(|s| s.trim().trim_matches('\'').to_string())
                    .unwrap_or_default();
                let email = parts
                    .get(1)
                    .map(|s| s.trim().trim_matches('\'').to_string())
                    .unwrap_or_default();
                return (name, email);
            }
        }
        ("unknown".to_string(), "unknown@test.com".to_string())
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
        let guest_dir = root.join("tests/fixtures/rust-sqlx-postgres-guest");

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
            .join("target/wasm32-unknown-unknown/release/rust_sqlx_postgres_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools.
        let component_path = guest_dir.join("target/rust-sqlx-postgres-guest.component.wasm");
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

fn test_shim_config() -> ShimConfig {
    ShimConfig {
        filesystem: true,
        dns: true,
        signals: true,
        database_proxy: true,
        threading: true,
        pool_config: pool_config(),
        ..ShimConfig::default()
    }
}

// ── Helper: create engine + store + instance ─────────────────────

/// Create a fresh store and instance from the shared engine and component.
/// Each test function call needs its own store+instance because the Wasm
/// component model requires `post_return` before re-entering.
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
    let host_state = engine.build_host_state(Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, component)
        .await
        .unwrap();

    (store, instance)
}

// ── Integration tests ─────────────────────────────────────────────

/// Full DDL lifecycle: CREATE TABLE, INSERT rows, SELECT with verification, DROP TABLE.
///
/// Validates that the guest component can execute a complete Postgres DDL lifecycle
/// through the database proxy shim, and that the mock server produces realistic
/// wire protocol responses including RowDescription, DataRow, and CommandComplete.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn test_create_insert_select_drop() {
    let server = StatefulMockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let config = test_shim_config();
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-create-insert-select-drop",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                server.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();

    let response = result.expect("DDL lifecycle should succeed");

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

    // Response should contain the inserted user names.
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("alice"),
        "response should contain inserted user 'alice'"
    );
    assert!(
        response_str.contains("bob"),
        "response should contain inserted user 'bob'"
    );

    // Response should contain CommandComplete with "SELECT 2" (two rows inserted).
    assert!(
        response_str.contains("SELECT 2"),
        "response should contain 'SELECT 2' CommandComplete tag"
    );

    // Response should end with ReadyForQuery.
    let last_6 = &response[response.len() - 6..];
    assert_eq!(
        last_6, &READY_FOR_QUERY,
        "response should end with ReadyForQuery"
    );
}

/// Multiple queries on the same connection handle verify connection reuse.
///
/// The guest executes SELECT 1, SELECT 2, SELECT 3 on a single handle.
/// Each query should produce a valid response with ReadyForQuery markers.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn test_connection_reuse_multiple_queries() {
    let server = StatefulMockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let config = test_shim_config();
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-connection-reuse",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                server.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();

    let response = result.expect("connection reuse should succeed");

    assert!(
        !response.is_empty(),
        "response should contain data from multiple queries"
    );

    // Count ReadyForQuery markers — should be 3 (one per query).
    let rfq_count = count_ready_for_query(&response);
    assert_eq!(
        rfq_count, 3,
        "should have 3 ReadyForQuery markers (one per query), got {rfq_count}"
    );

    // Response should contain the query results.
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("1"),
        "response should contain result of SELECT 1"
    );
    assert!(
        response_str.contains("3"),
        "response should contain result of SELECT 3"
    );
}

/// Close and reconnect validates that the pool manager reuses connections.
///
/// The guest connects, queries, closes, then reconnects and queries again.
/// Both query responses should contain valid Postgres wire protocol data,
/// confirming that close returns the connection to the pool and reconnect
/// reuses it.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn test_close_reconnect_reuses_pooled_connection() {
    let server = StatefulMockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let config = test_shim_config();
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
            &mut store,
            "test-reconnect-after-close",
        )
        .unwrap();

    let (result,) = func
        .call_async(
            &mut store,
            (
                "127.0.0.1".to_string(),
                server.addr.port(),
                "testdb".to_string(),
                "testuser".to_string(),
            ),
        )
        .await
        .unwrap();

    let response = result.expect("reconnect after close should succeed");

    assert!(
        !response.is_empty(),
        "response should contain data from both connection cycles"
    );

    // Should have 2 ReadyForQuery markers (one per query in each cycle).
    let rfq_count = count_ready_for_query(&response);
    assert_eq!(
        rfq_count, 2,
        "should have 2 ReadyForQuery markers (one per connection cycle), got {rfq_count}"
    );

    // Response should contain data from both queries.
    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("first_connection"),
        "response should contain first connection query result"
    );
    assert!(
        response_str.contains("second_connection"),
        "response should contain second connection query result"
    );
}

/// Full lifecycle: DDL lifecycle + connection reuse + reconnect in sequence.
///
/// Orchestrates all three test scenarios on the same engine/component to
/// validate the complete flow end-to-end.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn test_full_lifecycle_end_to_end() {
    let server = StatefulMockPostgresServer::start();
    let wasm_bytes = build_guest_component();
    let config = test_shim_config();
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    // Step 1: DDL lifecycle (CREATE, INSERT, SELECT, DROP).
    {
        let (mut store, instance) = fresh_instance(&engine, &component).await;
        let func = instance
            .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-create-insert-select-drop",
            )
            .unwrap();
        let (result,) = func
            .call_async(
                &mut store,
                (
                    "127.0.0.1".to_string(),
                    server.addr.port(),
                    "testdb".to_string(),
                    "testuser".to_string(),
                ),
            )
            .await
            .unwrap();
        let response = result.expect("DDL lifecycle should succeed");
        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.contains("alice"));
        assert!(response_str.contains("SELECT 2"));
    }

    // Step 2: Connection reuse (multiple queries on same handle).
    {
        let (mut store, instance) = fresh_instance(&engine, &component).await;
        let func = instance
            .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-connection-reuse",
            )
            .unwrap();
        let (result,) = func
            .call_async(
                &mut store,
                (
                    "127.0.0.1".to_string(),
                    server.addr.port(),
                    "testdb".to_string(),
                    "testuser".to_string(),
                ),
            )
            .await
            .unwrap();
        let response = result.expect("connection reuse should succeed");
        assert_eq!(count_ready_for_query(&response), 3);
    }

    // Step 3: Reconnect after close.
    {
        let (mut store, instance) = fresh_instance(&engine, &component).await;
        let func = instance
            .get_typed_func::<(String, u16, String, String), (Result<Vec<u8>, String>,)>(
                &mut store,
                "test-reconnect-after-close",
            )
            .unwrap();
        let (result,) = func
            .call_async(
                &mut store,
                (
                    "127.0.0.1".to_string(),
                    server.addr.port(),
                    "testdb".to_string(),
                    "testuser".to_string(),
                ),
            )
            .await
            .unwrap();
        let response = result.expect("reconnect should succeed");
        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.contains("first_connection"));
        assert!(response_str.contains("second_connection"));
    }
}

// ── Utility ────────────────────────────────────────────────────────

/// Count the number of ReadyForQuery ('Z' + len=5) markers in a byte slice.
fn count_ready_for_query(data: &[u8]) -> usize {
    let mut count = 0;
    for i in 0..data.len().saturating_sub(5) {
        if data[i] == b'Z' {
            let len = i32::from_be_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]);
            if len == 5 {
                count += 1;
            }
        }
    }
    count
}
