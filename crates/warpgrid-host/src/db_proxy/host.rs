//! Database proxy host functions.
//!
//! Implements the `warpgrid:shim/database-proxy` [`Host`] trait, delegating
//! connection management to the [`ConnectionPoolManager`].
//!
//! # Connection flow
//!
//! ```text
//! Guest calls connect(config)
//!   → DbProxyHost delegates to ConnectionPoolManager::checkout()
//!     → Pool has idle conn → reuse → Ok(handle)
//!     → Pool exhausted    → wait/timeout → Ok(handle) or Err
//!     → New pool key      → create conn → Ok(handle)
//!
//! Guest calls send(handle, data) / recv(handle, max_bytes)
//!   → DbProxyHost delegates to ConnectionPoolManager::send/recv()
//!
//! Guest calls close(handle)
//!   → DbProxyHost delegates to ConnectionPoolManager::release()
//!     → Healthy conn → returned to pool
//!     → Unhealthy   → destroyed
//! ```

use std::sync::Arc;

use crate::bindings::warpgrid::shim::database_proxy::{ConnectConfig, Host};
use super::ConnectionPoolManager;
use super::PoolKey;

/// Host-side implementation of the `warpgrid:shim/database-proxy` interface.
///
/// Wraps a [`ConnectionPoolManager`] and bridges the synchronous WIT Host
/// trait to the async pool manager via `tokio::task::block_in_place`.
pub struct DbProxyHost {
    /// The connection pool manager.
    pool_manager: Arc<ConnectionPoolManager>,
    /// Tokio runtime handle for running async operations from sync context.
    runtime_handle: tokio::runtime::Handle,
}

impl DbProxyHost {
    /// Create a new `DbProxyHost` wrapping the given pool manager.
    pub fn new(
        pool_manager: Arc<ConnectionPoolManager>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            pool_manager,
            runtime_handle,
        }
    }
}

impl Host for DbProxyHost {
    fn connect(&mut self, config: ConnectConfig) -> Result<u64, String> {
        tracing::debug!(
            host = %config.host,
            port = config.port,
            database = %config.database,
            user = %config.user,
            "db_proxy intercept: connect"
        );

        let key = PoolKey::new(&config.host, config.port, &config.database, &config.user);
        let password = config.password.as_deref();
        let mgr = Arc::clone(&self.pool_manager);

        let handle = self.runtime_handle.clone();
        tokio::task::block_in_place(|| handle.block_on(mgr.checkout(&key, password)))
    }

    fn send(&mut self, conn_handle: u64, data: Vec<u8>) -> Result<u32, String> {
        tracing::debug!(
            handle = conn_handle,
            bytes = data.len(),
            "db_proxy intercept: send"
        );

        let mgr = Arc::clone(&self.pool_manager);
        let handle = self.runtime_handle.clone();

        let sent = tokio::task::block_in_place(|| {
            handle.block_on(mgr.send(conn_handle, &data))
        })?;

        Ok(sent as u32)
    }

    fn recv(&mut self, conn_handle: u64, max_bytes: u32) -> Result<Vec<u8>, String> {
        tracing::debug!(
            handle = conn_handle,
            max_bytes = max_bytes,
            "db_proxy intercept: recv"
        );

        let mgr = Arc::clone(&self.pool_manager);
        let handle = self.runtime_handle.clone();

        tokio::task::block_in_place(|| {
            handle.block_on(mgr.recv(conn_handle, max_bytes as usize))
        })
    }

    fn close(&mut self, conn_handle: u64) -> Result<(), String> {
        tracing::debug!(
            handle = conn_handle,
            "db_proxy intercept: close"
        );

        let mgr = Arc::clone(&self.pool_manager);
        let handle = self.runtime_handle.clone();

        tokio::task::block_in_place(|| handle.block_on(mgr.release(conn_handle)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ConnectionBackend, ConnectionFactory, PoolConfig};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    // ── Mock backend and factory ─────────────────────────────────────

    #[derive(Debug)]
    struct MockBackend;

    impl ConnectionBackend for MockBackend {
        fn send(&mut self, data: &[u8]) -> Result<usize, String> {
            Ok(data.len())
        }

        fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
            Ok(vec![0x42; max_bytes.min(4)])
        }

        fn ping(&mut self) -> bool {
            true
        }

        fn close(&mut self) {}
    }

    struct TestFactory {
        connects: AtomicU64,
    }

    impl TestFactory {
        fn new() -> Self {
            Self {
                connects: AtomicU64::new(0),
            }
        }
    }

    impl ConnectionFactory for TestFactory {
        fn connect(
            &self,
            _key: &PoolKey,
            _password: Option<&str>,
        ) -> Result<Box<dyn ConnectionBackend>, String> {
            self.connects.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(MockBackend))
        }
    }

