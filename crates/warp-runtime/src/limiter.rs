//! WarpGridLimiter — memory and table enforcement for Wasm instances.
//!
//! Implements `wasmtime::ResourceLimiter` to cap memory growth and table
//! expansion per instance. This prevents a single guest from consuming
//! unbounded host resources.

use wasmtime::ResourceLimiter;

/// Per-instance resource limiter.
///
/// Tracks current memory and table usage against configured limits.
/// When a guest attempts to grow beyond limits, the allocation is denied.
pub struct WarpGridLimiter {
    /// Maximum memory in bytes this instance may use.
    memory_limit: usize,
    /// Maximum table elements this instance may allocate.
    table_limit: u32,
    /// Current memory usage (tracked by wasmtime callbacks).
    memory_used: usize,
}

impl WarpGridLimiter {
    /// Create a new limiter with the given memory limit (bytes) and table element limit.
    pub fn new(memory_limit: usize, table_limit: u32) -> Self {
        Self {
            memory_limit,
            table_limit,
            memory_used: 0,
        }
    }

    /// Create a limiter with sensible defaults (64 MiB memory, 10k table entries).
    pub fn with_defaults() -> Self {
        Self::new(64 * 1024 * 1024, 10_000)
    }

    /// Current memory usage in bytes.
    pub fn memory_used(&self) -> usize {
        self.memory_used
    }

    /// Configured memory limit in bytes.
    pub fn memory_limit(&self) -> usize {
        self.memory_limit
    }
}

impl ResourceLimiter for WarpGridLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.memory_used = desired;
        if desired > self.memory_limit {
            tracing::warn!(
                current,
                desired,
                limit = self.memory_limit,
                "memory growth denied"
            );
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        if desired > self.table_limit as usize {
            tracing::warn!(
                current,
                desired,
                limit = self.table_limit,
                "table growth denied"
            );
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_growth_within_limit() {
        let mut limiter = WarpGridLimiter::new(1024, 100);
        assert!(limiter.memory_growing(0, 512, None).unwrap());
        assert_eq!(limiter.memory_used(), 512);
    }

    #[test]
    fn denies_growth_beyond_limit() {
        let mut limiter = WarpGridLimiter::new(1024, 100);
        assert!(!limiter.memory_growing(0, 2048, None).unwrap());
    }

    #[test]
    fn allows_table_within_limit() {
        let mut limiter = WarpGridLimiter::new(1024, 100);
        assert!(limiter.table_growing(0, 50, None).unwrap());
    }

    #[test]
    fn denies_table_beyond_limit() {
        let mut limiter = WarpGridLimiter::new(1024, 100);
        assert!(!limiter.table_growing(0, 200, None).unwrap());
    }

    #[test]
    fn defaults_are_reasonable() {
        let limiter = WarpGridLimiter::with_defaults();
        assert_eq!(limiter.memory_limit(), 64 * 1024 * 1024);
    }

    #[test]
    fn tracks_memory_usage() {
        let mut limiter = WarpGridLimiter::new(1024 * 1024, 100);
        limiter.memory_growing(0, 1024, None).unwrap();
        assert_eq!(limiter.memory_used(), 1024);
        limiter.memory_growing(1024, 4096, None).unwrap();
        assert_eq!(limiter.memory_used(), 4096);
    }
}
