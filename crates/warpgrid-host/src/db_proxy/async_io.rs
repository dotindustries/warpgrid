//! Async I/O traits for the database proxy (US-506).
//!
//! Provides non-blocking versions of [`ConnectionBackend`] and [`ConnectionFactory`]
//! that use tokio async I/O. This allows the database proxy to send/receive data
//! without blocking the async executor, enabling concurrent query execution
//! within a single Wasm module instance.
//!
//! # Key difference from sync path
//!
//! The sync [`ConnectionBackend`] uses blocking `std::net::TcpStream`. When the
//! pool manager calls `send`/`recv`, it holds its internal mutex for the entire
//! I/O duration, preventing other connections from being accessed.
//!
//! The async path releases the mutex *before* I/O and reacquires it after,
//! enabling true concurrent database access across multiple connections.
//!
//! [`ConnectionBackend`]: super::ConnectionBackend
//! [`ConnectionFactory`]: super::ConnectionFactory

use std::future::Future;
use std::pin::Pin;

use super::PoolKey;

/// Boxed future alias for async connection factory results.
pub type AsyncConnectFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn AsyncConnectionBackend>, String>> + Send + 'a>>;

/// Async version of [`super::ConnectionBackend`] using non-blocking I/O.
///
/// Implementations use tokio async I/O (e.g., `tokio::net::TcpStream`) so that
/// send/recv operations don't block the async executor. The pool manager releases
/// its internal lock before calling these methods, allowing concurrent I/O across
/// multiple connections.
pub trait AsyncConnectionBackend: Send + std::fmt::Debug {
    /// Send bytes over the connection asynchronously. Returns bytes sent.
    fn send_async<'a>(
        &'a mut self,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>>;

    /// Receive up to `max_bytes` from the connection asynchronously.
    fn recv_async<'a>(
        &'a mut self,
        max_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;

    /// Async health-check ping. Returns `true` if the connection is alive.
    fn ping_async(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;

    /// Close the underlying transport asynchronously.
    fn close_async(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Factory for creating new async connections — injected for testability.
pub trait AsyncConnectionFactory: Send + Sync {
    /// Establish a new async connection to the given target.
    fn connect_async<'a>(
        &'a self,
        key: &'a PoolKey,
        password: Option<&'a str>,
    ) -> AsyncConnectFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Mock async backend for testing — echoes sent data on recv.
    #[derive(Debug)]
    struct MockAsyncBackend {
        buf: Vec<u8>,
        healthy: Arc<AtomicBool>,
    }

    impl MockAsyncBackend {
        fn new() -> Self {
            Self {
                buf: Vec::new(),
                healthy: Arc::new(AtomicBool::new(true)),
            }
        }
    }

    impl AsyncConnectionBackend for MockAsyncBackend {
        fn send_async<'a>(
            &'a mut self,
            data: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
            Box::pin(async move {
                self.buf = data.to_vec();
                Ok(data.len())
            })
        }

        fn recv_async<'a>(
            &'a mut self,
            max_bytes: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>> {
            Box::pin(async move {
                let len = max_bytes.min(self.buf.len());
                Ok(self.buf[..len].to_vec())
            })
        }

        fn ping_async(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            let healthy = self.healthy.load(Ordering::Relaxed);
            Box::pin(async move { healthy })
        }

        fn close_async(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
    }

    struct MockAsyncFactory;

    impl AsyncConnectionFactory for MockAsyncFactory {
        fn connect_async<'a>(
            &'a self,
            _key: &'a PoolKey,
            _password: Option<&'a str>,
        ) -> AsyncConnectFuture<'a> {
            Box::pin(async {
                Ok(Box::new(MockAsyncBackend::new()) as Box<dyn AsyncConnectionBackend>)
            })
        }
    }

    #[tokio::test]
    async fn async_backend_send_recv_echo() {
        let mut backend = MockAsyncBackend::new();
        let sent = backend.send_async(b"SELECT 1").await.unwrap();
        assert_eq!(sent, 8);

        let received = backend.recv_async(1024).await.unwrap();
        assert_eq!(received, b"SELECT 1");
    }

    #[tokio::test]
    async fn async_backend_ping_healthy() {
        let mut backend = MockAsyncBackend::new();
        assert!(backend.ping_async().await);
    }

    #[tokio::test]
    async fn async_backend_ping_unhealthy() {
        let mut backend = MockAsyncBackend::new();
        backend.healthy.store(false, Ordering::Relaxed);
        assert!(!backend.ping_async().await);
    }

    #[tokio::test]
    async fn async_factory_creates_backend() {
        let factory = MockAsyncFactory;
        let key = PoolKey::new("host", 5432, "db", "user");
        let backend = factory.connect_async(&key, None).await;
        assert!(backend.is_ok());
    }
}
