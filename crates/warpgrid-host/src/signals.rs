//! Signal delivery shim.
//!
//! Provides lifecycle signal handling (SIGTERM, SIGHUP, SIGINT) for Wasm modules
//! via a non-blocking poll interface.
//!
//! # Architecture
//!
//! [`SignalQueue`] is a bounded queue of signal types with interest registration.
//! Only signals matching registered interest are enqueued. When the queue is full,
//! the oldest undelivered signal is dropped.
//!
//! The [`host`] submodule provides the WIT `Host` trait implementation that bridges
//! guest calls to the signal queue.

pub mod host;

use std::collections::VecDeque;

use crate::bindings::warpgrid::shim::signals::SignalType;

/// Default maximum number of signals that can be queued.
const DEFAULT_CAPACITY: usize = 16;

/// Maps a [`SignalType`] to an index for the interest bitfield.
fn signal_index(signal: &SignalType) -> usize {
    match signal {
        SignalType::Terminate => 0,
        SignalType::Hangup => 1,
        SignalType::Interrupt => 2,
    }
}

/// Bounded queue of lifecycle signals with interest-based filtering.
///
/// Guests register interest in specific signal types via [`register_interest`].
/// The host delivers signals via [`deliver`], which enqueues only signals
/// matching registered interest. Guests poll the queue via [`poll`].
///
/// When the queue reaches capacity, the oldest signal is dropped to make room.
///
/// [`register_interest`]: SignalQueue::register_interest
/// [`deliver`]: SignalQueue::deliver
/// [`poll`]: SignalQueue::poll
pub struct SignalQueue {
    /// Interest bitfield: `[terminate, hangup, interrupt]`.
    interest: [bool; 3],
    /// FIFO queue of pending signals.
    queue: VecDeque<SignalType>,
    /// Maximum number of signals the queue can hold.
    capacity: usize,
}

impl SignalQueue {
    /// Create a new empty signal queue with default capacity (16).
    pub fn new() -> Self {
        Self {
            interest: [false; 3],
            queue: VecDeque::with_capacity(DEFAULT_CAPACITY),
            capacity: DEFAULT_CAPACITY,
        }
    }

    /// Create a new empty signal queue with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            interest: [false; 3],
            queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Register interest in receiving the given signal type.
    ///
    /// After registration, signals of this type delivered via [`deliver`]
    /// will be enqueued.
    ///
    /// [`deliver`]: SignalQueue::deliver
    pub fn register_interest(&mut self, signal: SignalType) {
        let idx = signal_index(&signal);
        self.interest[idx] = true;
        tracing::debug!(signal = ?signal, "registered interest in signal");
    }

    /// Check whether the queue has registered interest in the given signal type.
    pub fn has_interest(&self, signal: &SignalType) -> bool {
        self.interest[signal_index(signal)]
    }

    /// Deliver a signal to the queue.
    ///
    /// Returns `true` if the signal was enqueued, `false` if it was ignored
    /// (no interest registered for this signal type).
    ///
    /// If the queue is full, the oldest signal is dropped and a warning is logged.
    pub fn deliver(&mut self, signal: SignalType) -> bool {
        if !self.has_interest(&signal) {
            tracing::debug!(signal = ?signal, "signal ignored — no interest registered");
            return false;
        }

        if self.queue.len() >= self.capacity {
            let dropped = self.queue.pop_front();
            tracing::warn!(
                dropped = ?dropped,
                signal = ?signal,
                queue_capacity = self.capacity,
                "signal queue full — dropped oldest signal"
            );
        }

        self.queue.push_back(signal);
        tracing::debug!(signal = ?signal, queue_len = self.queue.len(), "signal enqueued");
        true
    }

    /// Dequeue the oldest pending signal.
    ///
    /// Returns `None` if the queue is empty.
    pub fn poll(&mut self) -> Option<SignalType> {
        let signal = self.queue.pop_front();
        if let Some(ref s) = signal {
            tracing::debug!(signal = ?s, remaining = self.queue.len(), "signal dequeued");
        }
        signal
    }

    /// Number of signals currently queued.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl Default for SignalQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ────────────────────────────────────────────────

