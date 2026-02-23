//! Signal handling host functions.
//!
//! Implements the `warpgrid:shim/signals` [`Host`] trait, delegating signal
//! registration and polling to the [`SignalQueue`].
//!
//! # Signal delivery flow
//!
//! ```text
//! Orchestrator calls SignalsHost::deliver_signal(signal_type)
//!   → SignalQueue checks interest
//!     → Interest registered → enqueue (drop oldest if full)
//!     → No interest         → silently ignored
//!
//! Guest calls poll_signal()
//!   → SignalsHost delegates to SignalQueue::poll()
//!     → Queue non-empty → Some(signal_type)
//!     → Queue empty     → None
//! ```

use crate::bindings::warpgrid::shim::signals::{Host, SignalType};
use super::SignalQueue;

/// Host-side implementation of the `warpgrid:shim/signals` interface.
///
/// Wraps a [`SignalQueue`] and provides both the guest-facing WIT Host trait
/// and a host-side [`deliver_signal`] method for the orchestrator.
///
/// Each `SignalsHost` instance corresponds to a single Wasm module instance.
/// The orchestrator routes signals to the correct instance externally
/// (see `WarpGridEngine` in US-121).
///
/// [`deliver_signal`]: SignalsHost::deliver_signal
pub struct SignalsHost {
    queue: SignalQueue,
}

impl SignalsHost {
    /// Create a new `SignalsHost` with a default-capacity signal queue.
    pub fn new() -> Self {
        Self {
            queue: SignalQueue::new(),
        }
    }

    /// Create a new `SignalsHost` wrapping the given signal queue.
    pub fn with_queue(queue: SignalQueue) -> Self {
        Self { queue }
    }

    /// Host-side API: deliver a signal to this module instance.
    ///
    /// Returns `true` if the signal was enqueued (interest was registered),
    /// `false` if the signal was silently ignored.
    ///
    /// If the queue is full, the oldest signal is dropped to make room.
    pub fn deliver_signal(&mut self, signal: SignalType) -> bool {
        self.queue.deliver(signal)
    }
}

impl Default for SignalsHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for SignalsHost {
    fn on_signal(&mut self, signal: SignalType) -> Result<(), String> {
        tracing::debug!(signal = ?signal, "signals intercept: on_signal (register interest)");
        self.queue.register_interest(signal);
        Ok(())
    }

