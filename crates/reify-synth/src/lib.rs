//! Deterministic induction of task-scoped MCP tools from captured HTTP exchanges.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
pub mod cluster;
pub mod compiler;
pub mod engine;
pub mod prune;
pub mod schema;
mod words;
pub use engine::{SynthEngine, SynthOutput};
