//! Redis-specific connection backend and factory.
//!
//! Wraps a generic [`ConnectionBackend`] (typically [`TcpBackend`]) with
//! Redis-aware health checking using the `PING` command. All other
//! operations (send, recv, close) are pure byte passthrough — the guest
//! handles the Redis RESP protocol directly.
//!
//! # Redis PING/PONG Protocol
//!
//! ```text
//! Client → Server:
//!   PING\r\n        (inline command format)
//!
//! Server → Client:
//!   +PONG\r\n       (RESP Simple String response)
//! ```
//!
//! Redis AUTH is forwarded transparently as part of the byte stream —
//! the host does not intercept or parse credentials.

use std::time::Duration;

use super::tcp::{TcpConnectionFactory, TlsConfig};
use super::{ConnectionBackend, ConnectionFactory, PoolKey};

/// Redis PING command in inline format (simplest, universally supported).
const REDIS_PING: &[u8] = b"PING\r\n";

/// Expected PONG response (RESP Simple String).
const REDIS_PONG: &[u8] = b"+PONG\r\n";

// ── RedisBackend ────────────────────────────────────────────────────

/// A [`ConnectionBackend`] wrapper that adds Redis-specific health checking.
///
/// All operations except `ping()` delegate directly to the inner backend.
/// `ping()` sends a Redis `PING` command and checks for a `+PONG\r\n` response.
pub struct RedisBackend {
    inner: Box<dyn ConnectionBackend>,
}

impl std::fmt::Debug for RedisBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisBackend")
            .field("inner", &self.inner)
            .finish()
    }
}

impl RedisBackend {
    /// Wrap an existing backend with Redis-aware health checking.
    pub fn new(inner: Box<dyn ConnectionBackend>) -> Self {
        Self { inner }
    }
}

impl ConnectionBackend for RedisBackend {
    fn send(&mut self, data: &[u8]) -> Result<usize, String> {
        self.inner.send(data)
    }

    fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
        self.inner.recv(max_bytes)
    }

    fn ping(&mut self) -> bool {
        // Send PING command.
        if self.inner.send(REDIS_PING).is_err() {
            return false;
        }

        // Read the response — expect "+PONG\r\n" (7 bytes).
        match self.inner.recv(REDIS_PONG.len()) {
            Ok(data) if data.is_empty() => {
                // EOF — server closed connection.
                false
            }
            Ok(data) => {
                // Check if the response is exactly "+PONG\r\n".
                data == REDIS_PONG
            }
            Err(_) => false,
        }
    }

    fn close(&mut self) {
        self.inner.close();
    }
}

// ── RedisConnectionFactory ──────────────────────────────────────────

/// Factory creating Redis connections with PING/PONG health checking.
///
/// Delegates TCP/TLS connection establishment to a [`TcpConnectionFactory`],
/// then wraps the resulting backend in a [`RedisBackend`].
pub struct RedisConnectionFactory {
    inner: TcpConnectionFactory,
}

impl RedisConnectionFactory {
    /// Create a factory for plain TCP Redis connections (no TLS).
    pub fn plain(recv_timeout: Duration, connect_timeout: Duration) -> Self {
        Self {
            inner: TcpConnectionFactory::plain(recv_timeout, connect_timeout),
        }
    }

    /// Create a factory for TLS-wrapped Redis connections.
    pub fn with_tls(
        recv_timeout: Duration,
        connect_timeout: Duration,
        tls_config: TlsConfig,
    ) -> Self {
        Self {
            inner: TcpConnectionFactory::with_tls(recv_timeout, connect_timeout, tls_config),
        }
    }
}

