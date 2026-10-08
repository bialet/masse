//! Error types for the harness core.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("provider error: {0}")]
    Provider(String),
    #[error("session error: {0}")]
    Session(#[from] std::io::Error),
    #[error("agent exceeded {0} tool iterations without finishing")]
    TooManyIterations(usize),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
