//! TCP/TLS connection backend for database proxy wire protocol passthrough.
//!
//! Provides a [`TcpBackend`] that implements [`ConnectionBackend`] over a
//! plain TCP or TLS-wrapped TCP connection. The backend performs **no protocol
//! parsing** — it passes raw bytes between the guest module and the remote
//! database server. TLS is managed transparently: the guest sends/receives
//! plaintext while the host encrypts/decrypts via `rustls`.
//!
//! # Architecture
//!
//! ```text
//! Guest sends plaintext bytes via database-proxy.send()
//!   → TcpBackend::send()
//!     → [TLS encrypt (if enabled)] → TCP write
//!
//! Guest calls database-proxy.recv()
//!   → TcpBackend::recv()
//!     → TCP read → [TLS decrypt (if enabled)]
//!       → plaintext bytes returned to guest
//! ```

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use super::{ConnectionBackend, ConnectionFactory, PoolKey};

// ── Transport ────────────────────────────────────────────────────────

/// Underlying transport — plain TCP or TLS over TCP.
enum Transport {
    /// Unencrypted TCP stream.
    Plain(TcpStream),
    /// TLS-encrypted TCP stream via `rustls`.
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

// ── TcpBackend ───────────────────────────────────────────────────────

/// A [`ConnectionBackend`] using TCP (optionally TLS) for wire protocol passthrough.
///
/// Sends and receives raw bytes without any protocol awareness. When TLS is
/// enabled, the guest sends plaintext and the backend transparently encrypts
/// on send and decrypts on recv.
pub struct TcpBackend {
    transport: Transport,
}

impl std::fmt::Debug for TcpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tls = matches!(self.transport, Transport::Tls(_));
        f.debug_struct("TcpBackend").field("tls", &tls).finish()
    }
}

impl TcpBackend {
    /// Create a backend wrapping a plain TCP stream.
    pub fn plain(stream: TcpStream) -> Self {
        Self {
            transport: Transport::Plain(stream),
        }
    }

    /// Create a backend wrapping a TLS-encrypted TCP stream.
    pub fn tls(stream: rustls::StreamOwned<rustls::ClientConnection, TcpStream>) -> Self {
        Self {
            transport: Transport::Tls(Box::new(stream)),
        }
    }

    /// Get a reference to the underlying TCP stream (regardless of TLS layer).
    fn tcp_stream(&self) -> &TcpStream {
        match &self.transport {
            Transport::Plain(s) => s,
            Transport::Tls(s) => &s.sock,
        }
    }
}

impl ConnectionBackend for TcpBackend {
    fn send(&mut self, data: &[u8]) -> Result<usize, String> {
        let n = data.len();
        match &mut self.transport {
            Transport::Plain(stream) => {
                stream.write_all(data).map_err(|e| format!("tcp send: {e}"))?;
            }
            Transport::Tls(stream) => {
                stream.write_all(data).map_err(|e| format!("tls send: {e}"))?;
            }
        }
        Ok(n)
    }

    fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; max_bytes];
        let n = match &mut self.transport {
            Transport::Plain(stream) => {
                stream.read(&mut buf).map_err(|e| format!("tcp recv: {e}"))?
            }
            Transport::Tls(stream) => {
                stream.read(&mut buf).map_err(|e| format!("tls recv: {e}"))?
            }
        };
        buf.truncate(n);
        Ok(buf)
    }

    fn ping(&mut self) -> bool {
        let stream = self.tcp_stream();

        // Temporarily set a short read timeout for the health check peek.
        let original_timeout = stream.read_timeout().ok().flatten();
        if stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .is_err()
        {
            return false;
        }

        let mut peek_buf = [0u8; 1];
        let alive = match stream.peek(&mut peek_buf) {
            Ok(0) => false, // EOF — peer closed the connection
            Ok(_) => true,  // Data available — connection alive
            Err(e) => {
                // TimedOut or WouldBlock means no data but connection is alive.
                matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
            }
        };

        // Restore original read timeout.
        let _ = stream.set_read_timeout(original_timeout);
        alive
    }

    fn close(&mut self) {
        let _ = self.tcp_stream().shutdown(std::net::Shutdown::Both);
    }
}

