//! E2E test: Bun handler queries Postgres in Wasm mode.
//!
//! Compiles bun-postgres-handler with `warp pack --lang bun`, loads the
//! resulting Wasm component into wasmtime with WASI HTTP + database proxy,
//! sends HTTP requests through `wasi:http/incoming-handler.handle()`, and
//! verifies JSON responses.
//!
//! Gated behind the `integration` feature because it requires external
//! tooling: `bun`, `jco`, and `componentize-js`.

#![cfg(feature = "integration")]

use std::fmt;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::bindings::http::types::Scheme;
use wasmtime_wasi_http::bindings::ProxyPre;
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use warpgrid_host::bindings::warpgrid::shim;
use warpgrid_host::db_proxy::host::DbProxyHost;
use warpgrid_host::db_proxy::{ConnectionBackend, ConnectionFactory, ConnectionPoolManager, PoolConfig, PoolKey};

// ── Composite host state ────────────────────────────────────────────

struct E2eState {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    db_proxy: Option<DbProxyHost>,
}

impl WasiView for E2eState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for E2eState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl shim::database_proxy::Host for E2eState {
    fn connect(&mut self, config: shim::database_proxy::ConnectConfig) -> Result<u64, String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "db proxy not enabled".to_string())
            .and_then(|db| db.connect(config))
    }

    fn send(&mut self, handle: u64, data: Vec<u8>) -> Result<u32, String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "db proxy not enabled".to_string())
            .and_then(|db| db.send(handle, data))
    }

    fn recv(&mut self, handle: u64, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "db proxy not enabled".to_string())
            .and_then(|db| db.recv(handle, max_bytes))
    }

    fn close(&mut self, handle: u64) -> Result<(), String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "db proxy not enabled".to_string())
            .and_then(|db| db.close(handle))
    }
}

// ── Mock TCP backend / factory ──────────────────────────────────────

/// Simple TCP backend for the mock redirect factory.
#[derive(Debug)]
struct MockTcpBackend {
    stream: std::net::TcpStream,
}

impl ConnectionBackend for MockTcpBackend {
    fn send(&mut self, data: &[u8]) -> Result<usize, String> {
        self.stream.write(data).map_err(|e| format!("send: {e}"))
    }

    fn recv(&mut self, max: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; max];
        let n = self.stream.read(&mut buf).map_err(|e| format!("recv: {e}"))?;
        buf.truncate(n);
        Ok(buf)
    }

    fn ping(&mut self) -> bool {
        true
    }

    fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Factory that redirects all connections to the mock Postgres server,
/// regardless of the requested host/port. This is needed because the
/// handler.js uses hardcoded `localhost:5432`.
struct MockRedirectFactory {
    target: std::net::SocketAddr,
    recv_timeout: Duration,
}

impl fmt::Debug for MockRedirectFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockRedirectFactory")
            .field("target", &self.target)
            .finish()
    }
}

impl ConnectionFactory for MockRedirectFactory {
    fn connect(
        &self,
        _key: &PoolKey,
        _password: Option<&str>,
    ) -> Result<Box<dyn ConnectionBackend>, String> {
        let stream = std::net::TcpStream::connect_timeout(
            &self.target,
            Duration::from_secs(2),
        )
        .map_err(|e| format!("connect to mock: {e}"))?;
        stream
            .set_read_timeout(Some(self.recv_timeout))
            .map_err(|e| format!("set timeout: {e}"))?;
        stream.set_nodelay(true).ok();
        Ok(Box::new(MockTcpBackend { stream }))
    }
}

// ── Postgres protocol constants ─────────────────────────────────────

const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];
const PARSE_COMPLETE: [u8; 5] = [b'1', 0, 0, 0, 4];
const BIND_COMPLETE: [u8; 5] = [b'2', 0, 0, 0, 4];

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