impl ConnectionFactory for RedisConnectionFactory {
    fn connect(
        &self,
        key: &PoolKey,
        password: Option<&str>,
    ) -> Result<Box<dyn ConnectionBackend>, String> {
        let tcp_backend = self.inner.connect(key, password)?;
        tracing::debug!(
            host = %key.host,
            port = key.port,
            "wrapping tcp connection with redis PING/PONG health check"
        );
        Ok(Box::new(RedisBackend::new(tcp_backend)))
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // ── Mock backend for testing RedisBackend ─────────────────────────

    /// Mock backend that records sent data and returns configurable responses.
    #[derive(Debug)]
    struct MockRedisInner {
        /// Bytes received by the last `send()` call.
        last_sent: Vec<u8>,
        /// Response to return on next `recv()`.
        recv_response: Vec<u8>,
        /// Whether recv should fail.
        recv_fails: bool,
        /// Whether send should fail.
        send_fails: bool,
        /// Health flag for the inner ping (unused by RedisBackend, but required).
        healthy: Arc<AtomicBool>,
        /// Track if close was called.
        closed: bool,
    }

    impl MockRedisInner {
        fn new() -> Self {
            Self {
                last_sent: vec![],
                recv_response: vec![],
                recv_fails: false,
                send_fails: false,
                healthy: Arc::new(AtomicBool::new(true)),
                closed: false,
            }
        }

        /// Configure to return a Redis PONG response.
        fn with_pong_response(mut self) -> Self {
            self.recv_response = b"+PONG\r\n".to_vec();
            self
        }

        /// Configure to return a Redis error response.
        fn with_err_response(mut self) -> Self {
            self.recv_response = b"-ERR not authenticated\r\n".to_vec();
            self
        }

        /// Configure to return a partial/unexpected response.
        fn with_unexpected_response(mut self) -> Self {
            self.recv_response = b"+OK\r\n".to_vec();
            self
        }
    }

    impl ConnectionBackend for MockRedisInner {
        fn send(&mut self, data: &[u8]) -> Result<usize, String> {
            if self.send_fails {
                return Err("send failed".to_string());
            }
            self.last_sent = data.to_vec();
            Ok(data.len())
        }

        fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
            if self.recv_fails {
                return Err("recv failed".to_string());
            }
            let len = max_bytes.min(self.recv_response.len());
            Ok(self.recv_response[..len].to_vec())
        }

        fn ping(&mut self) -> bool {
            self.healthy.load(Ordering::Relaxed)
        }

        fn close(&mut self) {
            self.closed = true;
        }
    }

    // ── RedisBackend: PING/PONG tests ────────────────────────────────

    #[test]
    fn redis_ping_sends_ping_and_receives_pong() {
        let inner = MockRedisInner::new().with_pong_response();
        let mut backend = RedisBackend::new(Box::new(inner));

        let healthy = backend.ping();
        assert!(healthy, "PING with PONG response should return true");
    }

    #[test]
    fn redis_ping_returns_false_on_err_response() {
        let inner = MockRedisInner::new().with_err_response();
        let mut backend = RedisBackend::new(Box::new(inner));

        let healthy = backend.ping();
        assert!(!healthy, "PING with ERR response should return false");
    }

    #[test]
    fn redis_ping_returns_false_on_unexpected_response() {
        let inner = MockRedisInner::new().with_unexpected_response();
        let mut backend = RedisBackend::new(Box::new(inner));

        let healthy = backend.ping();
        assert!(!healthy, "PING with unexpected response should return false");
    }

    #[test]
    fn redis_ping_returns_false_on_send_failure() {
        let mut inner = MockRedisInner::new();
        inner.send_fails = true;
        let mut backend = RedisBackend::new(Box::new(inner));

        assert!(!backend.ping(), "send failure should return false");
    }

    #[test]
    fn redis_ping_returns_false_on_recv_failure() {
        let mut inner = MockRedisInner::new();
        inner.recv_fails = true;
        let mut backend = RedisBackend::new(Box::new(inner));

        assert!(!backend.ping(), "recv failure should return false");
    }

    #[test]
    fn redis_ping_returns_false_on_empty_response() {
        // Server closes connection — recv returns empty.
        let inner = MockRedisInner::new(); // recv_response is empty by default
        let mut backend = RedisBackend::new(Box::new(inner));

        assert!(!backend.ping(), "empty response (EOF) should return false");
    }

    // ── RedisBackend: passthrough tests ──────────────────────────────

    #[test]
    fn redis_send_delegates_to_inner() {
        let inner = MockRedisInner::new();
        let mut backend = RedisBackend::new(Box::new(inner));

        // Redis SET command as raw RESP bytes.
        let data = b"*3\r\n$3\r\nSET\r\n$3\r\nkey\r\n$5\r\nvalue\r\n";
        let sent = backend.send(data).unwrap();
        assert_eq!(sent, data.len());
    }

