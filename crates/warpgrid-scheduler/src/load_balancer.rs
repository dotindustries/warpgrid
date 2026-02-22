//! Round-robin load balancer.
//!
//! Distributes work across a set of instance indices using an atomic
//! counter. Lock-free and safe for concurrent access.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A round-robin load balancer that selects indices into an instance pool.
///
/// Uses `AtomicUsize` for lock-free concurrent selection. The counter
/// wraps around when it exceeds the number of available instances.
pub struct RoundRobinBalancer {
    counter: AtomicUsize,
}

impl RoundRobinBalancer {
    /// Create a new round-robin balancer.
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
        }
    }

    /// Select the next index, wrapping around `count`.
    ///
    /// Returns `None` if count is zero.
    pub fn next(&self, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        Some(idx % count)
    }

    /// Reset the counter to zero.
    pub fn reset(&self) {
        self.counter.store(0, Ordering::Relaxed);
    }

    /// Current counter value (for diagnostics).
    pub fn current(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }
}

impl Default for RoundRobinBalancer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_cycles_through_indices() {
        let lb = RoundRobinBalancer::new();

        assert_eq!(lb.next(3), Some(0));
        assert_eq!(lb.next(3), Some(1));
        assert_eq!(lb.next(3), Some(2));
        assert_eq!(lb.next(3), Some(0)); // wraps
        assert_eq!(lb.next(3), Some(1));
    }

    #[test]
    fn round_robin_zero_count_returns_none() {
        let lb = RoundRobinBalancer::new();
        assert_eq!(lb.next(0), None);
    }

    #[test]
    fn round_robin_single_instance() {
        let lb = RoundRobinBalancer::new();

        for _ in 0..10 {
            assert_eq!(lb.next(1), Some(0));
        }
    }

    #[test]
    fn round_robin_reset() {
        let lb = RoundRobinBalancer::new();

        lb.next(3);
        lb.next(3);
        assert_eq!(lb.current(), 2);

        lb.reset();
        assert_eq!(lb.current(), 0);
        assert_eq!(lb.next(3), Some(0));
    }

    #[test]
    fn round_robin_adapts_to_changing_pool_size() {
        let lb = RoundRobinBalancer::new();

        // Start with 2 instances.
        assert_eq!(lb.next(2), Some(0));
        assert_eq!(lb.next(2), Some(1));

        // Pool grows to 4 instances.
        assert_eq!(lb.next(4), Some(2));
        assert_eq!(lb.next(4), Some(3));
        assert_eq!(lb.next(4), Some(0)); // wraps at 4

        // Pool shrinks to 2.
        assert_eq!(lb.next(2), Some(1));
    }

    #[test]
    fn round_robin_concurrent_safety() {
        use std::sync::Arc;
        use std::thread;

        let lb = Arc::new(RoundRobinBalancer::new());
        let mut handles = vec![];

        for _ in 0..4 {
            let lb = lb.clone();
            handles.push(thread::spawn(move || {
                let mut results = Vec::new();
                for _ in 0..100 {
                    results.push(lb.next(4).unwrap());
                }
                results
            }));
        }

        let mut all: Vec<usize> = vec![];
        for h in handles {
            all.extend(h.join().unwrap());
        }

        // 400 total selections, counter should be at 400.
        assert_eq!(lb.current(), 400);
        // All indices should be in range 0..4.
        assert!(all.iter().all(|&idx| idx < 4));
    }
}