// ── TlsConfig ────────────────────────────────────────────────────────

/// Configuration for TLS connections.
#[derive(Clone)]
pub struct TlsConfig {
    /// Pre-built `rustls` client configuration.
    pub client_config: Arc<rustls::ClientConfig>,
}

impl TlsConfig {
    /// Create a TLS config using the Mozilla root certificate store.
    ///
    /// This is the recommended configuration for production use.
    pub fn with_system_roots() -> Result<Self, String> {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls protocol version error: {e}"))?
        .with_root_certificates(root_store)
        .with_no_client_auth();

        Ok(Self {
            client_config: Arc::new(config),
        })
    }

    /// Create a TLS config that **skips certificate verification**.
    ///
    /// # Warning
    ///
    /// This is intended **only for testing**. Do not use in production.
    #[cfg(test)]
    pub fn dangerous_no_verify() -> Self {
        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .expect("safe default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(danger::NoVerifier))
        .with_no_client_auth();

        Self {
            client_config: Arc::new(config),
        }
    }
}

/// Build a `TlsConfig` from a pre-configured `rustls::ClientConfig`.
impl From<Arc<rustls::ClientConfig>> for TlsConfig {
    fn from(client_config: Arc<rustls::ClientConfig>) -> Self {
        Self { client_config }
    }
}

// ── Dangerous cert verifier (test only) ──────────────────────────────

#[cfg(test)]
mod danger {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub struct NoVerifier;

    impl ServerCertVerifier for NoVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }
}

// ── TcpConnectionFactory ─────────────────────────────────────────────

/// Factory creating TCP (optionally TLS) connections to database servers.
///
/// Performs **no protocol handshake** — the guest module handles all
/// protocol negotiation (Postgres startup, MySQL handshake, Redis AUTH, etc.)
/// through the raw byte passthrough.
pub struct TcpConnectionFactory {
    /// Timeout for recv (read) operations on created connections.
    recv_timeout: Duration,
    /// Timeout for establishing TCP connections.
    connect_timeout: Duration,
    /// Optional TLS configuration. If `None`, connections are plain TCP.
    tls_config: Option<TlsConfig>,
}

impl TcpConnectionFactory {
    /// Create a factory for plain TCP connections (no TLS).
    pub fn plain(recv_timeout: Duration, connect_timeout: Duration) -> Self {
        Self {
            recv_timeout,
            connect_timeout,
            tls_config: None,
        }
    }

    /// Create a factory for TLS-wrapped TCP connections.
    pub fn with_tls(
        recv_timeout: Duration,
        connect_timeout: Duration,
        tls_config: TlsConfig,
    ) -> Self {
        Self {
            recv_timeout,
            connect_timeout,
            tls_config: Some(tls_config),
        }
    }
}

