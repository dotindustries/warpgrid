//! Error types for the sprite subsystem.

use thiserror::Error;

pub type SpriteResult<T> = Result<T, SpriteError>;

#[derive(Debug, Error)]
pub enum SpriteError {
    #[error("hypervisor error: {0}")]
    Hypervisor(String),

    #[error("VM not found: {0}")]
    VmNotFound(String),

    #[error("VM already exists: {0}")]
    VmAlreadyExists(String),

    #[error("pool exhausted: no warm VMs available and max capacity reached")]
    PoolExhausted,

    #[error("vsock error: {0}")]
    Vsock(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("invalid state transition: {from:?} -> {to:?}")]
    InvalidStateTransition {
        from: warpgrid_state::SpriteStatus,
        to: warpgrid_state::SpriteStatus,
    },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
