//! Errors shared across compilation and execution boundaries.
use thiserror::Error;

/// A failure at a Reify system boundary.
#[derive(Debug, Error)]
pub enum ReifyError {
    /// A target could not be parsed or is not an HTTP(S) URL.
    #[error("invalid target URL: {0}")]
    InvalidTargetUrl(String),
    /// An envelope violates the protocol contract.
    #[error("protocol violation: {0}")]
    ProtocolViolation(String),
    /// JSON encoding or decoding failed.
    #[error("JSON serialization failed: {0}")]
    SerializationError(#[from] serde_json::Error),
    /// A header is malformed or ambiguous.
    #[error("header parse failed: {0}")]
    HeaderParseError(String),
    /// A tool input schema is invalid.
    #[error("schema validation failed: {0}")]
    SchemaValidationError(String),
    /// An upstream operation failed.
    #[error("execution failed: {0}")]
    ExecutionError(String),
}