impl ConnectionFactory for TcpConnectionFactory {
    fn connect(
        &self,
        key: &PoolKey,
        _password: Option<&str>,
    ) -> Result<Box<dyn ConnectionBackend>, String> {
        // Resolve hostname to socket address.
        let addr_str = format!("{}:{}", key.host, key.port);
        let addr = addr_str
            .to_socket_addrs()
            .map_err(|e| format!("dns resolution failed for {addr_str}: {e}"))?
            .next()
            .ok_or_else(|| format!("no address found for {addr_str}"))?;

        // Establish TCP connection with timeout.
        let stream = TcpStream::connect_timeout(&addr, self.connect_timeout)
            .map_err(|e| format!("tcp connect to {addr_str}: {e}"))?;

        // Configure recv timeout on the stream.
        stream
            .set_read_timeout(Some(self.recv_timeout))
            .map_err(|e| format!("set recv timeout: {e}"))?;

        // Disable Nagle's algorithm for low-latency wire protocol exchange.
        let _ = stream.set_nodelay(true);

        tracing::debug!(
            host = %key.host,
            port = key.port,
            tls = self.tls_config.is_some(),
            "established tcp connection"
        );

        if let Some(tls) = &self.tls_config {
            // Wrap with TLS — guest sends/receives plaintext, we encrypt/decrypt.
            let server_name = rustls::pki_types::ServerName::try_from(key.host.as_str())
                .map_err(|e| format!("invalid tls server name '{}': {e}", key.host))?
                .to_owned();

            let tls_conn =
                rustls::ClientConnection::new(Arc::clone(&tls.client_config), server_name)
                    .map_err(|e| format!("tls session creation: {e}"))?;

            let tls_stream = rustls::StreamOwned::new(tls_conn, stream);
            Ok(Box::new(TcpBackend::tls(tls_stream)))
        } else {
            Ok(Box::new(TcpBackend::plain(stream)))
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // ── Test helpers ────────────────────────────────────────────────

    /// Start a TCP listener on a random port.
    fn start_tcp_listener() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");
        (listener, addr)
    }

    /// Start a TCP echo server in a background thread. Returns its address.
    fn start_echo_server() -> std::net::SocketAddr {
        let (listener, addr) = start_tcp_listener();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
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
        // Give the listener thread time to start.
        std::thread::sleep(Duration::from_millis(10));
        addr
    }

    // ── TcpBackend: send/recv ───────────────────────────────────────

    #[test]
    fn send_and_recv_roundtrip() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        let sent = backend.send(b"hello world").unwrap();
        assert_eq!(sent, 11);

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn passthrough_preserves_exact_bytes() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        // Binary data — Postgres startup message (first 8 bytes of protocol v3.0).
        let pg_startup: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x00, 0x03, 0x00, 0x00];
        let sent = backend.send(&pg_startup).unwrap();
        assert_eq!(sent, 8);

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, pg_startup, "raw bytes must pass through unmodified");
    }

    #[test]
    fn recv_returns_partial_data() {
        let (listener, addr) = start_tcp_listener();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Send exactly 5 bytes.
            stream.write_all(b"short").unwrap();
        });

        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        // Request up to 1024 bytes but only 5 are available.
        let data = backend.recv(1024).unwrap();
        assert_eq!(data, b"short");
    }

    #[test]
    fn recv_timeout_returns_error() {
        let (listener, addr) = start_tcp_listener();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            // Hold connection open but send nothing.
            std::thread::sleep(Duration::from_secs(5));
        });

        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        let result = backend.recv(1024);
        assert!(result.is_err(), "recv should timeout");
    }

    #[test]
    fn send_empty_data_succeeds() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        let mut backend = TcpBackend::plain(stream);

        let sent = backend.send(b"").unwrap();
        assert_eq!(sent, 0);
    }

    #[test]
    fn multiple_send_recv_cycles() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        for i in 0..10 {
            let msg = format!("message {i}");
            let sent = backend.send(msg.as_bytes()).unwrap();
            assert_eq!(sent, msg.len());

            let data = backend.recv(1024).unwrap();
            assert_eq!(data, msg.as_bytes());
        }
    }

    #[test]
    fn large_payload_passthrough() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        // 64 KB payload — larger than typical TCP buffer.
        let payload: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();
        let sent = backend.send(&payload).unwrap();
        assert_eq!(sent, 65536);

        // Recv may return in chunks — collect until we have all bytes.
        let mut received = Vec::new();
        while received.len() < 65536 {
            let chunk = backend.recv(65536 - received.len()).unwrap();
            if chunk.is_empty() {
                break;
            }
            received.extend_from_slice(&chunk);
        }
        assert_eq!(received.len(), 65536);
        assert_eq!(received, payload);
    }

    // ── TcpBackend: ping ────────────────────────────────────────────

    #[test]
    fn ping_healthy_connection() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        let mut backend = TcpBackend::plain(stream);

        assert!(backend.ping(), "healthy connection should ping true");
    }

    #[test]
    fn ping_closed_connection() {
        let (listener, addr) = start_tcp_listener();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // Close immediately.
            drop(stream);
        });

        let stream = TcpStream::connect(addr).unwrap();
        // Give the server time to close.
        std::thread::sleep(Duration::from_millis(50));
        let mut backend = TcpBackend::plain(stream);

        assert!(!backend.ping(), "closed connection should ping false");
    }

    // ── TcpBackend: close ───────────────────────────────────────────

    #[test]
    fn close_terminates_connection() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut backend = TcpBackend::plain(stream);

        backend.close();

        // After close, send should fail.
        let result = backend.send(b"data");
        assert!(result.is_err(), "send after close should fail");
    }

    // ── TcpConnectionFactory ────────────────────────────────────────

    #[test]
    fn factory_creates_working_connection() {
        let addr = start_echo_server();
        let factory = TcpConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::new("127.0.0.1", addr.port(), "testdb", "user");

        let mut backend = factory.connect(&key, None).unwrap();

        let sent = backend.send(b"test").unwrap();
        assert_eq!(sent, 4);

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, b"test");
    }

    #[test]
    fn factory_connect_refused() {
        let factory = TcpConnectionFactory::plain(
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        // Port 1 is unlikely to have a listener.
        let key = PoolKey::new("127.0.0.1", 1, "testdb", "user");

        let result = factory.connect(&key, None);
        assert!(result.is_err());
    }

    #[test]
    fn factory_sets_recv_timeout() {
        let (listener, addr) = start_tcp_listener();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            // Hold connection open, send nothing.
            std::thread::sleep(Duration::from_secs(5));
        });

        let factory = TcpConnectionFactory::plain(
            Duration::from_millis(100), // 100ms recv timeout
            Duration::from_secs(2),
        );
        let key = PoolKey::new("127.0.0.1", addr.port(), "testdb", "user");

        let mut backend = factory.connect(&key, None).unwrap();
        let result = backend.recv(1024);
        assert!(result.is_err(), "recv should timeout with factory-configured timeout");
    }

    #[test]
    fn factory_ignores_password_for_passthrough() {
        let addr = start_echo_server();
        let factory = TcpConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::new("127.0.0.1", addr.port(), "testdb", "user");

        // Password is provided but should be ignored — guest sends auth via wire protocol.
        let mut backend = factory.connect(&key, Some("secret")).unwrap();
        let sent = backend.send(b"test").unwrap();
        assert_eq!(sent, 4);
    }

    // ── TlsConfig ──────────────────────────────────────────────────

    #[test]
    fn tls_config_with_system_roots_succeeds() {
        let config = TlsConfig::with_system_roots();
        assert!(config.is_ok());
    }

    #[test]
    fn tls_config_dangerous_no_verify_succeeds() {
        let _config = TlsConfig::dangerous_no_verify();
    }

    // ── TLS round-trip ──────────────────────────────────────────────

    #[test]
    fn tls_send_and_recv_roundtrip() {
        // Generate a self-signed certificate for the test server.
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert_params =
            rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = cert_params.self_signed(&key_pair).unwrap();
        let cert_der = cert.der().clone();
        let key_der = key_pair.serialize_der();

        // Build rustls ServerConfig.
        let server_cert =
            rustls::pki_types::CertificateDer::from(cert_der.to_vec());
        let server_key =
            rustls::pki_types::PrivateKeyDer::try_from(key_der).unwrap();
        let server_config = Arc::new(
            rustls::ServerConfig::builder_with_provider(
                rustls::crypto::ring::default_provider().into(),
            )
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![server_cert], server_key)
            .unwrap(),
        );

        // Start TLS echo server.
        let (listener, addr) = start_tcp_listener();
        let server_config_clone = server_config.clone();
        std::thread::spawn(move || {
            let (tcp_stream, _) = listener.accept().unwrap();
            let tls_conn =
                rustls::ServerConnection::new(server_config_clone).unwrap();
            let mut tls_stream = rustls::StreamOwned::new(tls_conn, tcp_stream);
            let mut buf = [0u8; 4096];
            loop {
                match tls_stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tls_stream.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Client side — use dangerous_no_verify since cert is self-signed.
        let tls_config = TlsConfig::dangerous_no_verify();
        let factory = TcpConnectionFactory::with_tls(
            Duration::from_secs(2),
            Duration::from_secs(2),
            tls_config,
        );

        // Use 127.0.0.1 (not "localhost") to avoid IPv4/IPv6 mismatch
        // since the listener is bound to 127.0.0.1. With dangerous_no_verify,
        // the cert SAN doesn't need to match the server name.
        let key = PoolKey::new("127.0.0.1", addr.port(), "testdb", "user");
        let mut backend = factory.connect(&key, None).unwrap();

        // Send plaintext — TcpBackend encrypts transparently.
        let sent = backend.send(b"encrypted hello").unwrap();
        assert_eq!(sent, 15);

        // Recv plaintext — TcpBackend decrypts transparently.
        let data = backend.recv(1024).unwrap();
        assert_eq!(data, b"encrypted hello");
    }

    #[test]
    fn tls_passthrough_preserves_exact_bytes() {
        // Generate cert.
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert_params =
            rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = cert_params.self_signed(&key_pair).unwrap();
        let cert_der = cert.der().clone();
        let key_der = key_pair.serialize_der();

        let server_cert =
            rustls::pki_types::CertificateDer::from(cert_der.to_vec());
        let server_key =
            rustls::pki_types::PrivateKeyDer::try_from(key_der).unwrap();
        let server_config = Arc::new(
            rustls::ServerConfig::builder_with_provider(
                rustls::crypto::ring::default_provider().into(),
            )
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![server_cert], server_key)
            .unwrap(),
        );

        let (listener, addr) = start_tcp_listener();
        let server_config_clone = server_config.clone();
        std::thread::spawn(move || {
            let (tcp_stream, _) = listener.accept().unwrap();
            let tls_conn =
                rustls::ServerConnection::new(server_config_clone).unwrap();
            let mut tls_stream = rustls::StreamOwned::new(tls_conn, tcp_stream);
            let mut buf = [0u8; 4096];
            loop {
                match tls_stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tls_stream.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let tls_config = TlsConfig::dangerous_no_verify();
        let factory = TcpConnectionFactory::with_tls(
            Duration::from_secs(2),
            Duration::from_secs(2),
            tls_config,
        );

        // Use 127.0.0.1 to match the listener's IPv4 bind address.
        let key = PoolKey::new("127.0.0.1", addr.port(), "testdb", "user");
        let mut backend = factory.connect(&key, None).unwrap();

        // Binary data — Postgres wire protocol bytes.
        let pg_bytes: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x00, 0x03, 0x00, 0x00];
        backend.send(&pg_bytes).unwrap();

        let data = backend.recv(1024).unwrap();
        assert_eq!(data, pg_bytes, "TLS must not modify wire protocol bytes");
    }

    // ── Full lifecycle ──────────────────────────────────────────────

    #[test]
    fn full_lifecycle_plain_tcp() {
        let addr = start_echo_server();
        let factory = TcpConnectionFactory::plain(
            Duration::from_secs(2),
            Duration::from_secs(2),
        );
        let key = PoolKey::new("127.0.0.1", addr.port(), "testdb", "user");

        let mut backend = factory.connect(&key, None).unwrap();

        // Send Postgres-like startup bytes.
        let startup = b"\x00\x00\x00\x08\x00\x03\x00\x00";
        assert_eq!(backend.send(startup).unwrap(), 8);

        // Recv echo.
        let response = backend.recv(1024).unwrap();
        assert_eq!(response, startup.as_slice());

        // Close.
        backend.close();

        // After close, send should fail.
        assert!(backend.send(b"x").is_err());
    }

    #[test]
    fn debug_format_plain() {
        let addr = start_echo_server();
        let stream = TcpStream::connect(addr).unwrap();
        let backend = TcpBackend::plain(stream);
        let debug = format!("{backend:?}");
        assert!(debug.contains("tls: false"));
    }
}
