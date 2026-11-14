//! Scoped, stateless MCP mounts with shared compilation and passive drift recovery.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
pub mod handler;
pub mod healing;
mod parameter_headers;
pub mod proxy;
pub mod server;
pub mod store;
mod wire;
use reify_core::ReifyError;
pub use server::{AppBuilder, BrowserAppBuilder, MountedApp, ReifyServer};
fn failure(message: impl Into<String>) -> ReifyError {
    ReifyError::ExecutionError(message.into())
}
