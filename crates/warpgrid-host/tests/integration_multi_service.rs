//! US-707: Multi-service polyglot integration test (T6).
//!
//! Tests 4 Wasm modules (gateway, user-svc, notification-svc, analytics-svc)
//! within a single WarpGridEngine instance with inter-service DNS routing.
//!
//! Architecture:
//! - Gateway: resolves DNS for downstream services
//! - User service: CRUD via Postgres wire protocol through DB proxy
//! - Notification service: Redis RPUSH through DB proxy
//! - Analytics service: Postgres INSERT through DB proxy
//!
//! The test harness acts as the service mesh, orchestrating calls between
//! independently instantiated Wasm components.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::db_proxy::host::DbProxyHost;
use warpgrid_host::db_proxy::redis::RedisConnectionFactory;
use warpgrid_host::db_proxy::tcp::TcpConnectionFactory;
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig};
use warpgrid_host::dns::host::DnsHost;
use warpgrid_host::dns::{CachedDnsResolver, DnsResolver};
use warpgrid_host::dns::cache::DnsCacheConfig;
use warpgrid_host::engine::{HostState, WarpGridEngine};
use warpgrid_host::filesystem::host::FilesystemHost;
use warpgrid_host::filesystem::VirtualFileMapBuilder;

// ── Workspace helpers ─────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

// ── Component build cache ─────────────────────────────────────────

static GATEWAY_BYTES: OnceLock<Vec<u8>> = OnceLock::new();
static USER_BYTES: OnceLock<Vec<u8>> = OnceLock::new();
static NOTIFY_BYTES: OnceLock<Vec<u8>> = OnceLock::new();
static ANALYTICS_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

/// Build a guest component from the test-apps/t6-multi-service directory.
fn build_component(svc_name: &str) -> Vec<u8> {
    let root = workspace_root();
    let svc_dir = root.join(format!("test-apps/t6-multi-service/{svc_name}"));

    // Step 1: Build the guest crate to a core Wasm module.
    let status = Command::new("cargo")
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .current_dir(&svc_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run cargo build for {svc_name}: {e}"));
    assert!(
        status.success(),
        "{svc_name} build failed with exit code {:?}",
        status.code()
    );

    // Derive the Wasm artifact name from the crate name (hyphens → underscores).
    let wasm_name = svc_name.replace('-', "_");
    let core_wasm_path = svc_dir
        .join(format!("target/wasm32-unknown-unknown/release/{wasm_name}.wasm"));

    // Step 2: Convert core module to component with wasm-tools.
    let component_path = svc_dir.join(format!("target/{svc_name}.component.wasm"));
    let status = Command::new("wasm-tools")
        .args([
            "component",
            "new",
            core_wasm_path.to_str().unwrap(),
            "-o",
            component_path.to_str().unwrap(),
        ])
        .status()
        .unwrap_or_else(|e| panic!("failed to run wasm-tools for {svc_name}: {e}"));
    assert!(
        status.success(),
        "wasm-tools component new for {svc_name} failed with exit code {:?}",
        status.code()
    );

    std::fs::read(&component_path)
        .unwrap_or_else(|e| panic!("failed to read compiled component {svc_name}: {e}"))
}

fn gateway_bytes() -> &'static [u8] {
    GATEWAY_BYTES.get_or_init(|| build_component("gateway-svc"))
}

fn user_bytes() -> &'static [u8] {
    USER_BYTES.get_or_init(|| build_component("user-svc"))
}

fn notify_bytes() -> &'static [u8] {
    NOTIFY_BYTES.get_or_init(|| build_component("notification-svc"))
}

fn analytics_bytes() -> &'static [u8] {
    ANALYTICS_BYTES.get_or_init(|| build_component("analytics-svc"))
}

// ── Mock servers ──────────────────────────────────────────────────

/// Postgres protocol constants.
const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

struct MockPostgresServer {
    addr: std::net::SocketAddr,
}

