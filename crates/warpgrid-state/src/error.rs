//! Error types for the WarpGrid state store.

use thiserror::Error;

/// Result type alias for state store operations.
pub type StateResult<T> = Result<T, StateError>;

/// Errors that can occur during state store operations.
#[derive(Debug, Error)]
pub enum StateError {
    #[error("failed to open database: {0}")]
    Open(String),

    #[error("transaction error: {0}")]
    Transaction(String),

    #[error("table error: {0}")]
    Table(String),

    #[error("read error: {0}")]
    Read(String),

    #[error("write error: {0}")]
    Write(String),

    #[error("serialization error: {0}")]
    Serialize(String),

    #[error("deserialization error: {0}")]
    Deserialize(String),

    #[error("not found: {0}")]
    NotFound(String),
}
