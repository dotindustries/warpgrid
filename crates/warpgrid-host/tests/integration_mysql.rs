//! US-115: MySQL wire protocol passthrough integration tests.
//!
//! These tests validate MySQL connection pooling, COM_PING health checking,
//! and byte passthrough using a `MockMysqlServer` that speaks a minimal
//! MySQL wire protocol over real TCP connections.
//!
//! The test stack: `ConnectionPoolManager` → `MysqlConnectionFactory` → TCP → `MockMysqlServer`

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use warpgrid_host::db_proxy::mysql::MysqlConnectionFactory;
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig, PoolKey, Protocol};

// ── MySQL protocol constants ─────────────────────────────────────────

/// Build a minimal MySQL server greeting packet.
///
/// Format: [payload_len: 3 LE] [seq: 1] [0x0a] [version\0] [thread_id: 4 LE]
///         [auth_data: 8] [filler: 1] [cap_low: 2] [charset: 1] [status: 2]
///         [cap_high: 2] [auth_len: 1] [reserved: 10] [auth_data2: 13]
fn mysql_server_greeting() -> Vec<u8> {
    let version = b"5.7.0-warpgrid-mock\0";
    let thread_id: u32 = 1;
    let auth_data_1: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let filler: u8 = 0x00;
    let cap_low: u16 = 0xffff; // All capabilities
    let charset: u8 = 0x21; // utf8_general_ci
    let status: u16 = 0x0002; // SERVER_STATUS_AUTOCOMMIT
    let cap_high: u16 = 0x00ff;
    let auth_len: u8 = 21; // length of auth-plugin-data
    let reserved: [u8; 10] = [0; 10];
    let auth_data_2: [u8; 13] = [0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
                                  0x11, 0x12, 0x13, 0x14, 0x00]; // null-terminated

    // Build payload.
    let mut payload = Vec::new();
    payload.push(0x0a); // protocol version
    payload.extend_from_slice(version);
    payload.extend_from_slice(&thread_id.to_le_bytes());
    payload.extend_from_slice(&auth_data_1);
    payload.push(filler);
    payload.extend_from_slice(&cap_low.to_le_bytes());
    payload.push(charset);
    payload.extend_from_slice(&status.to_le_bytes());
    payload.extend_from_slice(&cap_high.to_le_bytes());
    payload.push(auth_len);
    payload.extend_from_slice(&reserved);
    payload.extend_from_slice(&auth_data_2);

    // Wrap in MySQL packet: [payload_len: 3 LE] [seq_id: 1] [payload]
    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(0x00); // sequence id = 0
    packet.extend_from_slice(&payload);
    packet
}

/// MySQL OK packet for COM_PING responses.
fn mysql_ok_packet(seq_id: u8) -> Vec<u8> {
    let payload: [u8; 7] = [
        0x00, // OK marker
        0x00, // affected rows (lenenc)
        0x00, // last insert id (lenenc)
        0x02, 0x00, // status flags (SERVER_STATUS_AUTOCOMMIT)
        0x00, 0x00, // warnings
    ];
    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(seq_id);
    packet.extend_from_slice(&payload);
    packet
}

/// MySQL COM_PING command byte.
const COM_PING: u8 = 0x0e;

// ── MockMysqlServer ──────────────────────────────────────────────────

/// A TCP server that speaks a minimal MySQL wire protocol:
/// 1. Sends server greeting on connection
/// 2. Reads client handshake response
/// 3. Sends OK packet (auth success)
/// 4. Handles COM_PING with OK response
/// 5. Echoes all other packets
struct MockMysqlServer {
    addr: std::net::SocketAddr,
}

impl MockMysqlServer {
    /// Start a mock MySQL server that handles connections.
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

    /// Start a mock server that closes connections after greeting + auth.
    fn start_close_after_auth() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    // Send greeting.
                    let greeting = mysql_server_greeting();
                    let _ = stream.write_all(&greeting);
                    let _ = stream.flush();