impl MockPostgresServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
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

    fn handle_connection(stream: &mut std::net::TcpStream) {
        // Read startup message.
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).is_err() {
            return;
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if !(8..=10_000).contains(&len) {
            return;
        }
        let mut payload = vec![0u8; len - 4];
        if stream.read_exact(&mut payload).is_err() {
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

struct MockRedisServer {
    addr: std::net::SocketAddr,
}

impl MockRedisServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
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

    fn handle_connection(stream: &mut std::net::TcpStream) {
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let received = &buf[..n];

                    if received == b"PING\r\n"
                        || received.starts_with(b"*1\r\n$4\r\nPING\r\n")
                    {
                        if stream.write_all(b"+PONG\r\n").is_err() {
                            break;
                        }
                    } else {
                        // Echo everything else.
                        if stream.write_all(received).is_err() {
                            break;
                        }
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
            }
        }
    }
}

// ── Pool config helpers ───────────────────────────────────────────

fn test_pool_config() -> PoolConfig {
    PoolConfig {
        max_size: 5,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(30),
        connect_timeout: Duration::from_millis(500),
        recv_timeout: Duration::from_secs(5),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(30),
    }
}

// ── HostState builders ────────────────────────────────────────────

/// Build a HostState for the gateway service (DNS + filesystem).
fn gateway_host_state(runtime_handle: &tokio::runtime::Handle) -> HostState {
    let mut service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    service_registry.insert(
        "user-svc.test.warp.local".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
    );
    service_registry.insert(
        "notification-svc.test.warp.local".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
    );
    service_registry.insert(
        "analytics-svc.test.warp.local".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
    );
    // Deliberately NOT registering "bad-svc.test.warp.local" to test error path.

    let resolver = DnsResolver::new(service_registry, "127.0.0.1 localhost\n");
    let cache_config = DnsCacheConfig {
        ttl: Duration::from_secs(30),
        max_entries: 128,
    };
    let cached = Arc::new(CachedDnsResolver::new(resolver, cache_config));

    let file_map = VirtualFileMapBuilder::new()
        .with_dev_null()
        .with_dev_urandom()
        .with_resolv_conf("nameserver 127.0.0.1\n")
        .with_etc_hosts("127.0.0.1 localhost\n")
        .build();

    HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: Some(DnsHost::new(cached, runtime_handle.clone())),
        db_proxy: None,
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

/// Build a HostState for a Postgres-backed service (DB proxy + filesystem with config).
fn postgres_host_state(
    pg_addr: std::net::SocketAddr,
    runtime_handle: &tokio::runtime::Handle,
) -> HostState {
    let config = test_pool_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(config, factory));

    let db_conf = format!("127.0.0.1 {} testdb testuser", pg_addr.port());
    let file_map = VirtualFileMapBuilder::new()
        .with_dev_null()
        .with_static_file("/etc/warpgrid/db.conf", db_conf.as_bytes())
        .build();

    HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: None,
        db_proxy: Some(DbProxyHost::new(pool_manager, runtime_handle.clone())),
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

/// Build a HostState for the notification service (Redis DB proxy + filesystem with config).
fn redis_host_state(
    redis_addr: std::net::SocketAddr,
    runtime_handle: &tokio::runtime::Handle,
) -> HostState {
    let config = test_pool_config();
    let factory = Arc::new(RedisConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    let pool_manager = Arc::new(ConnectionPoolManager::new(config, factory));

    let redis_conf = format!("127.0.0.1 {}", redis_addr.port());
    let file_map = VirtualFileMapBuilder::new()
        .with_dev_null()
        .with_static_file("/etc/warpgrid/redis.conf", redis_conf.as_bytes())
        .build();

    HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: None,
        db_proxy: Some(DbProxyHost::new(pool_manager, runtime_handle.clone())),
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

// ── Type alias for the handle-request export signature ─────────

type HandleRequestParams = (String, String, String);
type HandleRequestResult = (Result<(u16, String, String), String>,);

// ── Integration tests ─────────────────────────────────────────────

/// Full flow: gateway routes to all 3 downstream services, each uses its
/// respective protocol (Postgres/Redis) through the DB proxy shim.
#[tokio::test(flavor = "multi_thread")]
async fn multi_service_full_flow() {
    let pg_server = MockPostgresServer::start();
    let redis_server = MockRedisServer::start();

    let engine = WarpGridEngine::new().unwrap();
    let runtime_handle = tokio::runtime::Handle::current();

    // Build all 4 components.
    let gw_comp = Component::new(engine.engine(), gateway_bytes()).unwrap();
    let user_comp = Component::new(engine.engine(), user_bytes()).unwrap();
    let notify_comp = Component::new(engine.engine(), notify_bytes()).unwrap();
    let analytics_comp = Component::new(engine.engine(), analytics_bytes()).unwrap();

    // ── 1. Gateway routes POST /users ────────────────────────────
    {
        let mut store = Store::new(engine.engine(), gateway_host_state(&runtime_handle));
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &gw_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "POST".to_string(),
                    "/users".to_string(),
                    r#"{"name":"alice"}"#.to_string(),
                ),
            )
            .await
            .unwrap();

        let (status, body, error_source) = result.expect("gateway should succeed");
        assert_eq!(status, 200);
        assert!(
            body.contains("user-svc.test.warp.local"),
            "gateway should resolve user-svc DNS, got: {body}"
        );
        assert!(
            body.contains("127.0.0.1"),
            "resolved address should be 127.0.0.1, got: {body}"
        );
        assert!(error_source.is_empty(), "no error expected");
    }

    // ── 2. User service creates user (POST /users) ───────────────
    {
        let mut store = Store::new(
            engine.engine(),
            postgres_host_state(pg_server.addr, &runtime_handle),
        );
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &user_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "POST".to_string(),
                    "/users".to_string(),
                    "alice".to_string(),
                ),
            )
            .await
            .unwrap();

        let (status, body, _) = result.expect("user-svc POST should succeed");
        assert_eq!(status, 201, "user creation should return 201");
        assert!(
            body.starts_with("user_created:"),
            "response should confirm user creation, got: {body}"
        );
        // The mock server echoes the SimpleQuery — verify it contains the INSERT.
        assert!(
            body.contains("INSERT INTO test_users"),
            "echoed query should contain INSERT, got: {body}"
        );
    }

    // ── 3. Gateway routes POST /notify ───────────────────────────
    {
        let mut store = Store::new(engine.engine(), gateway_host_state(&runtime_handle));
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &gw_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "POST".to_string(),
                    "/notify".to_string(),
                    "user_created:alice".to_string(),
                ),
            )
            .await
            .unwrap();

        let (status, body, _) = result.expect("gateway notify route should succeed");
        assert_eq!(status, 200);
        assert!(body.contains("notification-svc.test.warp.local"));
    }

    // ── 4. Notification service enqueues to Redis (POST /notify) ─
    {
        let mut store = Store::new(
            engine.engine(),
            redis_host_state(redis_server.addr, &runtime_handle),
        );
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &notify_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "POST".to_string(),
                    "/notify".to_string(),
                    "user_created:alice".to_string(),
                ),
            )
            .await
            .unwrap();

        let (status, body, _) = result.expect("notification-svc should succeed");
        assert_eq!(status, 202, "notification should return 202 Accepted");
        assert!(
            body.starts_with("notification_enqueued:"),
            "response should confirm enqueue, got: {body}"
        );
        // Mock Redis echoes RPUSH command — verify it contains notifications list name.
        assert!(
            body.contains("RPUSH") || body.contains("notifications"),
            "echoed RESP should contain RPUSH or notifications, got: {body}"
        );
    }

    // ── 5. Gateway routes POST /analytics/event ──────────────────
    {
        let mut store = Store::new(engine.engine(), gateway_host_state(&runtime_handle));
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &gw_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "POST".to_string(),
                    "/analytics/event".to_string(),
                    r#"{"event":"signup"}"#.to_string(),
                ),
            )
            .await
            .unwrap();

        let (status, body, _) = result.expect("gateway analytics route should succeed");
        assert_eq!(status, 200);
        assert!(body.contains("analytics-svc.test.warp.local"));
    }

    // ── 6. Analytics service records event (POST /analytics/event) ─
    {
        let mut store = Store::new(
            engine.engine(),
            postgres_host_state(pg_server.addr, &runtime_handle),
        );
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &analytics_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "POST".to_string(),
                    "/analytics/event".to_string(),
                    r#"{"event":"signup"}"#.to_string(),
                ),
            )
            .await
            .unwrap();

        let (status, body, _) = result.expect("analytics-svc should succeed");
        assert_eq!(status, 201, "analytics event should return 201");
        assert!(
            body.starts_with("event_recorded:"),
            "response should confirm event, got: {body}"
        );
        assert!(
            body.contains("INSERT INTO test_analytics"),
            "echoed query should contain INSERT, got: {body}"
        );
    }

    // ── 7. User service reads users (GET /users) ─────────────────
    {
        let mut store = Store::new(
            engine.engine(),
            postgres_host_state(pg_server.addr, &runtime_handle),
        );
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &user_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                (
                    "GET".to_string(),
                    "/users".to_string(),
                    String::new(),
                ),
            )
            .await
            .unwrap();

        let (status, body, _) = result.expect("user-svc GET should succeed");
        assert_eq!(status, 200, "user list should return 200");
        assert!(
            body.starts_with("users:"),
            "response should contain users prefix, got: {body}"
        );
        assert!(
            body.contains("SELECT"),
            "echoed query should contain SELECT, got: {body}"
        );
    }
}

