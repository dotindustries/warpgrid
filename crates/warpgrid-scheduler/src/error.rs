//! Scheduler error types.

use thiserror::Error;

/// Errors that can occur during scheduling operations.
#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("deployment not found: {0}")]
    DeploymentNotFound(String),

    #[error("deployment already scheduled: {0}")]
    AlreadyScheduled(String),

    #[error("no instances available for deployment: {0}")]
    NoInstancesAvailable(String),

    #[error("module not loaded: {0}")]
    ModuleNotLoaded(String),

    #[error("placement error: {0}")]
    Placement(String),

    #[error("state store error: {0}")]
    State(#[from] warpgrid_state::StateError),

    #[error("runtime error: {0}")]
    Runtime(#[from] anyhow::Error),
}

pub type SchedulerResult<T> = Result<T, SchedulerError>;