                    // Read client handshake response.
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);

                    // Send OK for auth.
                    let ok = mysql_ok_packet(2);
                    let _ = stream.write_all(&ok);
                    let _ = stream.flush();

                    // Close immediately.
                    drop(stream);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    fn handle_connection(stream: &mut std::net::TcpStream) {
        // 1. Send server greeting.
        let greeting = mysql_server_greeting();
        if stream.write_all(&greeting).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        // 2. Read client handshake response (variable length).
        let mut buf = [0u8; 4096];
        if stream.read(&mut buf).is_err() {
            return;
        }

        // 3. Send OK packet (auth success, seq_id = 2).
        let ok = mysql_ok_packet(2);
        if stream.write_all(&ok).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        // 4. Command loop: handle COM_PING, echo everything else.
        loop {
            // Read packet header (4 bytes).
            let mut header = [0u8; 4];
            match stream.read_exact(&mut header) {
                Ok(()) => {}
                Err(_) => break,
            }

            let payload_len =
                header[0] as usize | ((header[1] as usize) << 8) | ((header[2] as usize) << 16);
            let _seq_id = header[3];

            if payload_len == 0 {
                continue;
            }

            // Read payload.
            let mut payload = vec![0u8; payload_len];
            if stream.read_exact(&mut payload).is_err() {
                break;
            }

            // Check command byte.
            if payload[0] == COM_PING {
                // Respond with OK packet.
                let ok = mysql_ok_packet(_seq_id.wrapping_add(1));
                if stream.write_all(&ok).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            } else {
                // Echo the full packet (header + payload) as a single write
                // to avoid TCP partial reads in the client.
                let mut echo_buf = Vec::with_capacity(4 + payload.len());
                echo_buf.extend_from_slice(&header);
                echo_buf.extend_from_slice(&payload);
                if stream.write_all(&echo_buf).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            }
        }
    }

    fn pool_key(&self) -> PoolKey {
        PoolKey::with_protocol(
            "127.0.0.1",
            self.addr.port(),
            "testdb",
            "testuser",
            Protocol::MySQL,
        )
    }
}

// ── Helper functions ────────────────────────────────────────────────

fn default_mysql_pool_config(max_size: usize) -> PoolConfig {
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

fn make_mysql_manager(config: PoolConfig) -> ConnectionPoolManager {
    let factory = Arc::new(MysqlConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    ConnectionPoolManager::new(config, factory)
}

/// Build a minimal MySQL client handshake response.
fn mysql_handshake_response() -> Vec<u8> {
    // Minimal handshake: capability flags + user + auth
    let mut payload = Vec::new();
    payload.extend_from_slice(&0x0003ffffu32.to_le_bytes()); // capability flags
    payload.extend_from_slice(&0x01000000u32.to_le_bytes()); // max packet size
    payload.push(0x21); // charset utf8
    payload.extend_from_slice(&[0u8; 23]); // reserved
    payload.extend_from_slice(b"testuser\0"); // username
    payload.push(0x00); // auth response length = 0

    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(1); // sequence id = 1
    packet.extend_from_slice(&payload);
    packet
}

/// Perform MySQL handshake on a pool handle.
/// Reads server greeting, sends handshake response, reads OK packet.
async fn do_mysql_handshake(mgr: &ConnectionPoolManager, handle: u64) -> Vec<u8> {
    // Read server greeting.
    let mut greeting = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(handle, 4096).await.expect("recv greeting");
        greeting.extend_from_slice(&chunk);
        if !greeting.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !greeting.is_empty(),
        "should receive MySQL server greeting"
    );
    assert_eq!(greeting[4], 0x0a, "protocol version should be 0x0a");

    // Send client handshake response.
    let handshake = mysql_handshake_response();
    mgr.send(handle, &handshake)
        .await
        .expect("send handshake response");

    // Read auth OK.
    let mut auth_ok = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(handle, 1024).await.expect("recv auth OK");
        auth_ok.extend_from_slice(&chunk);
        if auth_ok.len() >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(auth_ok.len() >= 5, "should receive auth OK packet");
    assert_eq!(auth_ok[4], 0x00, "auth OK packet should have OK marker");

    auth_ok
}

// ── Tests ───────────────────────────────────────────────────────────

/// MockMysqlServer responds to connection with server greeting.
#[tokio::test]
async fn mysql_server_sends_greeting_on_connect() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let handle = mgr.checkout(&key, None).await.unwrap();

    // Read server greeting.
    let greeting = mgr.recv(handle, 4096).await.unwrap();
    assert!(!greeting.is_empty(), "should receive server greeting");
    assert_eq!(greeting[4], 0x0a, "protocol version should be 0x0a");

    mgr.release(handle).await.unwrap();
}