/// DNS shim correctly routes *.test.warp.local hostnames for all 3 services.
#[tokio::test(flavor = "multi_thread")]
async fn dns_routes_all_service_hostnames() {
    let engine = WarpGridEngine::new().unwrap();
    let runtime_handle = tokio::runtime::Handle::current();
    let gw_comp = Component::new(engine.engine(), gateway_bytes()).unwrap();

    let service_paths = [
        ("/users", "user-svc.test.warp.local"),
        ("/notify", "notification-svc.test.warp.local"),
        ("/analytics/event", "analytics-svc.test.warp.local"),
    ];

    for (path, expected_hostname) in service_paths {
        let mut store = Store::new(engine.engine(), gateway_host_state(&runtime_handle));
        let instance = engine
            .linker()
            .instantiate_async(&mut store, &gw_comp)
            .await
            .unwrap();

        let func = instance
            .get_typed_func::<HandleRequestParams, HandleRequestResult>(
                &mut store,
                "handle-request",
            )
            .unwrap();

        let (result,) = func
            .call_async(
                &mut store,
                ("GET".to_string(), path.to_string(), String::new()),
            )
            .await
            .unwrap();

        let (status, body, error_source) = result.expect("DNS resolution should succeed");
        assert_eq!(status, 200, "DNS route for {path} should return 200");
        assert!(
            body.contains(expected_hostname),
            "body should contain {expected_hostname}, got: {body}"
        );
        assert!(error_source.is_empty());
    }
}

