//! US-113: Database proxy unit tests with mock Postgres server.
//!
//! These tests validate connection pooling, health checking, idle timeout,
//! and byte passthrough using a `MockPostgresServer` that speaks the Postgres
//! v3.0 wire protocol startup handshake over real TCP connections.
//!
//! The test stack: `ConnectionPoolManager` → `TcpConnectionFactory` → TCP → `MockPostgresServer`

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use warpgrid_host::db_proxy::tcp::TcpConnectionFactory;
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig, PoolKey};

// ── Postgres protocol constants ─────────────────────────────────────

/// AuthenticationOk: 'R' + Int32(8) + Int32(0)
const AUTH_OK: [u8; 9] = [b'R', 0, 0, 0, 8, 0, 0, 0, 0];

/// ReadyForQuery: 'Z' + Int32(5) + 'I' (idle)
const READY_FOR_QUERY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

/// Expected handshake response size (AuthenticationOk + ReadyForQuery).
const HANDSHAKE_RESPONSE_LEN: usize = AUTH_OK.len() + READY_FOR_QUERY.len();

/// Build a minimal Postgres v3.0 startup message.
///
/// Format: Int32(length) + Int32(196608) + "user\0test\0\0"
fn pg_startup_message() -> Vec<u8> {
    let user_param = b"user\0test\0\0";
    let length = (4 + 4 + user_param.len()) as u32;
    let mut msg = Vec::with_capacity(length as usize);
    msg.extend_from_slice(&length.to_be_bytes());
    msg.extend_from_slice(&196608u32.to_be_bytes()); // Protocol v3.0
    msg.extend_from_slice(user_param);
    msg
}

// ── MockPostgresServer ──────────────────────────────────────────────

/// A TCP server that speaks a minimal Postgres v3.0 startup handshake,
/// then echoes all subsequent bytes (wire protocol passthrough).
struct MockPostgresServer {
    addr: std::net::SocketAddr,
}

impl MockPostgresServer {
    /// Start a mock Postgres server that:
    /// 1. Accepts connections
    /// 2. Reads the Postgres v3.0 startup message
    /// 3. Responds with AuthenticationOk + ReadyForQuery
    /// 4. Echoes all subsequent bytes
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

    /// Start a mock server that closes connections immediately after handshake.
    /// Used for testing health check detection of dead connections.
    fn start_close_after_handshake() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    if Self::read_startup_message(&mut stream).is_err() {
                        return;
                    }
                    let _ = stream.write_all(&AUTH_OK);
                    let _ = stream.write_all(&READY_FOR_QUERY);
                    let _ = stream.flush();
                    // Close immediately — connection becomes unhealthy.
                    drop(stream);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    /// Read and validate a Postgres v3.0 startup message from a stream.
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

        // Verify protocol version 3.0 (196608 = 0x00030000).
        if payload.len() >= 4 {
            let version =
                u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            assert_eq!(version, 196608, "expected Postgres v3.0 protocol");
        }

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

        // Echo mode — pass bytes through without modification.
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

    fn pool_key(&self) -> PoolKey {
        PoolKey::new("127.0.0.1", self.addr.port(), "testdb", "testuser")
    }
}

// ── Helper functions ────────────────────────────────────────────────

fn default_pool_config(max_size: usize) -> PoolConfig {
    PoolConfig {
        max_size,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(30),
        connect_timeout: Duration::from_millis(500),
        recv_timeout: Duration::from_secs(5),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(30),
    }
}

fn make_manager(config: PoolConfig) -> ConnectionPoolManager {
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    ConnectionPoolManager::new(config, factory)
}

