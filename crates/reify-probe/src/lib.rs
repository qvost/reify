//! Bounded, stateless browser traffic capture. Each probe owns an isolated browser.
#![warn(missing_docs)]
pub mod auth;
pub mod cdp;
pub mod engine;
pub mod filter;
pub mod process;
pub use engine::{ProbeEngine, ProbeOutput};
use reify_core::ReifyError;
fn failure(context: &str, error: impl std::fmt::Display) -> ReifyError {
    ReifyError::ExecutionError(format!("{context}: {error}"))
}
