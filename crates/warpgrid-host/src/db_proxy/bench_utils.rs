//! Public mock backends for benchmarks and integration tests.
//!
//! These types are gated behind the `bench` feature flag to avoid polluting the
//! public API during normal builds. Criterion benchmarks compile as external
//! crates and need public access to mock factories and backends with
//! configurable latency injection.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::async_io::{AsyncConnectFuture, AsyncConnectionBackend, AsyncConnectionFactory};
use super::{ConnectionBackend, ConnectionFactory, PoolKey};

// ── Sync mock backend ───────────────────────────────────────────────

/// Mock sync backend with configurable I/O latency.
///
/// Each `send`/`recv` call sleeps for the configured duration using
/// `std::thread::sleep`, simulating blocking database I/O.
#[derive(Debug)]
pub struct MockBackend {
    latency: Duration,
}

impl MockBackend {
    /// Create a mock backend with zero latency.
    pub fn new() -> Self {
        Self {
            latency: Duration::ZERO,
        }
    }

    /// Create a mock backend that sleeps `latency` on every send/recv.
    pub fn with_latency(latency: Duration) -> Self {
        Self { latency }
    }
}

impl ConnectionBackend for MockBackend {
    fn send(&mut self, data: &[u8]) -> Result<usize, String> {
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }
        Ok(data.len())
    }

    fn recv(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }
        Ok(vec![0u8; max_bytes.min(64)])
    }

    fn ping(&mut self) -> bool {
        true
    }

    fn close(&mut self) {}
}

// ── Sync mock factory ───────────────────────────────────────────────

/// Factory that produces [`MockBackend`] instances with a fixed latency.
pub struct MockFactory {
    latency: Duration,
    connect_count: AtomicU64,
}

impl Default for MockFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl MockFactory {
    /// Create a factory that produces zero-latency backends.
    pub fn new() -> Self {
        Self {
            latency: Duration::ZERO,
            connect_count: AtomicU64::new(0),
        }
    }

    /// Create a factory whose backends sleep `latency` per I/O call.
    pub fn with_latency(latency: Duration) -> Self {
        Self {
            latency,
            connect_count: AtomicU64::new(0),
        }
    }

    /// Number of connections created so far.
    pub fn connects(&self) -> u64 {
        self.connect_count.load(Ordering::Relaxed)
    }
}

impl ConnectionFactory for MockFactory {
    fn connect(
        &self,
        _key: &PoolKey,
        _password: Option<&str>,
    ) -> Result<Box<dyn ConnectionBackend>, String> {
        self.connect_count.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(MockBackend::with_latency(self.latency)))
    }
}

// ── Async mock backend ──────────────────────────────────────────────

/// Mock async backend with configurable I/O latency.
///
/// Each `send_async`/`recv_async` call awaits `tokio::time::sleep` for the
/// configured duration, simulating non-blocking database I/O.
#[derive(Debug)]
pub struct MockAsyncBackend {
    latency: Duration,
}

impl Default for MockAsyncBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockAsyncBackend {
    /// Create a mock async backend with zero latency.
    pub fn new() -> Self {
        Self {
            latency: Duration::ZERO,
        }
    }

    /// Create a mock async backend that sleeps `latency` on every send/recv.
    pub fn with_latency(latency: Duration) -> Self {
        Self { latency }
    }
}

impl AsyncConnectionBackend for MockAsyncBackend {
    fn send_async<'a>(
        &'a mut self,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
        Box::pin(async move {
            if !self.latency.is_zero() {
                tokio::time::sleep(self.latency).await;
            }
            Ok(data.len())
        })
    }

    fn recv_async<'a>(
        &'a mut self,
        max_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>> {
        Box::pin(async move {
            if !self.latency.is_zero() {
                tokio::time::sleep(self.latency).await;
            }
            Ok(vec![0u8; max_bytes.min(64)])
        })
    }

    fn ping_async(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async { true })
    }

    fn close_async(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

// ── Async mock factory ──────────────────────────────────────────────

/// Factory that produces [`MockAsyncBackend`] instances with a fixed latency.
pub struct MockAsyncFactory {
    latency: Duration,
    connect_count: AtomicU64,
}

impl Default for MockAsyncFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl MockAsyncFactory {
    /// Create a factory that produces zero-latency async backends.
    pub fn new() -> Self {
        Self {
            latency: Duration::ZERO,
            connect_count: AtomicU64::new(0),
        }
    }

    /// Create a factory whose backends sleep `latency` per I/O call.
    pub fn with_latency(latency: Duration) -> Self {
        Self {
            latency,
            connect_count: AtomicU64::new(0),
        }
    }

    /// Number of connections created so far.
    pub fn connects(&self) -> u64 {
        self.connect_count.load(Ordering::Relaxed)
    }
}

impl AsyncConnectionFactory for MockAsyncFactory {
    fn connect_async<'a>(
        &'a self,
        _key: &'a PoolKey,
        _password: Option<&'a str>,
    ) -> AsyncConnectFuture<'a> {
        self.connect_count.fetch_add(1, Ordering::Relaxed);
        let latency = self.latency;
        Box::pin(async move {
            Ok(Box::new(MockAsyncBackend::with_latency(latency))
                as Box<dyn AsyncConnectionBackend>)
        })
    }
}