fn row_description_users() -> Vec<u8> {
    let columns = [
        ("id", 23_i32, 4_i16),
        ("name", 25_i32, -1_i16),
        ("email", 25_i32, -1_i16),
    ];

    let mut fields = Vec::new();
    for (name, type_oid, type_len) in &columns {
        fields.extend_from_slice(name.as_bytes());
        fields.push(0);
        fields.extend_from_slice(&0_i32.to_be_bytes());
        fields.extend_from_slice(&0_i16.to_be_bytes());
        fields.extend_from_slice(&type_oid.to_be_bytes());
        fields.extend_from_slice(&type_len.to_be_bytes());
        fields.extend_from_slice(&(-1_i32).to_be_bytes());
        fields.extend_from_slice(&0_i16.to_be_bytes());
    }

    let field_count = columns.len() as i16;
    let len = (4 + 2 + fields.len()) as i32;
    let mut buf = Vec::with_capacity(1 + len as usize);
    buf.push(b'T');
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&field_count.to_be_bytes());
    buf.extend_from_slice(&fields);
    buf
}

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
    buf.push(b'D');
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&field_count.to_be_bytes());
    buf.extend_from_slice(&field_data);
    buf
}

// ── MockPostgresServer ──────────────────────────────────────────────

/// Mock Postgres server supporting both simple and extended query protocols.
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
        if stream.write_all(&AUTH_OK).is_err()
            || stream.write_all(&READY_FOR_QUERY).is_err()
            || stream.flush().is_err()
        {
            return;
        }

        // Message handling loop
        let mut buf = [0u8; 65536];
        loop {
            let n = match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };

            match buf[0] {
                b'Q' => {
                    // Simple query
                    let sql_end = buf[5..n].iter().position(|&b| b == 0).unwrap_or(n - 5);
                    let sql = std::str::from_utf8(&buf[5..5 + sql_end]).unwrap_or("");
                    let response = Self::handle_simple_query(sql);
                    if stream.write_all(&response).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
                b'P' => {
                    // Extended query batch — parse SQL from 'P' message
                    let sql = Self::extract_parse_sql(&buf[..n]).unwrap_or_default();
                    let response = Self::handle_extended_query(&sql);
                    if stream.write_all(&response).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
                b'X' => break, // Terminate
                _ => {}
            }
        }
    }

    /// Extract SQL from a Parse ('P') message at the start of a buffer.
    fn extract_parse_sql(buf: &[u8]) -> Option<String> {
        if buf.is_empty() || buf[0] != b'P' {
            return None;
        }
        if buf.len() < 5 {
            return None;
        }
        let msg_len = i32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
        let end = std::cmp::min(1 + msg_len, buf.len());
        let payload = &buf[5..end];

        // payload: stmt_name\0 + sql\0 + param_count(2) + param_types
        let stmt_end = payload.iter().position(|&b| b == 0)?;
        let sql_start = stmt_end + 1;
        let sql_end = payload[sql_start..].iter().position(|&b| b == 0)? + sql_start;
        let sql = std::str::from_utf8(&payload[sql_start..sql_end]).ok()?;
        Some(sql.to_string())
    }

    fn handle_simple_query(sql: &str) -> Vec<u8> {
        let mut response = Vec::new();
        let sql_lower = sql.to_lowercase();

        if sql_lower.contains("select") && sql_lower.contains("users") {
            response.extend_from_slice(&row_description_users());
            response.extend_from_slice(&data_row(&["1", "Alice Johnson", "alice@example.com"]));
            response.extend_from_slice(&command_complete("SELECT 1"));
        } else if sql_lower.contains("insert") && sql_lower.contains("users") {
            response.extend_from_slice(&row_description_users());
            response.extend_from_slice(&data_row(&["6", "Test User", "test@example.com"]));
            response.extend_from_slice(&command_complete("INSERT 0 1"));
        } else {
            response.extend_from_slice(&command_complete("SELECT 0"));
        }

        response.extend_from_slice(&READY_FOR_QUERY);
        response
    }

    fn handle_extended_query(sql: &str) -> Vec<u8> {
        let mut response = Vec::new();
        let sql_lower = sql.to_lowercase();

        // ParseComplete + BindComplete
        response.extend_from_slice(&PARSE_COMPLETE);
        response.extend_from_slice(&BIND_COMPLETE);

        if sql_lower.contains("insert") && sql_lower.contains("returning") {
            // INSERT ... RETURNING — return new row
            response.extend_from_slice(&row_description_users());
            response.extend_from_slice(&data_row(&["6", "Test User", "test@example.com"]));
            response.extend_from_slice(&command_complete("INSERT 0 1"));
        } else if sql_lower.contains("select") && sql_lower.contains("where") {
            // SELECT with WHERE — return single user
            response.extend_from_slice(&row_description_users());
            response.extend_from_slice(&data_row(&["1", "Alice Johnson", "alice@example.com"]));
            response.extend_from_slice(&command_complete("SELECT 1"));
        } else if sql_lower.contains("select") {
            // SELECT all
            response.extend_from_slice(&row_description_users());
            response.extend_from_slice(&data_row(&["1", "Alice Johnson", "alice@example.com"]));
            response.extend_from_slice(&command_complete("SELECT 1"));
        } else {
            response.extend_from_slice(&command_complete("SELECT 0"));
        }

        response.extend_from_slice(&READY_FOR_QUERY);
        response
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

fn build_bun_component() -> &'static [u8] {
    COMPONENT_BYTES.get_or_init(|| {
        let fixture_path = workspace_root().join("tests/fixtures/bun-postgres-handler");
        let result = warp_pack::pack_with_lang(&fixture_path, Some("bun"))
            .expect("warp pack --lang bun should succeed for bun-postgres-handler");
        std::fs::read(&result.output_path).expect("read compiled component")
    })
}