    #[test]
    fn new_queue_is_empty() {
        let queue = SignalQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn default_capacity_is_16() {
        let queue = SignalQueue::new();
        assert_eq!(queue.capacity, 16);
    }

    #[test]
    fn custom_capacity() {
        let queue = SignalQueue::with_capacity(4);
        assert_eq!(queue.capacity, 4);
    }

    // ── Interest registration ──────────────────────────────────────

    #[test]
    fn no_interest_by_default() {
        let queue = SignalQueue::new();
        assert!(!queue.has_interest(&SignalType::Terminate));
        assert!(!queue.has_interest(&SignalType::Hangup));
        assert!(!queue.has_interest(&SignalType::Interrupt));
    }

    #[test]
    fn register_single_interest() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        assert!(queue.has_interest(&SignalType::Terminate));
        assert!(!queue.has_interest(&SignalType::Hangup));
        assert!(!queue.has_interest(&SignalType::Interrupt));
    }

    #[test]
    fn register_multiple_interests() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);
        assert!(queue.has_interest(&SignalType::Terminate));
        assert!(queue.has_interest(&SignalType::Hangup));
        assert!(!queue.has_interest(&SignalType::Interrupt));
    }

    #[test]
    fn register_all_interests() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);
        queue.register_interest(SignalType::Interrupt);
        assert!(queue.has_interest(&SignalType::Terminate));
        assert!(queue.has_interest(&SignalType::Hangup));
        assert!(queue.has_interest(&SignalType::Interrupt));
    }

    #[test]
    fn register_same_interest_twice_is_idempotent() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Terminate);
        assert!(queue.has_interest(&SignalType::Terminate));
    }

    // ── Deliver ────────────────────────────────────────────────────

    #[test]
    fn deliver_with_interest_enqueues() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        assert!(queue.deliver(SignalType::Terminate));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn deliver_without_interest_ignored() {
        let mut queue = SignalQueue::new();
        assert!(!queue.deliver(SignalType::Terminate));
        assert!(queue.is_empty());
    }

    #[test]
    fn deliver_unregistered_type_ignored() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Hangup);
        // Deliver a terminate — no interest in terminate
        assert!(!queue.deliver(SignalType::Terminate));
        assert!(queue.is_empty());
    }

    #[test]
    fn deliver_multiple_signals() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);

        queue.deliver(SignalType::Terminate);
        queue.deliver(SignalType::Hangup);
        queue.deliver(SignalType::Terminate);

        assert_eq!(queue.len(), 3);
    }

    // ── Poll ───────────────────────────────────────────────────────

    #[test]
    fn poll_empty_returns_none() {
        let mut queue = SignalQueue::new();
        assert!(queue.poll().is_none());
    }

    #[test]
    fn poll_returns_oldest_first() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);

        queue.deliver(SignalType::Terminate);
        queue.deliver(SignalType::Hangup);

        let first = queue.poll();
        assert!(matches!(first, Some(SignalType::Terminate)));
        let second = queue.poll();
        assert!(matches!(second, Some(SignalType::Hangup)));
    }

    #[test]
    fn poll_drains_queue() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Interrupt);

        queue.deliver(SignalType::Interrupt);
        queue.deliver(SignalType::Interrupt);

        assert!(queue.poll().is_some());
        assert!(queue.poll().is_some());
        assert!(queue.poll().is_none());
        assert!(queue.is_empty());
    }

    // ── Queue bounding ─────────────────────────────────────────────

    #[test]
    fn queue_drops_oldest_when_full() {
        let mut queue = SignalQueue::with_capacity(2);
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);
        queue.register_interest(SignalType::Interrupt);

        queue.deliver(SignalType::Terminate);  // [Terminate]
        queue.deliver(SignalType::Hangup);     // [Terminate, Hangup]
        queue.deliver(SignalType::Interrupt);  // [Hangup, Interrupt] — Terminate dropped

        assert_eq!(queue.len(), 2);
        // Oldest remaining is Hangup
        assert!(matches!(queue.poll(), Some(SignalType::Hangup)));
        assert!(matches!(queue.poll(), Some(SignalType::Interrupt)));
    }

    #[test]
    fn queue_bounding_with_default_capacity() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);

        // Fill to capacity (16)
        for _ in 0..16 {
            queue.deliver(SignalType::Terminate);
        }
        assert_eq!(queue.len(), 16);

        // 17th delivery drops oldest
        queue.deliver(SignalType::Terminate);
        assert_eq!(queue.len(), 16);
    }

    #[test]
    fn overfill_preserves_most_recent() {
        let mut queue = SignalQueue::with_capacity(3);
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);
        queue.register_interest(SignalType::Interrupt);

        // Deliver 5 signals into a capacity-3 queue
        queue.deliver(SignalType::Terminate);  // dropped
        queue.deliver(SignalType::Hangup);     // dropped
        queue.deliver(SignalType::Interrupt);
        queue.deliver(SignalType::Terminate);
        queue.deliver(SignalType::Hangup);

        // Only the 3 most recent should remain
        assert_eq!(queue.len(), 3);
        assert!(matches!(queue.poll(), Some(SignalType::Interrupt)));
        assert!(matches!(queue.poll(), Some(SignalType::Terminate)));
        assert!(matches!(queue.poll(), Some(SignalType::Hangup)));
    }

    #[test]
    fn deliver_20_signals_only_16_remain() {
        let mut queue = SignalQueue::new(); // capacity 16
        queue.register_interest(SignalType::Terminate);

        for _ in 0..20 {
            queue.deliver(SignalType::Terminate);
        }

        assert_eq!(queue.len(), 16);

        // All 16 should be retrievable
        let mut count = 0;
        while queue.poll().is_some() {
            count += 1;
        }
        assert_eq!(count, 16);
    }

    // ── Signal filtering ───────────────────────────────────────────

    #[test]
    fn only_interested_signals_enqueued() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Hangup);

        // Deliver all three types
        queue.deliver(SignalType::Terminate);
        queue.deliver(SignalType::Hangup);
        queue.deliver(SignalType::Interrupt);

        // Only hangup should be in queue
        assert_eq!(queue.len(), 1);
        assert!(matches!(queue.poll(), Some(SignalType::Hangup)));
    }

    #[test]
    fn mixed_interest_filtering() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Interrupt);
        // No interest in Hangup

        queue.deliver(SignalType::Terminate);
        queue.deliver(SignalType::Hangup);    // ignored
        queue.deliver(SignalType::Interrupt);
        queue.deliver(SignalType::Hangup);    // ignored
        queue.deliver(SignalType::Terminate);

        assert_eq!(queue.len(), 3);
        assert!(matches!(queue.poll(), Some(SignalType::Terminate)));
        assert!(matches!(queue.poll(), Some(SignalType::Interrupt)));
        assert!(matches!(queue.poll(), Some(SignalType::Terminate)));
    }

    // ── Full lifecycle ─────────────────────────────────────────────

    #[test]
    fn full_lifecycle_register_deliver_poll() {
        let mut queue = SignalQueue::new();

        // Initially empty
        assert!(queue.poll().is_none());

        // Register interest
        queue.register_interest(SignalType::Terminate);

        // Deliver
        assert!(queue.deliver(SignalType::Terminate));

        // Poll
        let signal = queue.poll();
        assert!(matches!(signal, Some(SignalType::Terminate)));

        // Queue is empty again
        assert!(queue.poll().is_none());
        assert!(queue.is_empty());
    }

    #[test]
    fn interleaved_deliver_and_poll() {
        let mut queue = SignalQueue::new();
        queue.register_interest(SignalType::Terminate);
        queue.register_interest(SignalType::Hangup);

        queue.deliver(SignalType::Terminate);
        assert!(matches!(queue.poll(), Some(SignalType::Terminate)));

        queue.deliver(SignalType::Hangup);
        queue.deliver(SignalType::Terminate);
        assert!(matches!(queue.poll(), Some(SignalType::Hangup)));
        assert!(matches!(queue.poll(), Some(SignalType::Terminate)));
        assert!(queue.poll().is_none());
    }
}