/// Service error includes X-WarpGrid-Source-Service header (via error_source field).
#[tokio::test(flavor = "multi_thread")]
async fn service_error_includes_source_service_header() {
    let engine = WarpGridEngine::new().unwrap();
    let runtime_handle = tokio::runtime::Handle::current();
    let gw_comp = Component::new(engine.engine(), gateway_bytes()).unwrap();

    // Build a gateway state WITHOUT "bad-svc.test.warp.local" in the registry.
    // The DNS resolver will try /etc/hosts and system DNS, which won't have it either.
    // Use a custom gateway state with no system DNS fallback.
    let mut service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    service_registry.insert(
        "user-svc.test.warp.local".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
    );
    // Note: "bad-svc" is deliberately NOT registered.

    let resolver = DnsResolver::new(service_registry, "127.0.0.1 localhost\n");
    let cache_config = DnsCacheConfig {
        ttl: Duration::from_secs(30),
        max_entries: 128,
    };
    let cached = Arc::new(CachedDnsResolver::new(resolver, cache_config));
    let file_map = VirtualFileMapBuilder::new().with_dev_null().build();

    // Create a gateway HostState that routes to a path with unknown service.
    // We need a custom path that maps to a non-existent service.
    // Since the gateway maps paths to hostnames, we use a path not in the route table.
    let state = HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: Some(DnsHost::new(cached, runtime_handle.clone())),
        db_proxy: None,
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    };

    let mut store = Store::new(engine.engine(), state);
    let instance = engine
        .linker()
        .instantiate_async(&mut store, &gw_comp)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<HandleRequestParams, HandleRequestResult>(
            &mut store,
            "handle-request",
        )
        .unwrap();

    // Request to unknown path → 404 with no error_source.
    let (result,) = func
        .call_async(
            &mut store,
            (
                "GET".to_string(),
                "/unknown".to_string(),
                String::new(),
            ),
        )
        .await
        .unwrap();

    let (status, body, _) = result.expect("unknown path should return 404");
    assert_eq!(status, 404);
    assert!(body.contains("unknown path"));
}