    fn make_host() -> DbProxyHost {
        let factory = Arc::new(TestFactory::new());
        let config = PoolConfig {
            max_size: 5,
            connect_timeout: Duration::from_millis(100),
            ..PoolConfig::default()
        };
        let mgr = Arc::new(ConnectionPoolManager::new(config, factory));
        let handle = tokio::runtime::Handle::current();
        DbProxyHost::new(mgr, handle)
    }

    fn test_connect_config() -> ConnectConfig {
        ConnectConfig {
            host: "db.warp.local".into(),
            port: 5432,
            database: "mydb".into(),
            user: "app".into(),
            password: Some("secret".into()),
        }
    }

    // ── Host trait: connect ──────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_connect_returns_valid_handle() {
        let mut host = make_host();
        let result = host.connect(test_connect_config());
        assert!(result.is_ok());
        assert!(result.unwrap() > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_connect_multiple_returns_different_handles() {
        let mut host = make_host();
        let h1 = host.connect(test_connect_config()).unwrap();
        let h2 = host.connect(test_connect_config()).unwrap();
        assert_ne!(h1, h2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_connect_without_password() {
        let mut host = make_host();
        let config = ConnectConfig {
            password: None,
            ..test_connect_config()
        };
        let result = host.connect(config);
        assert!(result.is_ok());
    }

    // ── Host trait: send ─────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_send_returns_byte_count() {
        let mut host = make_host();
        let handle = host.connect(test_connect_config()).unwrap();
        let sent = host.send(handle, b"SELECT 1".to_vec()).unwrap();
        assert_eq!(sent, 8);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_send_invalid_handle_returns_error() {
        let mut host = make_host();
        let result = host.send(999, b"data".to_vec());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid handle"));
    }

    // ── Host trait: recv ─────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_recv_returns_data() {
        let mut host = make_host();
        let handle = host.connect(test_connect_config()).unwrap();
        let data = host.recv(handle, 1024).unwrap();
        assert!(!data.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_recv_invalid_handle_returns_error() {
        let mut host = make_host();
        let result = host.recv(999, 1024);
        assert!(result.is_err());
    }

    // ── Host trait: close ────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_close_valid_handle() {
        let mut host = make_host();
        let handle = host.connect(test_connect_config()).unwrap();
        let result = host.close(handle);
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_close_invalid_handle_returns_error() {
        let mut host = make_host();
        let result = host.close(999);
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_close_then_send_returns_error() {
        let mut host = make_host();
        let handle = host.connect(test_connect_config()).unwrap();
        host.close(handle).unwrap();
        let result = host.send(handle, b"data".to_vec());
        assert!(result.is_err());
    }

    // ── Full lifecycle via Host trait ────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_full_lifecycle() {
        let mut host = make_host();

        let handle = host.connect(test_connect_config()).unwrap();
        let sent = host.send(handle, b"SELECT 1;".to_vec()).unwrap();
        assert_eq!(sent, 9);
        let data = host.recv(handle, 1024).unwrap();
        assert!(!data.is_empty());
        host.close(handle).unwrap();

        // Handle invalid after close.
        assert!(host.send(handle, b"x".to_vec()).is_err());
        assert!(host.recv(handle, 1).is_err());
        assert!(host.close(handle).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_connection_reuse_after_close() {
        let factory = Arc::new(TestFactory::new());
        let config = PoolConfig {
            max_size: 5,
            connect_timeout: Duration::from_millis(100),
            ..PoolConfig::default()
        };
        let mgr = Arc::new(ConnectionPoolManager::new(config, factory.clone()));
        let handle = tokio::runtime::Handle::current();
        let mut host = DbProxyHost::new(mgr, handle);

        let h1 = host.connect(test_connect_config()).unwrap();
        assert_eq!(factory.connects.load(Ordering::Relaxed), 1);

        host.close(h1).unwrap();

        let _h2 = host.connect(test_connect_config()).unwrap();
        // Reused — no new factory connect.
        assert_eq!(factory.connects.load(Ordering::Relaxed), 1);
    }
}
