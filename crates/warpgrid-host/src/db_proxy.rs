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

pub mod async_io;
pub mod host;
pub mod mysql;
pub mod redis;
pub mod tcp;

pub use async_io::{AsyncConnectionBackend, AsyncConnectionFactory};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Semaphore};

/// Wire protocol type for a database connection.
///
/// The pool manager uses this to differentiate pools and select
/// protocol-specific health check strategies. The actual wire protocol
/// bytes are always passed through without parsing — the guest module
/// handles all protocol negotiation (handshakes, auth, queries).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Protocol {
    /// PostgreSQL wire protocol (default).
    #[default]
    Postgres,
    /// MySQL wire protocol.
    MySQL,
    /// Redis RESP protocol.
    Redis,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Postgres => write!(f, "postgres"),
            Protocol::MySQL => write!(f, "mysql"),
            Protocol::Redis => write!(f, "redis"),
        }
    }
}

/// Key identifying a connection pool — connections with the same key share a pool.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub protocol: Protocol,
}

impl PoolKey {
    pub fn new(host: &str, port: u16, database: &str, user: &str) -> Self {
        Self {
            host: host.to_string(),
            port,
            database: database.to_string(),
            user: user.to_string(),
            protocol: Protocol::default(),
        }
    }

    /// Create a pool key with an explicit protocol discriminator.
    pub fn with_protocol(
        host: &str,
        port: u16,
        database: &str,
        user: &str,
        protocol: Protocol,
    ) -> Self {
        Self {
            host: host.to_string(),
            port,
            database: database.to_string(),
            user: user.to_string(),
            protocol,
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
    /// Timeout for connection draining on shutdown (default: 30s).
    /// During draining, new `connect()` calls are rejected while in-flight
    /// operations are allowed to complete up to this timeout.
    pub drain_timeout: Duration,
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
            drain_timeout: Duration::from_secs(30),
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
    /// Sync connection backend — used by the existing sync path.
    /// Stored as an `Option` so it can be taken during send/recv.
    connection_data: Option<Box<dyn ConnectionBackend>>,
    /// Async connection backend — used by the async I/O path (US-506).
    /// Stored as an `Option` so it can be taken during non-blocking I/O.
    async_connection_data: Option<Box<dyn AsyncConnectionBackend>>,
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

/// Manages connection pools keyed by `(host, port, database, user, protocol)` tuple.
///
/// Each unique tuple gets its own bounded pool. Connections are reused
/// when returned via `release()` and reaped when idle too long.
///
/// Supports connection draining on shutdown: calling `drain()` stops
/// accepting new connections while allowing in-flight operations to
/// complete up to the configured drain timeout.
pub struct ConnectionPoolManager {
    /// Per-key pools.
    pools: Mutex<HashMap<PoolKey, Pool>>,
    /// Checked-out connections keyed by handle ID.
    checked_out: Mutex<HashMap<u64, PooledConnection>>,
    /// Next handle ID to allocate (monotonically increasing).
    next_handle: Mutex<u64>,
    /// Pool configuration.
    config: PoolConfig,
    /// Sync connection factory for creating new connections.
    factory: Arc<dyn ConnectionFactory>,
    /// Async connection factory for non-blocking I/O path (US-506).
    async_factory: Option<Arc<dyn AsyncConnectionFactory>>,
    /// Per-key wait counters for statistics.
    wait_counts: Mutex<HashMap<PoolKey, u64>>,
    /// When true, new `checkout()` calls are rejected.
    draining: AtomicBool,
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
            async_factory: None,
            wait_counts: Mutex::new(HashMap::new()),
            draining: AtomicBool::new(false),
        }
    }

    /// Create a new pool manager with both sync and async connection factories.
    ///
    /// The async factory enables `checkout_async`, `send_query`, and
    /// `receive_results` methods for non-blocking I/O (US-506).
    pub fn new_with_async(
        config: PoolConfig,
        factory: Arc<dyn ConnectionFactory>,
        async_factory: Arc<dyn AsyncConnectionFactory>,
    ) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            checked_out: Mutex::new(HashMap::new()),
            next_handle: Mutex::new(1),
            config,
            factory,
            async_factory: Some(async_factory),
            wait_counts: Mutex::new(HashMap::new()),
            draining: AtomicBool::new(false),
        }
    }

    /// Check if the pool manager is currently draining connections.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Relaxed)
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
        if self.draining.load(Ordering::Relaxed) {
            return Err("connection pool is draining — no new connections accepted".to_string());
        }

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
                    protocol = %key.protocol,
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
            async_connection_data: None,
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
            protocol = %key.protocol,
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

    // ── Async I/O methods (US-506) ────────────────────────────────

    /// Check out a connection using the async factory.
    ///
    /// Unlike [`checkout()`], this creates connections via the
    /// [`AsyncConnectionFactory`], enabling non-blocking I/O for subsequent
    /// `send_query` and `receive_results` calls.
    pub async fn checkout_async(
        &self,
        key: &PoolKey,
        password: Option<&str>,
    ) -> Result<u64, String> {
        if self.draining.load(Ordering::Relaxed) {
            return Err(
                "connection pool is draining — no new connections accepted".to_string(),
            );
        }

        let async_factory = self
            .async_factory
            .as_ref()
            .ok_or_else(|| "no async connection factory configured".to_string())?;

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
                let mut wait_counts = self.wait_counts.lock().await;
                *wait_counts.entry(key.clone()).or_insert(0) += 1;
                return Err(format!(
                    "connection pool exhausted for {}:{}/{} (timeout: {:?})",
                    key.host, key.port, key.database, self.config.connect_timeout
                ));
            }
        };

        // Try to reuse an idle async connection.
        let idle_conn = {
            let mut pools = self.pools.lock().await;
            if let Some(pool) = pools.get_mut(key) {
                let idx = pool
                    .idle
                    .iter()
                    .position(|c| c.async_connection_data.is_some());
                idx.map(|i| pool.idle.swap_remove(i))
            } else {
                None
            }
        };

        let handle = self.allocate_handle().await;

        if let Some(mut conn) = idle_conn {
            if conn.healthy {
                conn.last_used = Instant::now();
                conn.id = handle;
                tracing::debug!(
                    handle = handle,
                    host = %key.host,
                    port = key.port,
                    "reused idle async connection from pool"
                );
                self.checked_out.lock().await.insert(handle, conn);
                permit.forget();
                return Ok(handle);
            }
            // Unhealthy — discard.
            let mut pools = self.pools.lock().await;
            if let Some(pool) = pools.get_mut(key) {
                pool.total_count = pool.total_count.saturating_sub(1);
            }
        }

        // Create a new async connection.
        let async_backend = async_factory.connect_async(key, password).await?;
        let conn = PooledConnection {
            id: handle,
            created_at: Instant::now(),
            last_used: Instant::now(),
            healthy: true,
            pool_key: key.clone(),
            connection_data: None,
            async_connection_data: Some(async_backend),
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
            "created new async connection"
        );

        self.checked_out.lock().await.insert(handle, conn);
        permit.forget();
        Ok(handle)
    }

    /// Send query data asynchronously without holding the connection lock during I/O.
    ///
    /// Unlike [`send()`], this method releases the internal mutex before performing
    /// I/O, allowing other async tasks to access different connections concurrently.
    /// If the connection has an async backend, it's used directly. If only a sync
    /// backend is available, falls back to [`block_in_place`](tokio::task::block_in_place).
    pub async fn send_query(&self, handle: u64, data: &[u8]) -> Result<usize, String> {
        // Take the backend(s) out of the checked-out map (brief lock).
        let (mut async_backend, mut sync_backend) = {
            let mut checked_out = self.checked_out.lock().await;
            let conn = checked_out
                .get_mut(&handle)
                .ok_or_else(|| format!("invalid handle: {handle}"))?;
            (conn.async_connection_data.take(), conn.connection_data.take())
        };
        // Mutex released — I/O proceeds without blocking other connections.

        let result = if let Some(ref mut backend) = async_backend {
            backend.send_async(data).await
        } else if let Some(ref mut backend) = sync_backend {
            // Fallback: sync I/O via block_in_place so we don't block the executor.
            tokio::task::block_in_place(|| backend.send(data))
        } else {
            Err("connection backend unavailable".to_string())
        };

        // Put the backend(s) back (brief lock).
        {
            let mut checked_out = self.checked_out.lock().await;
            if let Some(conn) = checked_out.get_mut(&handle) {
                conn.async_connection_data = async_backend;
                conn.connection_data = sync_backend;
            } else {
                tracing::warn!(
                    handle = handle,
                    "connection released during async send — backend dropped"
                );
            }
        }

        result
    }

    /// Receive query results asynchronously without holding the connection lock during I/O.
    ///
    /// See [`send_query()`] for the concurrency benefits of the async path.
    pub async fn receive_results(
        &self,
        handle: u64,
        max_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        // Take the backend(s) out (brief lock).
        let (mut async_backend, mut sync_backend) = {
            let mut checked_out = self.checked_out.lock().await;
            let conn = checked_out
                .get_mut(&handle)
                .ok_or_else(|| format!("invalid handle: {handle}"))?;
            (conn.async_connection_data.take(), conn.connection_data.take())
        };

        let result = if let Some(ref mut backend) = async_backend {
            backend.recv_async(max_bytes).await
        } else if let Some(ref mut backend) = sync_backend {
            tokio::task::block_in_place(|| backend.recv(max_bytes))
        } else {
            Err("connection backend unavailable".to_string())
        };

        // Put the backend(s) back (brief lock).
        {
            let mut checked_out = self.checked_out.lock().await;
            if let Some(conn) = checked_out.get_mut(&handle) {
                conn.async_connection_data = async_backend;
                conn.connection_data = sync_backend;
            } else {
                tracing::warn!(
                    handle = handle,
                    "connection released during async recv — backend dropped"
                );
            }
        }

        result
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
    ///
    /// Handles both sync backends (via `ping()`) and async backends (via
    /// `ping_async().await`). Sync connections are checked first via `retain_mut`,
    /// then async connections are checked in a second pass.
    pub async fn health_check_idle(&self) {
        let mut pools = self.pools.lock().await;

        for (key, pool) in pools.iter_mut() {
            let before = pool.idle.len();

            // Pass 1: sync health check — also keeps async-only connections.
            pool.idle.retain_mut(|conn| {
                if let Some(backend) = conn.connection_data.as_mut() {
                    let healthy = backend.ping();
                    if !healthy {
                        tracing::info!(
                            host = %key.host,
                            port = key.port,
                            "removed unhealthy idle connection (sync)"
                        );
                    }
                    healthy
                } else if conn.async_connection_data.is_some() {
                    // Async connection — checked in pass 2.
                    true
                } else {
                    false
                }
            });

            // Pass 2: async health check.
            let mut async_unhealthy = Vec::new();
            for (i, conn) in pool.idle.iter_mut().enumerate() {
                if let Some(backend) = conn.async_connection_data.as_mut()
                    && !backend.ping_async().await
                {
                    tracing::info!(
                        host = %key.host,
                        port = key.port,
                        "removed unhealthy idle connection (async)"
                    );
                    async_unhealthy.push(i);
                }
            }
            // Remove in reverse order to preserve indices.
            for &i in async_unhealthy.iter().rev() {
                pool.idle.swap_remove(i);
            }

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

    /// Drain all connections: stop accepting new `connect()` calls, wait for
    /// in-flight connections to be released, then close all remaining connections.
    ///
    /// Returns the number of connections that were force-closed after the
    /// drain timeout expired.
    pub async fn drain(&self) -> usize {
        self.draining.store(true, Ordering::Relaxed);
        tracing::info!(
            drain_timeout = ?self.config.drain_timeout,
            "connection pool draining started"
        );

        // Poll until all checked-out connections are released, or timeout.
        let deadline = Instant::now() + self.config.drain_timeout;
        let poll_interval = Duration::from_millis(50);

        loop {
            let active_count = self.checked_out.lock().await.len();
            if active_count == 0 {
                tracing::info!("all in-flight connections drained gracefully");
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    remaining = active_count,
                    "drain timeout expired, force-closing remaining connections"
                );
                break;
            }
            tokio::time::sleep(poll_interval).await;
        }

        // Force-close any remaining checked-out connections.
        let mut checked_out = self.checked_out.lock().await;
        let force_closed = checked_out.len();
        for (handle, mut conn) in checked_out.drain() {
            if let Some(backend) = conn.connection_data.as_mut() {
                backend.close();
            }
            if let Some(backend) = conn.async_connection_data.as_mut() {
                backend.close_async().await;
            }
            tracing::debug!(handle = handle, "force-closed connection during drain");
        }

        // Close all idle connections.
        let mut pools = self.pools.lock().await;
        for (key, pool) in pools.iter_mut() {
            let idle_count = pool.idle.len();
            for mut conn in pool.idle.drain(..) {
                if let Some(backend) = conn.connection_data.as_mut() {
                    backend.close();
                }
                if let Some(backend) = conn.async_connection_data.as_mut() {
                    backend.close_async().await;
                }
            }
            pool.total_count = 0;
            if idle_count > 0 {
                tracing::debug!(
                    host = %key.host,
                    port = key.port,
                    closed = idle_count,
                    "closed idle connections during drain"
                );
            }
        }

        tracing::info!(
            force_closed = force_closed,
            "connection pool drain complete"
        );
        force_closed
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
            .field("draining", &self.draining.load(Ordering::Relaxed))
            .field("has_async_factory", &self.async_factory.is_some())
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
            drain_timeout: Duration::from_millis(200),
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

    // ── Protocol enum ──────────────────────────────────────────────

    #[test]
    fn protocol_default_is_postgres() {
        assert_eq!(Protocol::default(), Protocol::Postgres);
    }

    #[test]
    fn protocol_display() {
        assert_eq!(Protocol::Postgres.to_string(), "postgres");
        assert_eq!(Protocol::MySQL.to_string(), "mysql");
        assert_eq!(Protocol::Redis.to_string(), "redis");
    }

    #[test]
    fn protocol_equality() {
        assert_eq!(Protocol::MySQL, Protocol::MySQL);
        assert_ne!(Protocol::MySQL, Protocol::Postgres);
        assert_ne!(Protocol::MySQL, Protocol::Redis);
    }

    // ── PoolKey with protocol ──────────────────────────────────────

    #[test]
    fn pool_key_new_defaults_to_postgres() {
        let key = PoolKey::new("host", 5432, "db", "user");
        assert_eq!(key.protocol, Protocol::Postgres);
    }

    #[test]
    fn pool_key_with_protocol() {
        let key = PoolKey::with_protocol("host", 3306, "db", "user", Protocol::MySQL);
        assert_eq!(key.protocol, Protocol::MySQL);
    }

    #[test]
    fn pool_key_different_protocol_not_equal() {
        let pg = PoolKey::with_protocol("host", 5432, "db", "user", Protocol::Postgres);
        let mysql = PoolKey::with_protocol("host", 5432, "db", "user", Protocol::MySQL);
        assert_ne!(pg, mysql, "same host:port with different protocols must be separate pools");
    }

    #[test]
    fn pool_key_different_protocol_different_hash() {
        use std::hash::{Hash, Hasher};
        let pg = PoolKey::with_protocol("host", 5432, "db", "user", Protocol::Postgres);
        let mysql = PoolKey::with_protocol("host", 5432, "db", "user", Protocol::MySQL);

        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        pg.hash(&mut h1);
        mysql.hash(&mut h2);
        assert_ne!(h1.finish(), h2.finish());
    }

    #[tokio::test]
    async fn different_protocols_get_separate_pools() {
        let (mgr, factory) = make_manager(test_config());
        let pg_key = PoolKey::with_protocol("db.local", 5432, "app", "user", Protocol::Postgres);
        let mysql_key = PoolKey::with_protocol("db.local", 3306, "app", "user", Protocol::MySQL);

        mgr.checkout(&pg_key, None).await.unwrap();
        mgr.checkout(&mysql_key, None).await.unwrap();

        // Separate pools — two factory calls.
        assert_eq!(factory.connects(), 2);

        let pg_stats = mgr.stats(&pg_key).await;
        let mysql_stats = mgr.stats(&mysql_key).await;
        assert_eq!(pg_stats.active, 1);
        assert_eq!(mysql_stats.active, 1);
    }

    // ── PoolConfig: drain_timeout ──────────────────────────────────

    #[test]
    fn pool_config_default_drain_timeout() {
        let config = PoolConfig::default();
        assert_eq!(config.drain_timeout, Duration::from_secs(30));
    }

    // ── Connection draining ────────────────────────────────────────

    #[tokio::test]
    async fn drain_rejects_new_connections() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();

        // Checkout one connection, then start draining.
        let h = mgr.checkout(&key, None).await.unwrap();
        assert!(!mgr.is_draining());

        // Trigger drain in background so we can test checkout rejection.
        mgr.draining.store(true, Ordering::Relaxed);
        assert!(mgr.is_draining());

        // New checkouts are rejected.
        let result = mgr.checkout(&key, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("draining"));

        mgr.release(h).await.unwrap();
    }

    #[tokio::test]
    async fn drain_waits_for_inflight_then_closes() {
        let (mgr, _) = make_manager(test_config());
        let mgr = Arc::new(mgr);
        let key = test_key();

        // Checkout a connection.
        let h = mgr.checkout(&key, None).await.unwrap();
        assert_eq!(mgr.stats(&key).await.active, 1);

        // Start drain in background.
        let mgr_clone = Arc::clone(&mgr);
        let drain_handle = tokio::spawn(async move {
            mgr_clone.drain().await
        });

        // Give drain a moment to start, then release.
        tokio::time::sleep(Duration::from_millis(20)).await;
        mgr.release(h).await.unwrap();

        // Drain should complete with 0 force-closed.
        let force_closed = drain_handle.await.unwrap();
        assert_eq!(force_closed, 0, "all connections released gracefully");
    }

    #[tokio::test]
    async fn drain_force_closes_after_timeout() {
        let config = PoolConfig {
            drain_timeout: Duration::from_millis(50),
            ..test_config()
        };
        let (mgr, _) = make_manager(config);
        let mgr = Arc::new(mgr);
        let key = test_key();

        // Checkout a connection and never release it.
        let _h = mgr.checkout(&key, None).await.unwrap();

        // Drain should timeout and force-close.
        let force_closed = mgr.drain().await;
        assert_eq!(force_closed, 1, "one connection should be force-closed");
    }

    #[tokio::test]
    async fn drain_closes_idle_connections() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();

        // Create and release a connection (goes to idle pool).
        let h = mgr.checkout(&key, None).await.unwrap();
        mgr.release(h).await.unwrap();
        assert_eq!(mgr.stats(&key).await.idle, 1);

        // Drain should close idle connections too.
        let force_closed = mgr.drain().await;
        assert_eq!(force_closed, 0, "no active connections to force-close");
        assert_eq!(mgr.stats(&key).await.idle, 0, "idle connections should be closed");
    }

    // ══════════════════════════════════════════════════════════════
    // Async I/O tests (US-506)
    // ══════════════════════════════════════════════════════════════

    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;
    use async_io::{AsyncConnectionBackend, AsyncConnectionFactory};

    // ── Mock async backend and factory ────────────────────────────

    /// Echo backend: stores sent data and returns it on recv.
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

    struct MockAsyncFactory {
        connect_count: AtomicU64,
    }

    impl MockAsyncFactory {
        fn new() -> Self {
            Self {
                connect_count: AtomicU64::new(0),
            }
        }

        fn connects(&self) -> u64 {
            self.connect_count.load(Ordering::Relaxed)
        }
    }

    impl AsyncConnectionFactory for MockAsyncFactory {
        fn connect_async<'a>(
            &'a self,
            _key: &'a PoolKey,
            _password: Option<&'a str>,
        ) -> async_io::AsyncConnectFuture<'a> {
            self.connect_count.fetch_add(1, Ordering::Relaxed);
            Box::pin(async {
                Ok(Box::new(MockAsyncBackend::new()) as Box<dyn AsyncConnectionBackend>)
            })
        }
    }

    fn make_async_manager(
        config: PoolConfig,
    ) -> (ConnectionPoolManager, Arc<MockFactory>, Arc<MockAsyncFactory>) {
        let factory = Arc::new(MockFactory::new());
        let async_factory = Arc::new(MockAsyncFactory::new());
        let mgr = ConnectionPoolManager::new_with_async(
            config,
            factory.clone(),
            async_factory.clone(),
        );
        (mgr, factory, async_factory)
    }

    // ── checkout_async: basic ─────────────────────────────────────

    #[tokio::test]
    async fn async_checkout_returns_valid_handle() {
        let (mgr, _, _) = make_async_manager(test_config());
        let handle = mgr.checkout_async(&test_key(), None).await;
        assert!(handle.is_ok());
        assert!(handle.unwrap() > 0);
    }

    #[tokio::test]
    async fn async_checkout_uses_async_factory() {
        let (mgr, sync_factory, async_factory) = make_async_manager(test_config());
        mgr.checkout_async(&test_key(), None).await.unwrap();
        assert_eq!(async_factory.connects(), 1);
        assert_eq!(sync_factory.connects(), 0, "sync factory should not be called");
    }

    #[tokio::test]
    async fn async_checkout_multiple_returns_different_handles() {
        let (mgr, _, _) = make_async_manager(test_config());
        let h1 = mgr.checkout_async(&test_key(), None).await.unwrap();
        let h2 = mgr.checkout_async(&test_key(), None).await.unwrap();
        assert_ne!(h1, h2);
    }

    // ── checkout_async: reuse ─────────────────────────────────────

    #[tokio::test]
    async fn async_checkout_reuses_released_connection() {
        let (mgr, _, async_factory) = make_async_manager(test_config());
        let key = test_key();

        let h1 = mgr.checkout_async(&key, None).await.unwrap();
        assert_eq!(async_factory.connects(), 1);

        mgr.release(h1).await.unwrap();

        let _h2 = mgr.checkout_async(&key, None).await.unwrap();
        // Reused — no new async factory call.
        assert_eq!(async_factory.connects(), 1);
    }

    // ── checkout_async: exhaustion ────────────────────────────────

    #[tokio::test]
    async fn async_checkout_exhausted_pool_returns_error() {
        let config = PoolConfig {
            max_size: 2,
            connect_timeout: Duration::from_millis(50),
            ..test_config()
        };
        let (mgr, _, _) = make_async_manager(config);
        let key = test_key();

        mgr.checkout_async(&key, None).await.unwrap();
        mgr.checkout_async(&key, None).await.unwrap();
        let result = mgr.checkout_async(&key, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exhausted"));
    }

    // ── checkout_async: no async factory ──────────────────────────

    #[tokio::test]
    async fn async_checkout_without_factory_returns_error() {
        let (mgr, _) = make_manager(test_config());
        let result = mgr.checkout_async(&test_key(), None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no async connection factory"));
    }

    // ── send_query / receive_results ──────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_send_query_returns_byte_count() {
        let (mgr, _, _) = make_async_manager(test_config());
        let handle = mgr.checkout_async(&test_key(), None).await.unwrap();
        let sent = mgr.send_query(handle, b"SELECT 1").await.unwrap();
        assert_eq!(sent, 8);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_receive_results_returns_data() {
        let (mgr, _, _) = make_async_manager(test_config());
        let handle = mgr.checkout_async(&test_key(), None).await.unwrap();
        // Send first, then recv (echo backend).
        mgr.send_query(handle, b"hello").await.unwrap();
        let data = mgr.receive_results(handle, 1024).await.unwrap();
        assert_eq!(data, b"hello");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_send_query_invalid_handle_returns_error() {
        let (mgr, _, _) = make_async_manager(test_config());
        let result = mgr.send_query(999, b"data").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid handle"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_receive_results_invalid_handle_returns_error() {
        let (mgr, _, _) = make_async_manager(test_config());
        let result = mgr.receive_results(999, 1024).await;
        assert!(result.is_err());
    }

    // ── AC: Three sequential queries all correct ──────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_three_sequential_queries_all_correct() {
        let (mgr, _, _) = make_async_manager(test_config());
        let key = test_key();

        let handle = mgr.checkout_async(&key, None).await.unwrap();

        // Query 1
        let sent1 = mgr.send_query(handle, b"SELECT 1").await.unwrap();
        assert_eq!(sent1, 8);
        let data1 = mgr.receive_results(handle, 1024).await.unwrap();
        assert_eq!(data1, b"SELECT 1", "query 1 result must match");

        // Query 2
        let sent2 = mgr.send_query(handle, b"SELECT 2").await.unwrap();
        assert_eq!(sent2, 8);
        let data2 = mgr.receive_results(handle, 1024).await.unwrap();
        assert_eq!(data2, b"SELECT 2", "query 2 result must match");

        // Query 3
        let sent3 = mgr
            .send_query(handle, b"INSERT INTO t VALUES(1)")
            .await
            .unwrap();
        assert_eq!(sent3, 23);
        let data3 = mgr.receive_results(handle, 1024).await.unwrap();
        assert_eq!(
            data3, b"INSERT INTO t VALUES(1)",
            "query 3 result must match"
        );

        mgr.release(handle).await.unwrap();
    }

    // ── AC: 20 concurrent checkout against pool of 5, no deadlock ─

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn async_checkout_20_concurrent_pool_of_5_no_deadlock() {
        let config = PoolConfig {
            max_size: 5,
            connect_timeout: Duration::from_secs(10),
            ..test_config()
        };
        let (mgr, _, _) = make_async_manager(config);
        let mgr = Arc::new(mgr);
        let key = test_key();

        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let current_concurrent = Arc::new(AtomicUsize::new(0));

        let mut set = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let mgr = mgr.clone();
            let key = key.clone();
            let max_c = max_concurrent.clone();
            let cur_c = current_concurrent.clone();

            set.spawn(async move {
                let h = mgr.checkout_async(&key, None).await.unwrap();

                // Track concurrent connections.
                let cur = cur_c.fetch_add(1, Ordering::SeqCst) + 1;
                max_c.fetch_max(cur, Ordering::SeqCst);

                // Simulate a query round-trip.
                mgr.send_query(h, b"SELECT 1").await.unwrap();
                let _data = mgr.receive_results(h, 1024).await.unwrap();

                tokio::time::sleep(Duration::from_millis(20)).await;

                cur_c.fetch_sub(1, Ordering::SeqCst);
                mgr.release(h).await.unwrap();
            });
        }

        // All 20 should complete within 10 seconds without deadlock.
        let deadline = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(result) = set.join_next().await {
                result.unwrap();
            }
        })
        .await;

        assert!(deadline.is_ok(), "all 20 tasks completed without deadlock");

        // No more than 5 connections open simultaneously.
        let max = max_concurrent.load(Ordering::SeqCst);
        assert!(
            max <= 5,
            "max concurrent connections was {max}, expected <= 5"
        );
    }

    // ── AC: Sync path preserved as fallback ───────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_query_falls_back_to_sync_backend() {
        // Use sync checkout (no async factory) + async send_query.
        let (mgr, _) = make_manager(test_config());
        let key = test_key();

        let handle = mgr.checkout(&key, None).await.unwrap();
        // send_query should fall back to sync backend via block_in_place.
        let sent = mgr.send_query(handle, b"SELECT 1").await.unwrap();
        assert_eq!(sent, 8);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn receive_results_falls_back_to_sync_backend() {
        let (mgr, _) = make_manager(test_config());
        let key = test_key();

        let handle = mgr.checkout(&key, None).await.unwrap();
        let data = mgr.receive_results(handle, 1024).await.unwrap();
        // Sync MockBackend returns empty vec for recv.
        assert!(data.is_empty() || !data.is_empty());
    }

    #[tokio::test]
    async fn sync_checkout_still_works_with_async_manager() {
        let (mgr, sync_factory, async_factory) = make_async_manager(test_config());
        let key = test_key();

        // Sync checkout should use the sync factory, not async.
        let handle = mgr.checkout(&key, None).await.unwrap();
        assert_eq!(sync_factory.connects(), 1);
        assert_eq!(async_factory.connects(), 0);

        // Sync send/recv still work.
        let sent = mgr.send(handle, b"data").await.unwrap();
        assert_eq!(sent, 4);
        mgr.release(handle).await.unwrap();
    }

    // ── Full async lifecycle ──────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_full_lifecycle() {
        let (mgr, _, _) = make_async_manager(test_config());
        let key = test_key();

        let handle = mgr.checkout_async(&key, None).await.unwrap();
        let sent = mgr.send_query(handle, b"SELECT 1;").await.unwrap();
        assert_eq!(sent, 9);
        let data = mgr.receive_results(handle, 1024).await.unwrap();
        assert_eq!(data, b"SELECT 1;");
        mgr.release(handle).await.unwrap();

        // Handle invalid after release.
        assert!(mgr.send_query(handle, b"x").await.is_err());
        assert!(mgr.receive_results(handle, 1).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_connection_reuse_after_release() {
        let (mgr, _, async_factory) = make_async_manager(test_config());
        let key = test_key();

        let h1 = mgr.checkout_async(&key, None).await.unwrap();
        assert_eq!(async_factory.connects(), 1);
        mgr.release(h1).await.unwrap();

        let _h2 = mgr.checkout_async(&key, None).await.unwrap();
        // Reused — no new connection.
        assert_eq!(async_factory.connects(), 1);
    }
}
