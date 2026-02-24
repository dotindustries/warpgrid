//! MySQL-specific connection backend and factory.
//!
//! Wraps a generic [`ConnectionBackend`] (typically [`TcpBackend`]) with
//! MySQL-aware health checking using the `COM_PING` command. All other
//! operations (send, recv, close) are pure byte passthrough — the guest
//! handles the MySQL wire protocol directly.
//!
//! # MySQL COM_PING Protocol
//!
//! ```text
//! Client → Server:
//!   [payload_len: 3 bytes LE] [seq_id: 1 byte] [COM_PING: 0x0e]
//!   = [0x01, 0x00, 0x00, 0x00, 0x0e]
//!
//! Server → Client (OK):
//!   [payload_len: 3 bytes LE] [seq_id: 1 byte] [0x00 = OK] ...
//!
//! Server → Client (ERR):
//!   [payload_len: 3 bytes LE] [seq_id: 1 byte] [0xff = ERR] ...
//! ```

use std::time::Duration;

use super::tcp::{TcpConnectionFactory, TlsConfig};
use super::{ConnectionBackend, ConnectionFactory, PoolKey};

/// MySQL COM_PING command code.
const COM_PING: u8 = 0x0e;

/// A MySQL COM_PING packet: 3-byte payload length (1) + sequence id (0) + command byte.
const COM_PING_PACKET: [u8; 5] = [0x01, 0x00, 0x00, 0x00, COM_PING];

/// Minimum MySQL packet header size (3 bytes length + 1 byte sequence).
const MYSQL_HEADER_SIZE: usize = 4;

/// MySQL OK response marker (first byte of payload).
const OK_MARKER: u8 = 0x00;

// ── MysqlBackend ─────────────────────────────────────────────────────

/// A [`ConnectionBackend`] wrapper that adds MySQL-specific health checking.
///
/// All operations except `ping()` delegate directly to the inner backend.
/// `ping()` sends a MySQL `COM_PING` command and checks for an OK response.
pub struct MysqlBackend {
    inner: Box<dyn ConnectionBackend>,
}

impl std::fmt::Debug for MysqlBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlBackend")
            .field("inner", &self.inner)
            .finish()
    }
}

impl MysqlBackend {
    /// Wrap an existing backend with MySQL-aware health checking.
    pub fn new(inner: Box<dyn ConnectionBackend>) -> Self {
        Self { inner }
    }
}

impl ConnectionBackend for MysqlBackend {
    fn send(&mut self, data: &[u8]) -> Result<usize, String> {
        self.inner.send(data)
    }

    fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
        self.inner.recv(max_bytes)
    }

    fn ping(&mut self) -> bool {
        // Send COM_PING packet.
        if self.inner.send(&COM_PING_PACKET).is_err() {
            return false;
        }

        // Read the response — at minimum we need the 4-byte header + 1 status byte.
        match self.inner.recv(MYSQL_HEADER_SIZE + 1) {
            Ok(data) if data.len() > MYSQL_HEADER_SIZE => {
                // Check if the response status byte indicates OK (0x00).
                data[MYSQL_HEADER_SIZE] == OK_MARKER
            }
            Ok(data) if data.is_empty() => {
                // EOF — server closed connection.
                false
            }
            Ok(_) => {
                // Partial response — incomplete packet, treat as unhealthy.
                tracing::debug!("mysql COM_PING: incomplete response");
                false
            }
            Err(_) => false,
        }
    }

    fn close(&mut self) {
        self.inner.close();
    }
}

// ── MysqlConnectionFactory ───────────────────────────────────────────

/// Factory creating MySQL connections with COM_PING health checking.
///
/// Delegates TCP/TLS connection establishment to a [`TcpConnectionFactory`],
/// then wraps the resulting backend in a [`MysqlBackend`].
pub struct MysqlConnectionFactory {
    inner: TcpConnectionFactory,
}

impl MysqlConnectionFactory {
    /// Create a factory for plain TCP MySQL connections (no TLS).
    pub fn plain(recv_timeout: Duration, connect_timeout: Duration) -> Self {
        Self {
            inner: TcpConnectionFactory::plain(recv_timeout, connect_timeout),
        }
    }

