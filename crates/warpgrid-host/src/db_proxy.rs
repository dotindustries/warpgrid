//! Database proxy shim — connection pool manager.
//!
//! Provides wire-protocol-level connection pooling for Postgres, MySQL, and Redis.
//! Connections are pooled per `(host, port, database, user)` tuple and exposed
//! to guest modules via opaque `u64` handles.
//!
//! # Architecture
//!
//! ```text
//! Guest calls connect(config)
//!   → ConnectionPoolManager looks up pool for (host, port, database, user)
//!     → Pool exists with idle connection → return handle
//!     → Pool exists but exhausted → wait (with timeout) or error
//!     → No pool exists → create pool, establish connection → return handle
//! ```

pub mod host;
pub mod tcp;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Semaphore};

/// Key identifying a connection pool — connections with the same key share a pool.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
}

impl PoolKey {
    pub fn new(host: &str, port: u16, database: &str, user: &str) -> Self {
        Self {
            host: host.to_string(),
            port,
            database: database.to_string(),
            user: user.to_string(),
        }
    }
}

/// Configuration for the connection pool manager.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Maximum connections per pool key (default: 10).
    pub max_size: usize,
    /// Idle connection timeout — connections idle longer are reaped (default: 300s).
    pub idle_timeout: Duration,
    /// Health check interval for idle connections (default: 30s).
    pub health_check_interval: Duration,
    /// Maximum time to wait for a connection when pool is exhausted (default: 5s).
    pub connect_timeout: Duration,
    /// Timeout for recv (read) operations on connections (default: 30s).
    pub recv_timeout: Duration,
    /// Whether to use TLS for connections (default: true).
    pub use_tls: bool,
    /// Whether to verify TLS certificates (default: true).
    pub verify_certificates: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 10,
            idle_timeout: Duration::from_secs(300),
            health_check_interval: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            recv_timeout: Duration::from_secs(30),
            use_tls: true,
            verify_certificates: true,
        }
    }
}

/// A pooled connection with tracking metadata.
#[derive(Debug)]
pub struct PooledConnection {
    /// Unique ID for this connection within the pool.
    pub id: u64,
    /// When this connection was established.
    pub created_at: Instant,
    /// When this connection was last used (returned to pool or checked out).
    pub last_used: Instant,
    /// Whether this connection is currently healthy.
    pub healthy: bool,
    /// The pool key this connection belongs to.
    pub pool_key: PoolKey,
    /// Opaque connection data — placeholder for real TCP/TLS streams in US-112+.
    /// Stored as an `Option` so it can be taken during send/recv.
    connection_data: Option<Box<dyn ConnectionBackend>>,
}

/// Trait abstracting the underlying transport for testability.
///
/// US-112 (Postgres), US-115 (MySQL), and US-116 (Redis) implement this
/// with real TCP/TLS streams. For US-111, tests use a mock implementation.
pub trait ConnectionBackend: Send + std::fmt::Debug {
    /// Send bytes over the connection. Returns bytes sent.
    fn send(&mut self, data: &[u8]) -> Result<usize, String>;
    /// Receive up to `max_bytes` from the connection.
    fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String>;
    /// Health-check ping. Returns `true` if the connection is alive.
    fn ping(&mut self) -> bool;
    /// Close the underlying transport.
    fn close(&mut self);
}

/// Factory for creating new connections — injected for testability.
pub trait ConnectionFactory: Send + Sync {
    /// Establish a new connection to the given target.
    fn connect(&self, key: &PoolKey, password: Option<&str>) -> Result<Box<dyn ConnectionBackend>, String>;
}

/// Per-key connection pool holding idle connections and a semaphore for bounding.
#[derive(Debug)]
struct Pool {
    /// Idle connections available for checkout.
    idle: Vec<PooledConnection>,
    /// Total connections (idle + checked out) for this pool key.
    total_count: usize,
    /// Semaphore bounding total connections to `max_size`.
    semaphore: Arc<Semaphore>,
}

impl Pool {
    fn new(max_size: usize) -> Self {
        Self {
            idle: Vec::new(),
            total_count: 0,
            semaphore: Arc::new(Semaphore::new(max_size)),
        }
    }
}