// ── Test helpers ────────────────────────────────────────────────────

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

/// Create engine, linker, and ProxyPre for the E2E test.
fn setup_engine_and_linker(
    wasm_bytes: &[u8],
) -> anyhow::Result<(Engine, ProxyPre<E2eState>)> {
    let mut config = Config::new();
    config.async_support(true);
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);

    let engine = Engine::new(&config)?;
    let component = Component::new(&engine, wasm_bytes)?;

    let mut linker = Linker::<E2eState>::new(&engine);

    // Register core WASI interfaces (clock, random, io, cli)
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

    // Register WASI HTTP interfaces (types, outgoing-handler)
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

    // Register WarpGrid database proxy interface
    shim::database_proxy::add_to_linker::<E2eState, HasSelf<E2eState>>(
        &mut linker,
        |state: &mut E2eState| state,
    )?;

    let pre = linker.instantiate_pre(&component)?;
    let proxy_pre = ProxyPre::new(pre)?;

    Ok((engine, proxy_pre))
}

/// Create a Store<E2eState> with the mock redirect factory.
fn create_store(
    engine: &Engine,
    mock_addr: std::net::SocketAddr,
) -> Store<E2eState> {
    let factory = Arc::new(MockRedirectFactory {
        target: mock_addr,
        recv_timeout: Duration::from_secs(5),
    });
    let pool_manager = Arc::new(ConnectionPoolManager::new(test_pool_config(), factory));
    let runtime_handle = tokio::runtime::Handle::current();

    let wasi = WasiCtx::builder()
        .inherit_stderr()
        .build();

    let state = E2eState {
        wasi,
        http: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        db_proxy: Some(DbProxyHost::new(pool_manager, runtime_handle)),
    };

    Store::new(engine, state)
}

