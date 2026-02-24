//! US-116: Redis RESP protocol passthrough integration tests.
//!
//! These tests validate Redis connection pooling, PING/PONG health checking,
//! and byte passthrough using a `MockRedisServer` that speaks a minimal
//! Redis RESP protocol over real TCP connections.
//!
//! The test stack: `ConnectionPoolManager` → `RedisConnectionFactory` → TCP → `MockRedisServer`

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use warpgrid_host::db_proxy::redis::RedisConnectionFactory;
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig, PoolKey, Protocol};

// ── MockRedisServer ──────────────────────────────────────────────────

/// A TCP server that speaks a minimal Redis RESP protocol:
/// 1. Responds to PING with +PONG\r\n
/// 2. Responds to AUTH with +OK\r\n
/// 3. Echoes all other data
struct MockRedisServer {
    addr: std::net::SocketAddr,
}

impl MockRedisServer {
    /// Start a mock Redis server.
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

    /// Start a mock server that closes connections immediately after accept.
    fn start_close_immediately() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                // Close immediately.
                drop(stream);
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

                    // Check for inline PING command.
                    if received == b"PING\r\n" {
                        if stream.write_all(b"+PONG\r\n").is_err() {
                            break;
                        }
                    } else if received.starts_with(b"*1\r\n$4\r\nPING\r\n") {
                        // RESP array format PING.
                        if stream.write_all(b"+PONG\r\n").is_err() {
                            break;
                        }
                    } else if received.starts_with(b"*2\r\n$4\r\nAUTH\r\n") {
                        // AUTH command — respond with OK.
                        if stream.write_all(b"+OK\r\n").is_err() {
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

    fn pool_key(&self) -> PoolKey {
        PoolKey::with_protocol(
            "127.0.0.1",
            self.addr.port(),
            "",
            "",
            Protocol::Redis,
        )
    }
}

// ── Helper functions ─────────────────────────────────────────────────

fn default_redis_pool_config(max_size: usize) -> PoolConfig {
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

fn make_redis_manager(config: PoolConfig) -> ConnectionPoolManager {
    let factory = Arc::new(RedisConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    ConnectionPoolManager::new(config, factory)
}

// ── Tests ────────────────────────────────────────────────────────────

/// send and recv pass Redis RESP bytes through without modification.
#[tokio::test]
async fn redis_send_recv_pass_bytes_through() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Send a Redis SET command as raw RESP bytes.
    let set_cmd = b"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n";
    let sent = mgr.send(h, set_cmd).await.unwrap();
    assert_eq!(sent, set_cmd.len());

    // Read the echoed response.
    let data = mgr.recv(h, 4096).await.unwrap();
    assert_eq!(
        data,
        set_cmd.as_slice(),
        "Redis RESP bytes must pass through unmodified"
    );

    mgr.release(h).await.unwrap();
}

/// Binary data passes through the Redis proxy without modification.
#[tokio::test]
async fn redis_binary_data_passthrough() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Arbitrary binary data (not valid RESP, but should still pass through).
    let binary: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF, 0x42];
    let sent = mgr.send(h, &binary).await.unwrap();
    assert_eq!(sent, binary.len());

    let data = mgr.recv(h, 4096).await.unwrap();
    assert_eq!(
        data, binary,
        "binary bytes must pass through unmodified"
    );

    mgr.release(h).await.unwrap();
}

/// connect, close, reconnect reuses pooled Redis connection.
#[tokio::test]
async fn redis_connect_close_reconnect_reuses_pool() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    // First checkout creates a new connection.
    let h1 = mgr.checkout(&key, None).await.unwrap();
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
    let get_cmd = b"*2\r\n$3\r\nGET\r\n$5\r\nmykey\r\n";
    let sent = mgr.send(h2, get_cmd).await.unwrap();
    assert_eq!(sent, get_cmd.len());

    let data = mgr.recv(h2, 4096).await.unwrap();
    assert_eq!(
        data,
        get_cmd.as_slice(),
        "reused Redis connection should echo bytes"
    );

    mgr.release(h2).await.unwrap();
}

/// PING health check detects healthy Redis connections.
#[tokio::test]
async fn redis_health_check_ping_healthy() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Release to idle pool, then health check.
    mgr.release(h).await.unwrap();
    assert_eq!(mgr.stats(&key).await.idle, 1);

    mgr.health_check_idle().await;

    // PING should succeed — connection stays in pool.
    assert_eq!(
        mgr.stats(&key).await.idle, 1,
        "healthy Redis connection should survive health check"
    );
}