/// Pool-level statistics for a single pool key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolStats {
    /// Connections currently checked out by guests.
    pub active: usize,
    /// Connections sitting idle in the pool.
    pub idle: usize,
    /// Total connections (active + idle).
    pub total: usize,
    /// Number of times a checkout had to wait for an available connection.
    pub wait_count: u64,
}

/// Manages connection pools keyed by `(host, port, database, user)` tuple.
///
/// Each unique tuple gets its own bounded pool. Connections are reused
/// when returned via `release()` and reaped when idle too long.
pub struct ConnectionPoolManager {
    /// Per-key pools.
    pools: Mutex<HashMap<PoolKey, Pool>>,
    /// Checked-out connections keyed by handle ID.
    checked_out: Mutex<HashMap<u64, PooledConnection>>,
    /// Next handle ID to allocate (monotonically increasing).
    next_handle: Mutex<u64>,
    /// Pool configuration.
    config: PoolConfig,
    /// Connection factory for creating new connections.
    factory: Arc<dyn ConnectionFactory>,
    /// Per-key wait counters for statistics.
    wait_counts: Mutex<HashMap<PoolKey, u64>>,
}

impl ConnectionPoolManager {
    /// Create a new `ConnectionPoolManager` with the given configuration and factory.
    pub fn new(config: PoolConfig, factory: Arc<dyn ConnectionFactory>) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            checked_out: Mutex::new(HashMap::new()),
            next_handle: Mutex::new(1),
            config,
            factory,
            wait_counts: Mutex::new(HashMap::new()),
        }
    }

    /// Allocate the next connection handle.
    async fn allocate_handle(&self) -> u64 {
        let mut handle = self.next_handle.lock().await;
        let id = *handle;
        *handle += 1;
        id
    }

    /// Checkout a connection from the pool for the given key.
    ///
    /// If an idle connection is available, it is returned immediately.
    /// If the pool is exhausted, waits up to `connect_timeout` for one to become available.
    /// If no connection becomes available in time, creates a new one (if under limit) or errors.
    pub async fn checkout(
        &self,
        key: &PoolKey,
        password: Option<&str>,
    ) -> Result<u64, String> {
        let semaphore = {
            let mut pools = self.pools.lock().await;
            let pool = pools
                .entry(key.clone())
                .or_insert_with(|| Pool::new(self.config.max_size));
            Arc::clone(&pool.semaphore)
        };

        // Try to acquire a semaphore permit within the timeout.
        let permit = match tokio::time::timeout(
            self.config.connect_timeout,
            semaphore.acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => return Err("connection pool semaphore closed".to_string()),
            Err(_) => {
                // Timeout — record wait count.
                let mut wait_counts = self.wait_counts.lock().await;
                *wait_counts.entry(key.clone()).or_insert(0) += 1;
                return Err(format!(
                    "connection pool exhausted for {}:{}/{} (timeout: {:?})",
                    key.host, key.port, key.database, self.config.connect_timeout
                ));
            }
        };

        // We have a permit. Try to reuse an idle connection first.
        let idle_conn = {
            let mut pools = self.pools.lock().await;
            if let Some(pool) = pools.get_mut(key) {
                pool.idle.pop()
            } else {
                None
            }
        };

        let handle = self.allocate_handle().await;

        if let Some(mut conn) = idle_conn {
            // Check if the idle connection is still healthy.
            if conn.healthy {
                conn.last_used = Instant::now();
                conn.id = handle;
                tracing::debug!(
                    handle = handle,
                    host = %key.host,
                    port = key.port,
                    database = %key.database,
                    "reused idle connection from pool"
                );
                self.checked_out.lock().await.insert(handle, conn);
                // Forget the permit — it stays acquired while connection is checked out.
                permit.forget();
                return Ok(handle);
            }
            // Unhealthy — discard it, decrement total count.
            tracing::debug!(
                host = %key.host,
                port = key.port,
                "discarded unhealthy idle connection"
            );
            let mut pools = self.pools.lock().await;
            if let Some(pool) = pools.get_mut(key) {
                pool.total_count = pool.total_count.saturating_sub(1);
            }
        }

        // No reusable connection — create a new one.
        let backend = self.factory.connect(key, password)?;
        let conn = PooledConnection {
            id: handle,
            created_at: Instant::now(),
            last_used: Instant::now(),
            healthy: true,
            pool_key: key.clone(),
            connection_data: Some(backend),
        };

        {
            let mut pools = self.pools.lock().await;
            let pool = pools
                .entry(key.clone())
                .or_insert_with(|| Pool::new(self.config.max_size));
            pool.total_count += 1;
        }

        tracing::debug!(
            handle = handle,
            host = %key.host,
            port = key.port,
            database = %key.database,
            "created new connection"
        );

        self.checked_out.lock().await.insert(handle, conn);
        // Forget the permit — stays acquired while checked out.
        permit.forget();
        Ok(handle)
    }

    /// Release a connection back to the pool for reuse.
    ///
    /// If the connection is unhealthy, it is destroyed instead of returned.
    pub async fn release(&self, handle: u64) -> Result<(), String> {
        let conn = self
            .checked_out
            .lock()
            .await
            .remove(&handle)
            .ok_or_else(|| format!("invalid handle: {handle}"))?;

        let key = conn.pool_key.clone();

        if !conn.healthy {
            tracing::debug!(
                handle = handle,
                host = %key.host,
                "destroying unhealthy connection on release"
            );
            let mut pools = self.pools.lock().await;
            if let Some(pool) = pools.get_mut(&key) {
                pool.total_count = pool.total_count.saturating_sub(1);
                // Add a permit back since we're destroying the connection.
                pool.semaphore.add_permits(1);
            }
            return Ok(());
        }

        // Return to idle pool.
        let mut pools = self.pools.lock().await;
        let pool = pools
            .entry(key.clone())
            .or_insert_with(|| Pool::new(self.config.max_size));
        pool.idle.push(PooledConnection {
            last_used: Instant::now(),
            ..conn
        });
        // Add a permit back since connection is now idle (available for reuse).
        pool.semaphore.add_permits(1);

        tracing::debug!(
            handle = handle,
            host = %key.host,
            idle_count = pool.idle.len(),
            "returned connection to pool"
        );
        Ok(())
    }

    /// Send data through a checked-out connection.
    pub async fn send(&self, handle: u64, data: &[u8]) -> Result<usize, String> {
        let mut checked_out = self.checked_out.lock().await;
        let conn = checked_out
            .get_mut(&handle)
            .ok_or_else(|| format!("invalid handle: {handle}"))?;

        let backend = conn
            .connection_data
            .as_mut()
            .ok_or_else(|| "connection backend unavailable".to_string())?;

        backend.send(data)
    }

    /// Receive data from a checked-out connection.
    pub async fn recv(&self, handle: u64, max_bytes: usize) -> Result<Vec<u8>, String> {
        let mut checked_out = self.checked_out.lock().await;
        let conn = checked_out
            .get_mut(&handle)
            .ok_or_else(|| format!("invalid handle: {handle}"))?;

        let backend = conn
            .connection_data
            .as_mut()
            .ok_or_else(|| "connection backend unavailable".to_string())?;

        backend.recv(max_bytes)
    }

    /// Reap idle connections that have exceeded the idle timeout.
    pub async fn reap_idle(&self) {
        let mut pools = self.pools.lock().await;
        let idle_timeout = self.config.idle_timeout;

        for (key, pool) in pools.iter_mut() {
            let before = pool.idle.len();
            pool.idle.retain(|conn| {
                conn.last_used.elapsed() < idle_timeout
            });
            let reaped = before - pool.idle.len();
            pool.total_count = pool.total_count.saturating_sub(reaped);
            // Return permits for reaped connections.
            if reaped > 0 {
                pool.semaphore.add_permits(reaped);
                tracing::info!(
                    host = %key.host,
                    port = key.port,
                    database = %key.database,
                    reaped = reaped,
                    remaining_idle = pool.idle.len(),
                    "reaped idle connections"
                );
            }
        }
    }

    /// Health-check all idle connections, marking unhealthy ones for removal.
    pub async fn health_check_idle(&self) {
        let mut pools = self.pools.lock().await;

        for (key, pool) in pools.iter_mut() {
            let before = pool.idle.len();
            pool.idle.retain_mut(|conn| {
                if let Some(backend) = conn.connection_data.as_mut() {
                    let healthy = backend.ping();
                    if !healthy {
                        tracing::info!(
                            host = %key.host,
                            port = key.port,
                            "removed unhealthy idle connection"
                        );
                    }
                    healthy
                } else {
                    false
                }
            });
            let removed = before - pool.idle.len();
            pool.total_count = pool.total_count.saturating_sub(removed);
            if removed > 0 {
                pool.semaphore.add_permits(removed);
            }
        }
    }

    /// Get statistics for a specific pool key.
    pub async fn stats(&self, key: &PoolKey) -> PoolStats {
        let pools = self.pools.lock().await;
        let checked_out = self.checked_out.lock().await;
        let wait_counts = self.wait_counts.lock().await;

        let active = checked_out
            .values()
            .filter(|c| c.pool_key == *key)
            .count();

        let (idle, total) = pools
            .get(key)
            .map(|p| (p.idle.len(), p.total_count))
            .unwrap_or((0, 0));

        let wait_count = wait_counts.get(key).copied().unwrap_or(0);

        PoolStats {
            active,
            idle,
            total,
            wait_count,
        }
    }

    /// Log pool statistics for all pools at `tracing::info` level.
    pub async fn log_stats(&self) {
        let pools = self.pools.lock().await;
        let checked_out = self.checked_out.lock().await;
        let wait_counts = self.wait_counts.lock().await;

        for (key, pool) in pools.iter() {
            let active = checked_out
                .values()
                .filter(|c| c.pool_key == *key)
                .count();
            let wait_count = wait_counts.get(key).copied().unwrap_or(0);

            tracing::info!(
                host = %key.host,
                port = key.port,
                database = %key.database,
                user = %key.user,
                active = active,
                idle = pool.idle.len(),
                total = pool.total_count,
                wait_count = wait_count,
                "pool statistics"
            );
        }
    }
}

