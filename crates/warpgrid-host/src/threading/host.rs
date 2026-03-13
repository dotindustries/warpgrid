//! Threading model declaration host functions.
//!
//! Implements the `warpgrid:shim/threading` [`Host`] trait, storing the
//! guest-declared threading model and enforcing immutability (single
//! declaration only).
//!
//! # Declaration flow
//!
//! ```text
//! Guest calls declare_threading_model(model)
//!   → ThreadingHost checks immutability (already declared?)
//!     → Already declared → Err("threading model already declared")
//!     → Not declared:
//!       → ParallelRequired → warn (not supported, cooperative fallback)
//!       → Cooperative      → info log
//!       → Store model, return Ok(())
//! ```

use crate::bindings::warpgrid::shim::threading::{Host, ThreadingModel};

/// Host-side implementation of the `warpgrid:shim/threading` interface.
///
/// Stores the guest-declared threading model and enforces that it can
/// only be declared once per instance (immutability after first call).
///
/// The host can query the declared model via [`threading_model`] to
/// adapt execution strategy.
///
/// [`threading_model`]: ThreadingHost::threading_model
pub struct ThreadingHost {
    model: Option<ThreadingModel>,
}

impl ThreadingHost {
    /// Create a new `ThreadingHost` with no declared model.
    pub fn new() -> Self {
        Self { model: None }
    }

    /// Query the declared threading model.
    ///
    /// Returns `None` if the guest has not yet declared a model.
    pub fn threading_model(&self) -> Option<&ThreadingModel> {
        self.model.as_ref()
    }
}

impl Default for ThreadingHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for ThreadingHost {
    fn declare_threading_model(&mut self, model: ThreadingModel) -> Result<(), String> {
        if self.model.is_some() {
            return Err("threading model already declared".to_string());
        }

        match model {
            ThreadingModel::ParallelRequired => {
                tracing::warn!(
                    ?model,
                    "parallel threading requested but not supported; execution will use cooperative mode"
                );
            }
            ThreadingModel::Cooperative => {
                tracing::info!(?model, "cooperative threading model declared");
            }
        }

        self.model = Some(model);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ────────────────────────────────────────────────

    #[test]
    fn new_host_has_no_model() {
        let host = ThreadingHost::new();
        assert!(host.threading_model().is_none());
    }

    #[test]
    fn default_has_no_model() {
        let host = ThreadingHost::default();
        assert!(host.threading_model().is_none());
    }

    // ── Cooperative declaration ─────────────────────────────────────

    #[test]
    fn declare_cooperative_succeeds() {
        let mut host = ThreadingHost::new();
        let result = host.declare_threading_model(ThreadingModel::Cooperative);
        assert!(result.is_ok());
    }

    #[test]
    fn declare_cooperative_is_queryable() {
        let mut host = ThreadingHost::new();
        host.declare_threading_model(ThreadingModel::Cooperative).unwrap();
        assert!(matches!(
            host.threading_model(),
            Some(&ThreadingModel::Cooperative)
        ));
    }

    // ── Parallel-required declaration ───────────────────────────────

    #[test]
    fn declare_parallel_required_succeeds() {
        let mut host = ThreadingHost::new();
        let result = host.declare_threading_model(ThreadingModel::ParallelRequired);
        assert!(result.is_ok());
    }

    #[test]
    fn declare_parallel_required_is_queryable() {
        let mut host = ThreadingHost::new();
        host.declare_threading_model(ThreadingModel::ParallelRequired).unwrap();
        assert!(matches!(
            host.threading_model(),
            Some(&ThreadingModel::ParallelRequired)
        ));
    }

    // ── Immutability (double declaration) ───────────────────────────

    #[test]
    fn double_declaration_returns_error() {
        let mut host = ThreadingHost::new();
        host.declare_threading_model(ThreadingModel::Cooperative).unwrap();

        let result = host.declare_threading_model(ThreadingModel::Cooperative);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already declared"));
    }

    #[test]
    fn double_declaration_different_models_returns_error() {
        let mut host = ThreadingHost::new();
        host.declare_threading_model(ThreadingModel::Cooperative).unwrap();

        let result = host.declare_threading_model(ThreadingModel::ParallelRequired);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already declared"));
    }

    #[test]
    fn double_declaration_preserves_original_model() {
        let mut host = ThreadingHost::new();
        host.declare_threading_model(ThreadingModel::Cooperative).unwrap();

        let _ = host.declare_threading_model(ThreadingModel::ParallelRequired);
        assert!(matches!(
            host.threading_model(),
            Some(&ThreadingModel::Cooperative)
        ));
    }
}