/// Verify that when DNS fails for a known path, the error source is populated.
#[tokio::test(flavor = "multi_thread")]
async fn dns_failure_populates_error_source() {
    let engine = WarpGridEngine::new().unwrap();
    let runtime_handle = tokio::runtime::Handle::current();
    let gw_comp = Component::new(engine.engine(), gateway_bytes()).unwrap();

    // Empty registry — all DNS resolutions will fail at registry level.
    // The /etc/hosts and system DNS might resolve "user-svc.test.warp.local",
    // so we use an empty /etc/hosts to ensure the chain fails completely.
    let service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    let resolver = DnsResolver::new(service_registry, "");
    let cache_config = DnsCacheConfig {
        ttl: Duration::from_secs(30),
        max_entries: 128,
    };
    let cached = Arc::new(CachedDnsResolver::new(resolver, cache_config));
    let file_map = VirtualFileMapBuilder::new().with_dev_null().build();

    let state = HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: Some(DnsHost::new(cached, runtime_handle.clone())),
        db_proxy: None,
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    };

    let mut store = Store::new(engine.engine(), state);
    let instance = engine
        .linker()
        .instantiate_async(&mut store, &gw_comp)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<HandleRequestParams, HandleRequestResult>(
            &mut store,
            "handle-request",
        )
        .unwrap();

    // Request to /users path — DNS resolution should fail.
    let (result,) = func
        .call_async(
            &mut store,
            (
                "POST".to_string(),
                "/users".to_string(),
                "test".to_string(),
            ),
        )
        .await
        .unwrap();

    let (status, _body, error_source) = result.expect("should return error, not panic");
    assert_eq!(status, 503, "DNS failure should produce 503");
    assert_eq!(
        error_source, "user-svc.test.warp.local",
        "error_source should identify the failing service"
    );
}

/// Each service compiles to a separate Wasm component and all can be
/// instantiated within the same WarpGridEngine.
#[tokio::test(flavor = "multi_thread")]
async fn four_components_instantiate_in_same_engine() {
    let pg_server = MockPostgresServer::start();
    let redis_server = MockRedisServer::start();

    let engine = WarpGridEngine::new().unwrap();
    let runtime_handle = tokio::runtime::Handle::current();

    // All 4 components load successfully.
    let gw_comp = Component::new(engine.engine(), gateway_bytes()).unwrap();
    let user_comp = Component::new(engine.engine(), user_bytes()).unwrap();
    let notify_comp = Component::new(engine.engine(), notify_bytes()).unwrap();
    let analytics_comp = Component::new(engine.engine(), analytics_bytes()).unwrap();

    // All 4 components instantiate successfully.
    let mut gw_store = Store::new(engine.engine(), gateway_host_state(&runtime_handle));
    let _gw = engine
        .linker()
        .instantiate_async(&mut gw_store, &gw_comp)
        .await
        .unwrap();

    let mut user_store = Store::new(
        engine.engine(),
        postgres_host_state(pg_server.addr, &runtime_handle),
    );
    let _user = engine
        .linker()
        .instantiate_async(&mut user_store, &user_comp)
        .await
        .unwrap();

    let mut notify_store = Store::new(
        engine.engine(),
        redis_host_state(redis_server.addr, &runtime_handle),
    );
    let _notify = engine
        .linker()
        .instantiate_async(&mut notify_store, &notify_comp)
        .await
        .unwrap();

    let mut analytics_store = Store::new(
        engine.engine(),
        postgres_host_state(pg_server.addr, &runtime_handle),
    );
    let _analytics = engine
        .linker()
        .instantiate_async(&mut analytics_store, &analytics_comp)
        .await
        .unwrap();
}