    fn poll_signal(&mut self) -> Option<SignalType> {
        tracing::debug!("signals intercept: poll_signal");
        self.queue.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ────────────────────────────────────────────────

    #[test]
    fn new_host_has_empty_queue() {
        let mut host = SignalsHost::new();
        assert!(host.poll_signal().is_none());
    }

    // ── on_signal (register interest) ──────────────────────────────

    #[test]
    fn on_signal_registers_interest() {
        let mut host = SignalsHost::new();
        let result = host.on_signal(SignalType::Terminate);
        assert!(result.is_ok());

        // Now delivering a terminate signal should enqueue
        assert!(host.deliver_signal(SignalType::Terminate));
    }

    #[test]
    fn on_signal_for_multiple_types() {
        let mut host = SignalsHost::new();
        assert!(host.on_signal(SignalType::Terminate).is_ok());
        assert!(host.on_signal(SignalType::Hangup).is_ok());
        assert!(host.on_signal(SignalType::Interrupt).is_ok());

        // All three types should be enqueued
        assert!(host.deliver_signal(SignalType::Terminate));
        assert!(host.deliver_signal(SignalType::Hangup));
        assert!(host.deliver_signal(SignalType::Interrupt));
    }

    // ── poll_signal ────────────────────────────────────────────────

    #[test]
    fn poll_signal_empty_returns_none() {
        let mut host = SignalsHost::new();
        assert!(host.poll_signal().is_none());
    }

    #[test]
    fn poll_signal_returns_delivered_signal() {
        let mut host = SignalsHost::new();
        host.on_signal(SignalType::Terminate).unwrap();
        host.deliver_signal(SignalType::Terminate);

        let polled = host.poll_signal();
        assert!(matches!(polled, Some(SignalType::Terminate)));
    }

    #[test]
    fn poll_signal_returns_fifo_order() {
        let mut host = SignalsHost::new();
        host.on_signal(SignalType::Terminate).unwrap();
        host.on_signal(SignalType::Hangup).unwrap();

        host.deliver_signal(SignalType::Terminate);
        host.deliver_signal(SignalType::Hangup);

        assert!(matches!(host.poll_signal(), Some(SignalType::Terminate)));
        assert!(matches!(host.poll_signal(), Some(SignalType::Hangup)));
        assert!(host.poll_signal().is_none());
    }

    // ── deliver_signal (host-side API) ─────────────────────────────

    #[test]
    fn deliver_signal_without_interest_returns_false() {
        let mut host = SignalsHost::new();
        // No interest registered
        assert!(!host.deliver_signal(SignalType::Terminate));
        assert!(host.poll_signal().is_none());
    }

    #[test]
    fn deliver_signal_with_interest_returns_true() {
        let mut host = SignalsHost::new();
        host.on_signal(SignalType::Hangup).unwrap();
        assert!(host.deliver_signal(SignalType::Hangup));
    }

    // ── Signal filtering through Host trait ─────────────────────────

    #[test]
    fn signal_filtering_only_registered_types_enqueued() {
        let mut host = SignalsHost::new();
        host.on_signal(SignalType::Hangup).unwrap();

        // Deliver all three types
        host.deliver_signal(SignalType::Terminate); // ignored
        host.deliver_signal(SignalType::Hangup);    // enqueued
        host.deliver_signal(SignalType::Interrupt);  // ignored

        // Only hangup should be retrievable
        assert!(matches!(host.poll_signal(), Some(SignalType::Hangup)));
        assert!(host.poll_signal().is_none());
    }

    // ── Queue bounding through Host trait ──────────────────────────

    #[test]
    fn queue_bounding_drops_oldest() {
        let queue = SignalQueue::with_capacity(2);
        let mut host = SignalsHost::with_queue(queue);
        host.on_signal(SignalType::Terminate).unwrap();
        host.on_signal(SignalType::Hangup).unwrap();
        host.on_signal(SignalType::Interrupt).unwrap();

        host.deliver_signal(SignalType::Terminate);  // [Terminate]
        host.deliver_signal(SignalType::Hangup);     // [Terminate, Hangup]
        host.deliver_signal(SignalType::Interrupt);  // [Hangup, Interrupt]

        assert!(matches!(host.poll_signal(), Some(SignalType::Hangup)));
        assert!(matches!(host.poll_signal(), Some(SignalType::Interrupt)));
        assert!(host.poll_signal().is_none());
    }

    #[test]
    fn deliver_20_poll_only_16() {
        let mut host = SignalsHost::new();
        host.on_signal(SignalType::Terminate).unwrap();

        for _ in 0..20 {
            host.deliver_signal(SignalType::Terminate);
        }

        let mut count = 0;
        while host.poll_signal().is_some() {
            count += 1;
        }
        assert_eq!(count, 16);
    }

    // ── Full lifecycle ─────────────────────────────────────────────

    #[test]
    fn full_lifecycle_register_deliver_poll_via_host_trait() {
        let mut host = SignalsHost::new();

        // Register interest via Host trait
        host.on_signal(SignalType::Terminate).unwrap();

        // Deliver via host-side API
        host.deliver_signal(SignalType::Terminate);

        // Poll via Host trait
        let signal = host.poll_signal();
        assert!(matches!(signal, Some(SignalType::Terminate)));

        // Empty
        assert!(host.poll_signal().is_none());
    }

    #[test]
    fn interleaved_operations() {
        let mut host = SignalsHost::new();
        host.on_signal(SignalType::Terminate).unwrap();
        host.on_signal(SignalType::Hangup).unwrap();

        // Deliver one, poll one, deliver two more
        host.deliver_signal(SignalType::Terminate);
        assert!(matches!(host.poll_signal(), Some(SignalType::Terminate)));

        host.deliver_signal(SignalType::Hangup);
        host.deliver_signal(SignalType::Terminate);
        assert!(matches!(host.poll_signal(), Some(SignalType::Hangup)));
        assert!(matches!(host.poll_signal(), Some(SignalType::Terminate)));
        assert!(host.poll_signal().is_none());
    }
}