// ── Debug impl (cannot auto-derive due to dyn trait) ────────────────

impl std::fmt::Debug for ConnectionPoolManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPoolManager")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    // ── Mock backend and factory ─────────────────────────────────────

    #[derive(Debug)]
    struct MockBackend {
        healthy: Arc<AtomicBool>,
        send_response: Vec<u8>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                healthy: Arc::new(AtomicBool::new(true)),
                send_response: vec![],
            }
        }

        fn with_health(healthy: Arc<AtomicBool>) -> Self {
            Self {
                healthy,
                send_response: vec![],
            }
        }
    }

    impl ConnectionBackend for MockBackend {
        fn send(&mut self, data: &[u8]) -> Result<usize, String> {
            Ok(data.len())
        }

        fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
            let response = if self.send_response.len() > max_bytes {
                self.send_response[..max_bytes].to_vec()
            } else {
                self.send_response.clone()
            };
            Ok(response)
        }

        fn ping(&mut self) -> bool {
            self.healthy.load(Ordering::Relaxed)
        }

        fn close(&mut self) {}
    }

    struct MockFactory {
        connect_count: AtomicU64,
        should_fail: AtomicBool,
    }

    impl MockFactory {
        fn new() -> Self {
            Self {
                connect_count: AtomicU64::new(0),
                should_fail: AtomicBool::new(false),
            }
        }

        fn connects(&self) -> u64 {
            self.connect_count.load(Ordering::Relaxed)
        }
    }

    impl ConnectionFactory for MockFactory {
        fn connect(
            &self,
            _key: &PoolKey,
            _password: Option<&str>,
        ) -> Result<Box<dyn ConnectionBackend>, String> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err("connection refused".to_string());
            }
            self.connect_count.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(MockBackend::new()))
        }
    }

    fn test_key() -> PoolKey {
        PoolKey::new("db.warp.local", 5432, "mydb", "app")
    }

    fn test_config() -> PoolConfig {
        PoolConfig {
            max_size: 3,
            idle_timeout: Duration::from_secs(300),
            health_check_interval: Duration::from_secs(30),
            connect_timeout: Duration::from_millis(200),
            recv_timeout: Duration::from_secs(30),
            use_tls: false,
            verify_certificates: false,
        }
    }

    fn make_manager(config: PoolConfig) -> (ConnectionPoolManager, Arc<MockFactory>) {
        let factory = Arc::new(MockFactory::new());
        let manager = ConnectionPoolManager::new(config, factory.clone());
        (manager, factory)
    }

    // ── PoolKey ─────────────────────────────────────────────────────

    #[test]
    fn pool_key_equality() {
        let a = PoolKey::new("host", 5432, "db", "user");
        let b = PoolKey::new("host", 5432, "db", "user");
        assert_eq!(a, b);
    }

    #[test]
    fn pool_key_different_port() {
        let a = PoolKey::new("host", 5432, "db", "user");
        let b = PoolKey::new("host", 3306, "db", "user");
        assert_ne!(a, b);
    }

    #[test]
    fn pool_key_different_host() {
        let a = PoolKey::new("host1", 5432, "db", "user");
        let b = PoolKey::new("host2", 5432, "db", "user");
        assert_ne!(a, b);
    }

    #[test]
    fn pool_key_different_database() {
        let a = PoolKey::new("host", 5432, "db1", "user");
        let b = PoolKey::new("host", 5432, "db2", "user");
        assert_ne!(a, b);
    }

    #[test]
    fn pool_key_different_user() {
        let a = PoolKey::new("host", 5432, "db", "user1");
        let b = PoolKey::new("host", 5432, "db", "user2");
        assert_ne!(a, b);
    }

    #[test]
    fn pool_key_hashable() {
        let mut map = HashMap::new();
        let key = PoolKey::new("host", 5432, "db", "user");
        map.insert(key.clone(), 42);
        assert_eq!(map.get(&key), Some(&42));
    }

    // ── PoolConfig ──────────────────────────────────────────────────

    #[test]
    fn pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.max_size, 10);
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.health_check_interval, Duration::from_secs(30));
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert!(config.use_tls);
        assert!(config.verify_certificates);
    }

    // ── Checkout: basic ─────────────────────────────────────────────

    #[tokio::test]
    async fn checkout_returns_valid_handle() {
        let (mgr, _) = make_manager(test_config());
        let handle = mgr.checkout(&test_key(), None).await;
        assert!(handle.is_ok());
        assert!(handle.unwrap() > 0);
    }

    #[tokio::test]
    async fn checkout_creates_connection_via_factory() {
        let (mgr, factory) = make_manager(test_config());
        assert_eq!(factory.connects(), 0);
        mgr.checkout(&test_key(), None).await.unwrap();
        assert_eq!(factory.connects(), 1);
    }

    #[tokio::test]
    async fn checkout_multiple_returns_different_handles() {
        let (mgr, _) = make_manager(test_config());
        let h1 = mgr.checkout(&test_key(), None).await.unwrap();
        let h2 = mgr.checkout(&test_key(), None).await.unwrap();
        assert_ne!(h1, h2);
    }

    #[tokio::test]
    async fn checkout_handles_monotonically_increasing() {
        let (mgr, _) = make_manager(test_config());
        let h1 = mgr.checkout(&test_key(), None).await.unwrap();
        let h2 = mgr.checkout(&test_key(), None).await.unwrap();
        let h3 = mgr.checkout(&test_key(), None).await.unwrap();
        assert!(h1 < h2);
        assert!(h2 < h3);
    }

    // ── Checkout: connection reuse ──────────────────────────────────

    #[tokio::test]
    async fn checkout_reuses_released_connection() {
        let (mgr, factory) = make_manager(test_config());
        let key = test_key();

        let h1 = mgr.checkout(&key, None).await.unwrap();
        assert_eq!(factory.connects(), 1);

        mgr.release(h1).await.unwrap();

        let _h2 = mgr.checkout(&key, None).await.unwrap();
        // Should reuse the released connection, not create a new one.
        assert_eq!(factory.connects(), 1);
    }

    #[tokio::test]
    async fn checkout_different_keys_get_separate_pools() {
        let (mgr, factory) = make_manager(test_config());
        let key1 = PoolKey::new("host1", 5432, "db", "user");
        let key2 = PoolKey::new("host2", 5432, "db", "user");

        mgr.checkout(&key1, None).await.unwrap();
        mgr.checkout(&key2, None).await.unwrap();
        // Two separate factories called.
        assert_eq!(factory.connects(), 2);
    }

    // ── Checkout: pool exhaustion ───────────────────────────────────

    #[tokio::test]
    async fn checkout_exhausted_pool_returns_error() {
        let config = PoolConfig {
            max_size: 2,
            connect_timeout: Duration::from_millis(50),
            ..test_config()
        };
        let (mgr, _) = make_manager(config);
        let key = test_key();

        mgr.checkout(&key, None).await.unwrap();
        mgr.checkout(&key, None).await.unwrap();
        // Pool size is 2, both checked out — third should timeout.
        let result = mgr.checkout(&key, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exhausted"));
    }

    #[tokio::test]
    async fn checkout_factory_failure_returns_error() {
        let (mgr, factory) = make_manager(test_config());
        factory.should_fail.store(true, Ordering::Relaxed);

        let result = mgr.checkout(&test_key(), None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("connection refused"));
    }

    // ── Release ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn release_valid_handle_succeeds() {
        let (mgr, _) = make_manager(test_config());
        let handle = mgr.checkout(&test_key(), None).await.unwrap();
        let result = mgr.release(handle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn release_invalid_handle_returns_error() {
        let (mgr, _) = make_manager(test_config());
        let result = mgr.release(999).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid handle"));
    }

    #[tokio::test]
    async fn release_same_handle_twice_fails() {
        let (mgr, _) = make_manager(test_config());
        let handle = mgr.checkout(&test_key(), None).await.unwrap();
        mgr.release(handle).await.unwrap();
        let result = mgr.release(handle).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn release_frees_pool_slot() {
        let config = PoolConfig {
            max_size: 1,
            connect_timeout: Duration::from_millis(100),
            ..test_config()
        };
        let (mgr, _) = make_manager(config);
        let key = test_key();

        let h1 = mgr.checkout(&key, None).await.unwrap();
        // Pool full — can't checkout another.
        assert!(mgr.checkout(&key, None).await.is_err());

        // Release frees the slot.
        mgr.release(h1).await.unwrap();
        // Now we can checkout again.
        let h2 = mgr.checkout(&key, None).await;
        assert!(h2.is_ok());
    }

    // ── Send / Recv ─────────────────────────────────────────────────

    #[tokio::test]
    async fn send_on_valid_handle() {
        let (mgr, _) = make_manager(test_config());
        let handle = mgr.checkout(&test_key(), None).await.unwrap();
        let result = mgr.send(handle, b"SELECT 1").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 8);
    }

    #[tokio::test]
    async fn send_on_invalid_handle_returns_error() {
        let (mgr, _) = make_manager(test_config());
        let result = mgr.send(999, b"data").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid handle"));
    }

    #[tokio::test]
    async fn recv_on_valid_handle() {
        let (mgr, _) = make_manager(test_config());
        let handle = mgr.checkout(&test_key(), None).await.unwrap();
        let result = mgr.recv(handle, 1024).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn recv_on_invalid_handle_returns_error() {
        let (mgr, _) = make_manager(test_config());
        let result = mgr.recv(999, 1024).await;
        assert!(result.is_err());
    }

    // ── Idle reaping ────────────────────────────────────────────────

    #[tokio::test]
    async fn reap_idle_removes_old_connections() {
        let config = PoolConfig {
            idle_timeout: Duration::from_millis(1),
            ..test_config()
        };
        let (mgr, _) = make_manager(config);
        let key = test_key();

        let h = mgr.checkout(&key, None).await.unwrap();
        mgr.release(h).await.unwrap();

        // Wait for idle timeout to expire.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let stats_before = mgr.stats(&key).await;
        assert_eq!(stats_before.idle, 1);

        mgr.reap_idle().await;

        let stats_after = mgr.stats(&key).await;
        assert_eq!(stats_after.idle, 0);
    }

    #[tokio::test]
    async fn reap_idle_keeps_recent_connections() {
        let config = PoolConfig {
            idle_timeout: Duration::from_secs(300),
            ..test_config()
        };
        let (mgr, _) = make_manager(config);
        let key = test_key();

        let h = mgr.checkout(&key, None).await.unwrap();
        mgr.release(h).await.unwrap();

        mgr.reap_idle().await;

        let stats = mgr.stats(&key).await;
        assert_eq!(stats.idle, 1);
    }

    // ── Health checking ─────────────────────────────────────────────

    #[tokio::test]
    async fn health_check_removes_unhealthy_connections() {
        let health_flag = Arc::new(AtomicBool::new(true));
        let health_clone = health_flag.clone();

        // Custom factory that creates connections with controllable health.
        struct HealthFactory {
            healthy: Arc<AtomicBool>,
        }
        impl ConnectionFactory for HealthFactory {
            fn connect(
                &self,
                _key: &PoolKey,
                _password: Option<&str>,
            ) -> Result<Box<dyn ConnectionBackend>, String> {
                Ok(Box::new(MockBackend::with_health(self.healthy.clone())))
            }
        }

        let factory = Arc::new(HealthFactory { healthy: health_clone });
        let mgr = ConnectionPoolManager::new(test_config(), factory);
        let key = test_key();

        let h = mgr.checkout(&key, None).await.unwrap();
        mgr.release(h).await.unwrap();
        assert_eq!(mgr.stats(&key).await.idle, 1);

        // Mark unhealthy.
        health_flag.store(false, Ordering::Relaxed);
        mgr.health_check_idle().await;

        assert_eq!(mgr.stats(&key).await.idle, 0);
    }

    #[tokio::test]
    async fn health_check_keeps_healthy_connections() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();

        let h = mgr.checkout(&key, None).await.unwrap();
        mgr.release(h).await.unwrap();

        mgr.health_check_idle().await;

        assert_eq!(mgr.stats(&key).await.idle, 1);
    }

    // ── Statistics ───────────────────────────────────────────────────

    #[tokio::test]
    async fn stats_empty_pool() {
        let (mgr, _) = make_manager(test_config());
        let stats = mgr.stats(&test_key()).await;
        assert_eq!(stats, PoolStats {
            active: 0,
            idle: 0,
            total: 0,
            wait_count: 0,
        });
    }

    #[tokio::test]
    async fn stats_with_active_connection() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();
        mgr.checkout(&key, None).await.unwrap();

        let stats = mgr.stats(&key).await;
        assert_eq!(stats.active, 1);
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.total, 1);
    }

    #[tokio::test]
    async fn stats_with_idle_connection() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();
        let h = mgr.checkout(&key, None).await.unwrap();
        mgr.release(h).await.unwrap();

        let stats = mgr.stats(&key).await;
        assert_eq!(stats.active, 0);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total, 1);
    }

    #[tokio::test]
    async fn stats_wait_count_increments_on_exhaustion() {
        let config = PoolConfig {
            max_size: 1,
            connect_timeout: Duration::from_millis(10),
            ..test_config()
        };
        let (mgr, _) = make_manager(config);
        let key = test_key();

        mgr.checkout(&key, None).await.unwrap();
        // This should fail and increment wait count.
        let _ = mgr.checkout(&key, None).await;

        let stats = mgr.stats(&key).await;
        assert_eq!(stats.wait_count, 1);
    }

    // ── Full lifecycle ──────────────────────────────────────────────

    #[tokio::test]
    async fn full_lifecycle_checkout_send_recv_release() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();

        let handle = mgr.checkout(&key, None).await.unwrap();
        let sent = mgr.send(handle, b"SELECT 1;").await.unwrap();
        assert_eq!(sent, 9);
        let _data = mgr.recv(handle, 1024).await.unwrap();
        mgr.release(handle).await.unwrap();

        // Handle is no longer valid after release.
        assert!(mgr.send(handle, b"data").await.is_err());
    }

    #[tokio::test]
    async fn multiple_pools_independent() {
        let (mgr, _) = make_manager(test_config());
        let pg_key = PoolKey::new("pg.local", 5432, "app", "user");
        let redis_key = PoolKey::new("redis.local", 6379, "0", "default");

        let h1 = mgr.checkout(&pg_key, None).await.unwrap();
        let h2 = mgr.checkout(&redis_key, None).await.unwrap();

        let pg_stats = mgr.stats(&pg_key).await;
        let redis_stats = mgr.stats(&redis_key).await;
        assert_eq!(pg_stats.active, 1);
        assert_eq!(redis_stats.active, 1);

        mgr.release(h1).await.unwrap();
        mgr.release(h2).await.unwrap();
    }

    #[tokio::test]
    async fn checkout_release_cycle_no_handle_leak() {
        let config = PoolConfig {
            max_size: 2,
            connect_timeout: Duration::from_millis(50),
            ..test_config()
        };
        let (mgr, factory) = make_manager(config);
        let key = test_key();

        // Cycle 100 checkouts through a pool of size 2.
        for _ in 0..100 {
            let h = mgr.checkout(&key, None).await.unwrap();
            mgr.release(h).await.unwrap();
        }

        // Should have created at most 1 real connection (reused each time).
        assert_eq!(factory.connects(), 1);
        let stats = mgr.stats(&key).await;
        assert_eq!(stats.active, 0);
        assert_eq!(stats.idle, 1);
    }
}
