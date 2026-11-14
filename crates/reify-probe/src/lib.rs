//! Bounded SSR state extraction, with isolated browser traffic capture as fallback.
#![warn(missing_docs)]
pub mod auth;
pub mod cdp;
pub mod engine;
pub mod filter;
pub mod process;
pub mod ssr;
pub use engine::{ProbeEngine, ProbeOutput};
use reify_core::ReifyError;
fn failure(context: &str, error: impl std::fmt::Display) -> ReifyError {
    ReifyError::ExecutionError(format!("{context}: {error}"))
}