/// Full MySQL handshake succeeds through the proxy.
#[tokio::test]
async fn mysql_full_handshake_succeeds() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let handle = mgr.checkout(&key, None).await.unwrap();
    let auth_ok = do_mysql_handshake(&mgr, handle).await;
    assert_eq!(auth_ok[4], 0x00, "should receive OK marker after auth");

    mgr.release(handle).await.unwrap();
}

/// connect, close, reconnect reuses pooled MySQL connection.
#[tokio::test]
async fn mysql_connect_close_reconnect_reuses_pool() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    // First checkout creates a new connection.
    let h1 = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h1).await;

    let stats_active = mgr.stats(&key).await;
    assert_eq!(stats_active.active, 1);
    assert_eq!(stats_active.total, 1);

    // Release returns to pool.
    mgr.release(h1).await.unwrap();
    let stats_idle = mgr.stats(&key).await;
    assert_eq!(stats_idle.idle, 1);
    assert_eq!(stats_idle.total, 1);

    // Second checkout reuses the pooled connection.
    let h2 = mgr.checkout(&key, None).await.unwrap();
    let stats_reused = mgr.stats(&key).await;
    assert_eq!(
        stats_reused.total, 1,
        "total should be 1 — connection was reused"
    );

    // Reused connection should still pass bytes through.
    let query_packet = build_mysql_query_packet(b"SELECT 1;");
    let sent = mgr.send(h2, &query_packet).await.unwrap();
    assert_eq!(sent, query_packet.len());
    // Read full echoed packet (may arrive in multiple TCP segments).
    let mut echoed = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(h2, 4096).await.unwrap();
        echoed.extend_from_slice(&chunk);
        if echoed.len() >= query_packet.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        echoed, query_packet,
        "reused MySQL connection should echo bytes"
    );

    mgr.release(h2).await.unwrap();
}

/// send and recv pass MySQL wire protocol bytes through without modification.
#[tokio::test]
async fn mysql_send_recv_pass_bytes_through() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    // Send a MySQL COM_QUERY-like packet and verify echo.
    let query_packet = build_mysql_query_packet(b"SELECT version()");
    let sent = mgr.send(h, &query_packet).await.unwrap();
    assert_eq!(sent, query_packet.len());

    // Read full echoed packet (may arrive in multiple TCP segments).
    let mut received = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(h, 4096).await.unwrap();
        received.extend_from_slice(&chunk);
        if received.len() >= query_packet.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        received, query_packet,
        "MySQL query bytes must pass through unmodified"
    );

    mgr.release(h).await.unwrap();
}

/// Binary data passes through the MySQL proxy without modification.
#[tokio::test]
async fn mysql_binary_data_passthrough() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    // Arbitrary binary MySQL packet.
    let binary_packet: Vec<u8> = vec![
        0x05, 0x00, 0x00, // payload length = 5
        0x00, // sequence id
        0x03, // COM_QUERY
        0xDE, 0xAD, 0xBE, 0xEF, // binary data
    ];
    let sent = mgr.send(h, &binary_packet).await.unwrap();
    assert_eq!(sent, binary_packet.len());

    // Read full echoed packet (may arrive in multiple TCP segments).
    let mut received = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(h, 4096).await.unwrap();
        received.extend_from_slice(&chunk);
        if received.len() >= binary_packet.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        received, binary_packet,
        "binary MySQL bytes must pass through unmodified"
    );

    mgr.release(h).await.unwrap();
}

/// COM_PING health check detects healthy MySQL connections.
#[tokio::test]
async fn mysql_health_check_com_ping_healthy() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    // Release to idle pool, then health check.
    mgr.release(h).await.unwrap();
    assert_eq!(mgr.stats(&key).await.idle, 1);

    mgr.health_check_idle().await;

    // COM_PING should succeed — connection stays in pool.
    assert_eq!(
        mgr.stats(&key).await.idle, 1,
        "healthy MySQL connection should survive health check"
    );
}