/// Perform Postgres startup handshake on a pool handle.
/// Sends the startup message and reads the AuthOk + ReadyForQuery response.
async fn do_handshake(mgr: &ConnectionPoolManager, handle: u64) -> Vec<u8> {
    let startup = pg_startup_message();
    mgr.send(handle, &startup)
        .await
        .expect("send startup message");

    // Read handshake response — may arrive in chunks over TCP.
    let mut response = Vec::new();
    for _ in 0..20 {
        let chunk = mgr
            .recv(handle, 1024)
            .await
            .expect("recv handshake response");
        response.extend_from_slice(&chunk);
        if response.len() >= HANDSHAKE_RESPONSE_LEN {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    response
}

// ── Tests ───────────────────────────────────────────────────────────

/// MockPostgresServer listens on local TCP port and responds to startup handshake.
#[tokio::test]
async fn mock_postgres_server_responds_to_startup_handshake() {
    let server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    let handle = mgr.checkout(&key, None).await.unwrap();
    let response = do_handshake(&mgr, handle).await;

    // Verify AuthenticationOk.
    assert!(
        response.len() >= AUTH_OK.len(),
        "response too short for AuthenticationOk: {} bytes",
        response.len()
    );
    assert_eq!(response[0], b'R', "first byte should be 'R' (Authentication)");
    assert_eq!(
        &response[..AUTH_OK.len()],
        &AUTH_OK,
        "should receive AuthenticationOk"
    );

    // Verify ReadyForQuery.
    assert!(
        response.len() >= HANDSHAKE_RESPONSE_LEN,
        "response should include ReadyForQuery, got {} bytes",
        response.len()
    );
    assert_eq!(
        &response[AUTH_OK.len()..HANDSHAKE_RESPONSE_LEN],
        &READY_FOR_QUERY,
        "should receive ReadyForQuery"
    );

    mgr.release(handle).await.unwrap();
}

/// connect returns valid handle, close returns to pool, reconnect reuses pooled connection.
#[tokio::test]
async fn connect_close_reconnect_reuses_pooled_connection() {
    let server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    // First checkout creates a new connection.
    let h1 = mgr.checkout(&key, None).await.unwrap();
    assert!(h1 > 0);
    do_handshake(&mgr, h1).await;

    let stats_active = mgr.stats(&key).await;
    assert_eq!(stats_active.active, 1);
    assert_eq!(stats_active.total, 1);

    // Release returns connection to the pool.
    mgr.release(h1).await.unwrap();
    let stats_idle = mgr.stats(&key).await;
    assert_eq!(stats_idle.active, 0);
    assert_eq!(stats_idle.idle, 1);
    assert_eq!(stats_idle.total, 1);

    // Second checkout reuses the pooled connection (total stays 1).
    let h2 = mgr.checkout(&key, None).await.unwrap();
    let stats_reused = mgr.stats(&key).await;
    assert_eq!(stats_reused.active, 1);
    assert_eq!(
        stats_reused.total, 1,
        "total should still be 1 — connection was reused, not newly created"
    );

    // The reused connection should still be usable for byte passthrough.
    let query = b"SELECT 1;";
    let sent = mgr.send(h2, query).await.unwrap();
    assert_eq!(sent, query.len());
    let echoed = mgr.recv(h2, 1024).await.unwrap();
    assert_eq!(echoed, query, "reused connection should still pass bytes through");

    mgr.release(h2).await.unwrap();
}

/// When pool size is exhausted, checkout returns a timeout error.
#[tokio::test]
async fn pool_exhausted_returns_timeout_error() {
    let server = MockPostgresServer::start();
    let config = PoolConfig {
        max_size: 2,
        connect_timeout: Duration::from_millis(100),
        ..default_pool_config(2)
    };
    let mgr = make_manager(config);
    let key = server.pool_key();

    // Fill the pool.
    let h1 = mgr.checkout(&key, None).await.unwrap();
    let h2 = mgr.checkout(&key, None).await.unwrap();

    // Third checkout should timeout since both slots are occupied.
    let result = mgr.checkout(&key, None).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("exhausted"),
        "error should mention pool exhaustion, got: {err}"
    );

    // Verify wait count was incremented.
    let stats = mgr.stats(&key).await;
    assert_eq!(stats.wait_count, 1, "wait_count should be 1 after timeout");

    mgr.release(h1).await.unwrap();
    mgr.release(h2).await.unwrap();
}

/// Idle connections are reaped after the configured timeout.
#[tokio::test]
async fn idle_connections_reaped_after_configured_timeout() {
    let server = MockPostgresServer::start();
    let config = PoolConfig {
        idle_timeout: Duration::from_millis(50),
        ..default_pool_config(5)
    };
    let mgr = make_manager(config);
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_handshake(&mgr, h).await;
    mgr.release(h).await.unwrap();

    assert_eq!(mgr.stats(&key).await.idle, 1, "should have 1 idle connection");

    // Wait for idle timeout to expire.
    tokio::time::sleep(Duration::from_millis(100)).await;
    mgr.reap_idle().await;

    let stats = mgr.stats(&key).await;
    assert_eq!(stats.idle, 0, "idle connection should be reaped after timeout");
    assert_eq!(stats.total, 0, "total count should reflect the reaped connection");
}

/// Health check removes connections that fail the ping.
#[tokio::test]
async fn health_check_removes_connections_that_fail_ping() {
    // This server closes the connection right after the handshake.
    let server = MockPostgresServer::start_close_after_handshake();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_handshake(&mgr, h).await;
    mgr.release(h).await.unwrap();

    assert_eq!(mgr.stats(&key).await.idle, 1);

    // Give the server time to close its side of the connection.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should detect the closed connection via ping.
    mgr.health_check_idle().await;

    assert_eq!(
        mgr.stats(&key).await.idle,
        0,
        "unhealthy connection should be removed by health check"
    );
}

/// send and recv pass bytes through the mock Postgres server without modification.
#[tokio::test]
async fn send_recv_pass_bytes_through_without_modification() {
    let server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_handshake(&mgr, h).await;

    // Send Postgres-like SimpleQuery message: 'Q' + Int32(len) + "SELECT 1\0"
    let query_bytes: Vec<u8> = vec![
        b'Q',                                              // Message type
        0, 0, 0, 14,                                       // Length (14 including self)
        b'S', b'E', b'L', b'E', b'C', b'T', b' ', b'1', 0, // "SELECT 1\0"
    ];

    let sent = mgr.send(h, &query_bytes).await.unwrap();
    assert_eq!(sent, query_bytes.len());

    // Mock server echoes bytes back — verify exact passthrough.
    let received = mgr.recv(h, 1024).await.unwrap();
    assert_eq!(
        received, query_bytes,
        "bytes must pass through the mock server unmodified"
    );

    mgr.release(h).await.unwrap();
}

/// Binary data (arbitrary byte patterns) passes through without modification.
#[tokio::test]
async fn binary_data_passthrough_preserves_exact_bytes() {
    let server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_handshake(&mgr, h).await;

    // Postgres startup message bytes (binary, not ASCII).
    let pg_binary: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x00, 0x03, 0x00, 0x00];
    let sent = mgr.send(h, &pg_binary).await.unwrap();
    assert_eq!(sent, 8);

    let received = mgr.recv(h, 1024).await.unwrap();
    assert_eq!(
        received.as_slice(),
        &pg_binary,
        "raw binary bytes must pass through unmodified"
    );

    mgr.release(h).await.unwrap();
}