    /// Create a factory for TLS-wrapped MySQL connections.
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

impl ConnectionFactory for MysqlConnectionFactory {
    fn connect(
        &self,
        key: &PoolKey,
        password: Option<&str>,
    ) -> Result<Box<dyn ConnectionBackend>, String> {
        let tcp_backend = self.inner.connect(key, password)?;
        tracing::debug!(
            host = %key.host,
            port = key.port,
            "wrapping tcp connection with mysql COM_PING health check"
        );
        Ok(Box::new(MysqlBackend::new(tcp_backend)))
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // ── Mock backend for testing MysqlBackend ────────────────────────

    /// Mock backend that records sent data and returns configurable responses.
    #[derive(Debug)]
    struct MockMysqlInner {
        /// Bytes received by the last `send()` call.
        last_sent: Vec<u8>,
        /// Response to return on next `recv()`.
        recv_response: Vec<u8>,
        /// Whether recv should fail.
        recv_fails: bool,
        /// Whether send should fail.
        send_fails: bool,
        /// Health flag for the inner ping (unused by MysqlBackend, but required).
        healthy: Arc<AtomicBool>,
        /// Track if close was called.
        closed: bool,
    }

    impl MockMysqlInner {
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

        /// Configure to return a MySQL OK response to COM_PING.
        fn with_ok_response(mut self) -> Self {
            // OK packet: header (3-byte len + seq 1) + 0x00 (OK marker) + padding
            self.recv_response = vec![
                0x07, 0x00, 0x00, // payload length = 7
                0x01, // sequence id = 1
                0x00, // OK marker
                0x00, 0x00, // affected rows = 0
                0x00, 0x00, // last insert id = 0
                0x02, 0x00, // status flags
            ];
            self
        }

        /// Configure to return a MySQL ERR response to COM_PING.
        fn with_err_response(mut self) -> Self {
            // ERR packet: header + 0xff (ERR marker)
            self.recv_response = vec![
                0x05, 0x00, 0x00, // payload length = 5
                0x01, // sequence id = 1
                0xff, // ERR marker
                0xe8, 0x03, // error code
                0x48, 0x59, // state marker + state
            ];
            self
        }
    }

    impl ConnectionBackend for MockMysqlInner {
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

    // ── MysqlBackend: COM_PING tests ────────────────────────────────

    #[test]
    fn mysql_ping_sends_com_ping_packet() {
        let inner = MockMysqlInner::new().with_ok_response();
        let mut backend = MysqlBackend::new(Box::new(inner));

        let healthy = backend.ping();
        assert!(healthy, "COM_PING with OK response should return true");
    }

    #[test]
    fn mysql_ping_returns_false_on_err_response() {
        let inner = MockMysqlInner::new().with_err_response();
        let mut backend = MysqlBackend::new(Box::new(inner));

        let healthy = backend.ping();
        assert!(!healthy, "COM_PING with ERR response should return false");
    }

    #[test]
    fn mysql_ping_returns_false_on_send_failure() {
        let mut inner = MockMysqlInner::new();
        inner.send_fails = true;
        let mut backend = MysqlBackend::new(Box::new(inner));

        assert!(!backend.ping(), "send failure should return false");
    }

    #[test]
    fn mysql_ping_returns_false_on_recv_failure() {
        let mut inner = MockMysqlInner::new();
        inner.recv_fails = true;
        let mut backend = MysqlBackend::new(Box::new(inner));

        assert!(!backend.ping(), "recv failure should return false");
    }

    #[test]
    fn mysql_ping_returns_false_on_empty_response() {
        // Server closes connection — recv returns empty.
        let inner = MockMysqlInner::new(); // recv_response is empty by default
        let mut backend = MysqlBackend::new(Box::new(inner));

        assert!(!backend.ping(), "empty response (EOF) should return false");
    }

    #[test]
    fn mysql_ping_returns_false_on_short_response() {
        let mut inner = MockMysqlInner::new();
        inner.recv_response = vec![0x01, 0x00]; // Too short for a valid packet
        let mut backend = MysqlBackend::new(Box::new(inner));

        assert!(!backend.ping(), "short response should return false");
    }

    // ── MysqlBackend: passthrough tests ─────────────────────────────

    #[test]
    fn mysql_send_delegates_to_inner() {
        let inner = MockMysqlInner::new();
        let mut backend = MysqlBackend::new(Box::new(inner));

        let data = b"mysql handshake bytes";
        let sent = backend.send(data).unwrap();
        assert_eq!(sent, data.len());
    }

    #[test]
    fn mysql_recv_delegates_to_inner() {
        let mut inner = MockMysqlInner::new();
        inner.recv_response = b"server response".to_vec();
        let mut backend = MysqlBackend::new(Box::new(inner));

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, b"server response");
    }

    #[test]
    fn mysql_close_delegates_to_inner() {
        let inner = MockMysqlInner::new();
        let mut backend = MysqlBackend::new(Box::new(inner));
        backend.close();
        // Close doesn't return anything, just verify no panic.
    }

    // ── MysqlConnectionFactory tests ────────────────────────────────

    #[test]
    fn mysql_factory_creates_plain_tcp_connection() {
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

        let factory = MysqlConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            addr.port(),
            "testdb",
            "user",
            super::super::Protocol::MySQL,
        );

        let backend = factory.connect(&key, None);
        assert!(backend.is_ok(), "factory should create a connection");
    }

    #[test]
    fn mysql_factory_connect_refused() {
        let factory = MysqlConnectionFactory::plain(
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            1,
            "testdb",
            "user",
            super::super::Protocol::MySQL,
        );

        let result = factory.connect(&key, None);
        assert!(result.is_err());
    }

    // ── Integration-style test: MysqlBackend with real TCP ──────────

    #[test]
    fn mysql_backend_send_recv_over_tcp() {
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

        let factory = MysqlConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::with_protocol(
            "127.0.0.1",
            addr.port(),
            "testdb",
            "user",
            super::super::Protocol::MySQL,
        );

        let mut backend = factory.connect(&key, None).unwrap();

        // MySQL handshake-like bytes (binary data passthrough).
        let mysql_greeting = vec![0x0a, b'm', b'y', b's', b'q', b'l', 0x00];
        let sent = backend.send(&mysql_greeting).unwrap();
        assert_eq!(sent, mysql_greeting.len());

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, mysql_greeting, "MySQL bytes must pass through unmodified");
    }
}
