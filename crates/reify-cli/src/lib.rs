//! CLI parsing, orchestration, and terminal rendering for the `reify` executable.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
pub mod args;
pub mod commands;
pub mod ui;
pub use args::{Cli, Color, Command};
use reify_core::ReifyError;
/// Errors with concise messages suitable for the terminal.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A core compilation, protocol, or execution failure.
    #[error(transparent)]
    Core(#[from] ReifyError),
    /// Filesystem/terminal/listener failure.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Invalid JSON input/output.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Explicit user cancellation.
    #[error("interrupted")]
    Interrupted,
    /// A tool completed with an upstream failure.
    #[error("tool execution failed (see JSON result above)")]
    ToolFailed,
}
pub(crate) fn failure(message: impl Into<String>) -> CliError {
    ReifyError::ExecutionError(message.into()).into()
}
