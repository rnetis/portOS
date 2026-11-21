//! Core error type shared by every PortOS crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("internal error: {0}")]
    Internal(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl CoreError {
    /// Machine-readable kind, surfaced as `problem.type` suffix by the API.
    pub fn kind(&self) -> &'static str {
        match self {
            CoreError::NotFound(_) => "not-found",
            CoreError::Validation(_) => "validation",
            CoreError::Conflict(_) => "conflict",
            CoreError::Unauthorized => "unauthorized",
            CoreError::Forbidden => "forbidden",
            CoreError::Internal(_) => "internal",
            CoreError::Unavailable(_) => "unavailable",
        }
    }

    /// i18n catalog key carrying the human message for this error class.
    /// The API layer fills parameters (e.g. `{detail}`) and localizes it
    /// per the caller's language (PRD L-8).
    pub fn message_key(&self) -> &'static str {
        match self {
            CoreError::NotFound(_) => "error.not_found",
            CoreError::Validation(_) => "error.validation",
            CoreError::Conflict(_) => "error.conflict",
            CoreError::Unauthorized => "error.unauthorized",
            CoreError::Forbidden => "error.forbidden",
            CoreError::Internal(_) => "error.internal",
            CoreError::Unavailable(_) => "error.unavailable",
        }
    }
}