/// COM_PING health check removes dead MySQL connections.
#[tokio::test]
async fn mysql_health_check_com_ping_removes_dead_connections() {
    let server = MockMysqlServer::start_close_after_auth();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    // Release to idle pool.
    mgr.release(h).await.unwrap();
    assert_eq!(mgr.stats(&key).await.idle, 1);

    // Give server time to close.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should detect closed connection via COM_PING failure.
    mgr.health_check_idle().await;

    assert_eq!(
        mgr.stats(&key).await.idle, 0,
        "dead MySQL connection should be removed by COM_PING health check"
    );
}

/// Pool exhaustion returns timeout error for MySQL connections.
#[tokio::test]
async fn mysql_pool_exhausted_returns_error() {
    let server = MockMysqlServer::start();
    let config = PoolConfig {
        max_size: 2,
        connect_timeout: Duration::from_millis(100),
        ..default_mysql_pool_config(2)
    };
    let mgr = make_mysql_manager(config);
    let key = server.pool_key();

    let _h1 = mgr.checkout(&key, None).await.unwrap();
    let _h2 = mgr.checkout(&key, None).await.unwrap();

    let result = mgr.checkout(&key, None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("exhausted"));
}

/// Multiple send/recv cycles on same MySQL connection.
#[tokio::test]
async fn mysql_multiple_send_recv_cycles() {
    let server = MockMysqlServer::start();
    let mgr = make_mysql_manager(default_mysql_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    for i in 0..10 {
        let msg = format!("query {i}");
        let packet = build_mysql_query_packet(msg.as_bytes());
        let sent = mgr.send(h, &packet).await.unwrap();
        assert_eq!(sent, packet.len());

        // Read full echoed packet (may arrive in multiple TCP segments).
        let mut received = Vec::new();
        for _ in 0..20 {
            let chunk = mgr.recv(h, 4096).await.unwrap();
            received.extend_from_slice(&chunk);
            if received.len() >= packet.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            received, packet,
            "cycle {i}: echoed MySQL packet should match"
        );
    }

    mgr.release(h).await.unwrap();
}

/// Connection draining stops new MySQL connections.
#[tokio::test]
async fn mysql_connection_draining() {
    let server = MockMysqlServer::start();
    let config = PoolConfig {
        drain_timeout: Duration::from_millis(100),
        ..default_mysql_pool_config(5)
    };
    let mgr = make_mysql_manager(config);
    let mgr = Arc::new(mgr);
    let key = server.pool_key();

    // Checkout a connection.
    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    // Start drain in background.
    let mgr_clone = Arc::clone(&mgr);
    let drain_handle = tokio::spawn(async move { mgr_clone.drain().await });

    // Give drain time to start, then release.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // New connections should be rejected.
    let result = mgr.checkout(&key, None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("draining"));

    // Release the active connection.
    mgr.release(h).await.unwrap();

    let force_closed = drain_handle.await.unwrap();
    assert_eq!(force_closed, 0, "connection should have drained gracefully");
}

/// MySQL and Postgres connections use separate pools.
#[tokio::test]
async fn mysql_and_postgres_separate_pools() {
    let server = MockMysqlServer::start();
    let _mgr = make_mysql_manager(default_mysql_pool_config(5));

    let mysql_key = server.pool_key();
    let pg_key = PoolKey::new("127.0.0.1", server.addr.port(), "testdb", "testuser");

    assert_ne!(mysql_key, pg_key, "MySQL and Postgres keys should differ");
    assert_eq!(mysql_key.protocol, Protocol::MySQL);
    assert_eq!(pg_key.protocol, Protocol::Postgres);
}

// ── Helper to build MySQL query packets ──────────────────────────────

/// Build a MySQL COM_QUERY packet (command byte 0x03 + query string).
fn build_mysql_query_packet(query: &[u8]) -> Vec<u8> {
    let payload_len = 1 + query.len(); // COM_QUERY byte + query
    let mut packet = Vec::with_capacity(4 + payload_len);
    packet.push((payload_len & 0xff) as u8);
    packet.push(((payload_len >> 8) & 0xff) as u8);
    packet.push(((payload_len >> 16) & 0xff) as u8);
    packet.push(0x00); // sequence id
    packet.push(0x03); // COM_QUERY
    packet.extend_from_slice(query);
    packet
}