    #[test]
    fn redis_recv_delegates_to_inner() {
        let mut inner = MockRedisInner::new();
        inner.recv_response = b"+OK\r\n".to_vec();
        let mut backend = RedisBackend::new(Box::new(inner));

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, b"+OK\r\n");
    }

    #[test]
    fn redis_close_delegates_to_inner() {
        let inner = MockRedisInner::new();
        let mut backend = RedisBackend::new(Box::new(inner));
        backend.close();
        // Close doesn't return anything, just verify no panic.
    }

    #[test]
    fn redis_auth_forwarded_transparently() {
        let inner = MockRedisInner::new();
        let mut backend = RedisBackend::new(Box::new(inner));

        // Redis AUTH command as raw RESP bytes — host must not intercept.
        let auth = b"*2\r\n$4\r\nAUTH\r\n$8\r\npassword\r\n";
        let sent = backend.send(auth).unwrap();
        assert_eq!(sent, auth.len(), "AUTH bytes should pass through unmodified");
    }

    // ── RedisConnectionFactory tests ─────────────────────────────────

    #[test]
    fn redis_factory_creates_plain_tcp_connection() {
        // Start a minimal TCP server that accepts and holds connections.
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            while let Ok((_stream, _)) = listener.accept() {
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        std::thread::sleep(Duration::from_millis(10));

        let factory = RedisConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            addr.port(),
            "",
            "",
            super::super::Protocol::Redis,
        );

        let backend = factory.connect(&key, None);
        assert!(backend.is_ok(), "factory should create a connection");
    }

    #[test]
    fn redis_factory_connect_refused() {
        let factory = RedisConnectionFactory::plain(
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            1,
            "",
            "",
            super::super::Protocol::Redis,
        );

        let result = factory.connect(&key, None);
        assert!(result.is_err());
    }

    // ── Integration-style test: RedisBackend with real TCP ───────────

    #[test]
    fn redis_backend_send_recv_over_tcp() {
        // Start an echo server.
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    use std::io::{Read, Write};
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if stream.write_all(&buf[..n]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        std::thread::sleep(Duration::from_millis(10));

        let factory = RedisConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            addr.port(),
            "",
            "",
            super::super::Protocol::Redis,
        );

        let mut backend = factory.connect(&key, None).unwrap();

        // Redis RESP SET command (binary data passthrough).
        let set_cmd = b"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n";
        let sent = backend.send(set_cmd).unwrap();
        assert_eq!(sent, set_cmd.len());

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, set_cmd.as_slice(), "RESP bytes must pass through unmodified");
    }

    // ── Ping with real TCP mock Redis server ─────────────────────────

    #[test]
    fn redis_ping_over_tcp_with_mock_redis_server() {
        // Start a mock Redis server that responds to PING with +PONG\r\n.
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    use std::io::{Read, Write};
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let received = &buf[..n];
                                if received == b"PING\r\n" {
                                    if stream.write_all(b"+PONG\r\n").is_err() {
                                        break;
                                    }
                                } else {
                                    // Echo other data back.
                                    if stream.write_all(received).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                });
            }
        });
        std::thread::sleep(Duration::from_millis(10));

        let factory = RedisConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            addr.port(),
            "",
            "",
            super::super::Protocol::Redis,
        );

        let mut backend = factory.connect(&key, None).unwrap();
        assert!(backend.ping(), "PING to mock Redis server should return true");
    }

    // ── Connection draining shares ConnectionPoolManager behavior ────

    #[tokio::test]
    async fn redis_connection_draining_rejects_new_connects() {
        use super::super::{ConnectionPoolManager, PoolConfig};

        // Use a mock factory for the pool manager.
        struct RedisTestFactory;
        impl ConnectionFactory for RedisTestFactory {
            fn connect(
                &self,
                _key: &PoolKey,
                _password: Option<&str>,
            ) -> Result<Box<dyn ConnectionBackend>, String> {
                Ok(Box::new(RedisBackend::new(Box::new(MockRedisInner::new().with_pong_response()))))
            }
        }

        let config = PoolConfig {
            max_size: 5,
            connect_timeout: Duration::from_millis(100),
            drain_timeout: Duration::from_millis(100),
            ..PoolConfig::default()
        };
        let factory = Arc::new(RedisTestFactory);
        let mgr = ConnectionPoolManager::new(config, factory);

        let redis_key = PoolKey::with_protocol(
            "redis.warp.local",
            6379,
            "",
            "",
            super::super::Protocol::Redis,
        );

        // Checkout a connection before draining.
        let handle = mgr.checkout(&redis_key, None).await.unwrap();

        // Start draining.
        let mgr = Arc::new(mgr);
        let mgr_clone = Arc::clone(&mgr);
        let drain_handle = tokio::spawn(async move {
            mgr_clone.drain().await
        });

        // New connections should be rejected during drain.
        // Give drain a moment to set the flag.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result = mgr.checkout(&redis_key, None).await;
        assert!(result.is_err(), "checkout during drain should fail");
        assert!(
            result.unwrap_err().contains("draining"),
            "error should mention draining"
        );

        // Release the in-flight connection so drain can complete.
        mgr.release(handle).await.unwrap();
        let force_closed = drain_handle.await.unwrap();
        assert_eq!(force_closed, 0, "no connections should be force-closed");
    }
}