/// send or recv on an invalid handle returns an error.
#[tokio::test]
async fn send_recv_on_invalid_handle_returns_error() {
    let _server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));

    // send on non-existent handle.
    let send_result = mgr.send(999, b"data").await;
    assert!(send_result.is_err());
    assert!(
        send_result.unwrap_err().contains("invalid handle"),
        "send error should mention 'invalid handle'"
    );

    // recv on non-existent handle.
    let recv_result = mgr.recv(999, 1024).await;
    assert!(recv_result.is_err());
    assert!(
        recv_result.unwrap_err().contains("invalid handle"),
        "recv error should mention 'invalid handle'"
    );
}

/// Multiple send/recv cycles on the same connection pass data correctly.
#[tokio::test]
async fn multiple_send_recv_cycles() {
    let server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_handshake(&mgr, h).await;

    for i in 0..10 {
        let msg = format!("query {i}");
        let sent = mgr.send(h, msg.as_bytes()).await.unwrap();
        assert_eq!(sent, msg.len());

        let received = mgr.recv(h, 1024).await.unwrap();
        assert_eq!(received, msg.as_bytes(), "cycle {i}: echoed data should match");
    }

    mgr.release(h).await.unwrap();
}

/// Full lifecycle: checkout, handshake, send, recv, release — then handle is invalid.
#[tokio::test]
async fn full_lifecycle_checkout_handshake_send_recv_release() {
    let server = MockPostgresServer::start();
    let mgr = make_manager(default_pool_config(5));
    let key = server.pool_key();

    let handle = mgr.checkout(&key, None).await.unwrap();

    // Postgres handshake.
    let response = do_handshake(&mgr, handle).await;
    assert_eq!(response[0], b'R');

    // Send query bytes.
    let query = b"SELECT version();";
    let sent = mgr.send(handle, query).await.unwrap();
    assert_eq!(sent, query.len());

    // Receive echoed bytes.
    let data = mgr.recv(handle, 1024).await.unwrap();
    assert_eq!(data.as_slice(), query.as_slice());

    // Release.
    mgr.release(handle).await.unwrap();

    // Handle is invalid after release.
    assert!(mgr.send(handle, b"x").await.is_err());
    assert!(mgr.recv(handle, 1).await.is_err());
}