/// Send an HTTP request through the Wasm handler and return (status, body).
async fn send_request(
    proxy_pre: &ProxyPre<E2eState>,
    store: &mut Store<E2eState>,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> anyhow::Result<(u16, String)> {
    let body_bytes = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
    let request_body = Full::new(body_bytes).map_err(|never| match never {});

    let uri = format!("http://localhost{path}");
    let mut builder = hyper::Request::builder()
        .method(method)
        .uri(&uri);

    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }

    let request = builder.body(request_body)?;

    let req = store.data_mut().new_incoming_request(Scheme::Http, request)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let out = store.data_mut().new_response_outparam(tx)?;

    let proxy = proxy_pre.instantiate_async(&mut *store).await?;
    proxy
        .wasi_http_incoming_handler()
        .call_handle(store, req, out)
        .await?;

    let response = match rx.await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => anyhow::bail!("handler returned error: {e:?}"),
        Err(_) => anyhow::bail!("response channel closed without response"),
    };

    let status = response.status().as_u16();
    let body_bytes = response.into_body().collect().await?.to_bytes();
    let body_str = String::from_utf8(body_bytes.to_vec())?;

    Ok((status, body_str))
}

// ── Integration Tests ───────────────────────────────────────────────

/// Test: POST /users creates a user and returns 201.
#[tokio::test(flavor = "multi_thread")]
async fn test_post_users_returns_201() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_bun_component();
    let (engine, proxy_pre) = setup_engine_and_linker(wasm_bytes)
        .expect("engine and linker setup should succeed");

    let mut store = create_store(&engine, mock_pg.addr);

    let (status, body) = send_request(
        &proxy_pre,
        &mut store,
        "POST",
        "/users",
        Some(r#"{"name":"Test User","email":"test@example.com"}"#),
    )
    .await
    .expect("POST /users should succeed");

    assert_eq!(status, 201, "POST /users should return 201, body: {body}");
    assert!(body.contains("id"), "response should contain 'id': {body}");
    assert!(
        body.contains("Test User") || body.contains("name"),
        "response should contain user data: {body}"
    );
}

/// Test: GET /users/:id returns 200 with user data.
#[tokio::test(flavor = "multi_thread")]
async fn test_get_user_by_id_returns_200() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_bun_component();
    let (engine, proxy_pre) = setup_engine_and_linker(wasm_bytes)
        .expect("engine and linker setup should succeed");

    let mut store = create_store(&engine, mock_pg.addr);

    let (status, body) = send_request(
        &proxy_pre,
        &mut store,
        "GET",
        "/users/1",
        None,
    )
    .await
    .expect("GET /users/1 should succeed");

    assert_eq!(status, 200, "GET /users/1 should return 200, body: {body}");
    assert!(
        body.contains("Alice Johnson"),
        "response should contain 'Alice Johnson': {body}"
    );
    assert!(
        body.contains("alice@example.com"),
        "response should contain 'alice@example.com': {body}"
    );
}

/// Test: Unknown route returns 404.
#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_route_returns_404() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_bun_component();
    let (engine, proxy_pre) = setup_engine_and_linker(wasm_bytes)
        .expect("engine and linker setup should succeed");

    let mut store = create_store(&engine, mock_pg.addr);

    let (status, body) = send_request(
        &proxy_pre,
        &mut store,
        "GET",
        "/nonexistent",
        None,
    )
    .await
    .expect("GET /nonexistent should succeed");

    assert_eq!(status, 404, "unknown route should return 404, body: {body}");
    assert!(
        body.contains("Not Found"),
        "response should contain 'Not Found': {body}"
    );
}

/// Test: POST /users with missing fields returns 400.
#[tokio::test(flavor = "multi_thread")]
async fn test_post_users_missing_fields_returns_400() {
    let mock_pg = MockPostgresServer::start();
    let wasm_bytes = build_bun_component();
    let (engine, proxy_pre) = setup_engine_and_linker(wasm_bytes)
        .expect("engine and linker setup should succeed");

    let mut store = create_store(&engine, mock_pg.addr);

    let (status, body) = send_request(
        &proxy_pre,
        &mut store,
        "POST",
        "/users",
        Some(r#"{"name":"Only Name"}"#),
    )
    .await
    .expect("POST /users with missing email should succeed");

    assert_eq!(
        status, 400,
        "missing fields should return 400, body: {body}"
    );
}