/// PING health check removes dead Redis connections.
#[tokio::test]
async fn redis_health_check_ping_removes_dead_connections() {
    let server = MockRedisServer::start_close_immediately();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Release to idle pool.
    mgr.release(h).await.unwrap();
    assert_eq!(mgr.stats(&key).await.idle, 1);

    // Give server time to close.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should detect closed connection via PING failure.
    mgr.health_check_idle().await;

    assert_eq!(
        mgr.stats(&key).await.idle, 0,
        "dead Redis connection should be removed by PING health check"
    );
}

/// Pool exhaustion returns timeout error for Redis connections.
#[tokio::test]
async fn redis_pool_exhausted_returns_error() {
    let server = MockRedisServer::start();
    let config = PoolConfig {
        max_size: 2,
        connect_timeout: Duration::from_millis(100),
        ..default_redis_pool_config(2)
    };
    let mgr = make_redis_manager(config);
    let key = server.pool_key();

    let _h1 = mgr.checkout(&key, None).await.unwrap();
    let _h2 = mgr.checkout(&key, None).await.unwrap();

    let result = mgr.checkout(&key, None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("exhausted"));
}

/// Multiple send/recv cycles on same Redis connection.
#[tokio::test]
async fn redis_multiple_send_recv_cycles() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    for i in 0..10 {
        let cmd = format!("*3\r\n$3\r\nSET\r\n$4\r\nkey{i}\r\n$5\r\nval{i:02}\r\n");
        let sent = mgr.send(h, cmd.as_bytes()).await.unwrap();
        assert_eq!(sent, cmd.len());

        let data = mgr.recv(h, 4096).await.unwrap();
        assert_eq!(
            data,
            cmd.as_bytes(),
            "cycle {i}: echoed Redis RESP should match"
        );
    }

    mgr.release(h).await.unwrap();
}

/// Connection draining stops new Redis connections.
#[tokio::test]
async fn redis_connection_draining() {
    let server = MockRedisServer::start();
    let config = PoolConfig {
        drain_timeout: Duration::from_millis(100),
        ..default_redis_pool_config(5)
    };
    let mgr = make_redis_manager(config);
    let mgr = Arc::new(mgr);
    let key = server.pool_key();

    // Checkout a connection.
    let h = mgr.checkout(&key, None).await.unwrap();

    // Start drain in background.
    let mgr_clone = Arc::clone(&mgr);
    let drain_handle = tokio::spawn(async move { mgr_clone.drain().await });

    // Give drain time to start.
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

/// Redis AUTH is forwarded transparently as part of the byte stream.
#[tokio::test]
async fn redis_auth_forwarded_transparently() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Send AUTH command — host should NOT intercept or parse credentials.
    let auth_cmd = b"*2\r\n$4\r\nAUTH\r\n$8\r\npassword\r\n";
    let sent = mgr.send(h, auth_cmd).await.unwrap();
    assert_eq!(sent, auth_cmd.len());

    // Mock server responds with +OK for AUTH.
    let data = mgr.recv(h, 4096).await.unwrap();
    assert_eq!(data, b"+OK\r\n", "AUTH response should be received");

    mgr.release(h).await.unwrap();
}

/// Redis and Postgres connections use separate pools.
#[tokio::test]
async fn redis_and_postgres_separate_pools() {
    let server = MockRedisServer::start();

    let redis_key = server.pool_key();
    let pg_key = PoolKey::new("127.0.0.1", server.addr.port(), "", "");

    assert_ne!(redis_key, pg_key, "Redis and Postgres keys should differ");
    assert_eq!(redis_key.protocol, Protocol::Redis);
    assert_eq!(pg_key.protocol, Protocol::Postgres);
}

/// Redis connections established as plain TCP work correctly.
#[tokio::test]
async fn redis_plain_tcp_connection() {
    let server = MockRedisServer::start();
    let mgr = make_redis_manager(default_redis_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Basic RESP command — DEL key.
    let del_cmd = b"*2\r\n$3\r\nDEL\r\n$5\r\nmykey\r\n";
    let sent = mgr.send(h, del_cmd).await.unwrap();
    assert_eq!(sent, del_cmd.len());

    let data = mgr.recv(h, 4096).await.unwrap();
    assert_eq!(data, del_cmd.as_slice());

    mgr.release(h).await.unwrap();
}
