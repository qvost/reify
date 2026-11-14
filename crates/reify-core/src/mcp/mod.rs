//! JSON-RPC envelopes, Streamable HTTP routing, and Reify tool models.
//! `initialize` detection is retained for legacy clients; it is not a modern handshake.
mod envelope;
mod headers;
mod tools;
pub use envelope::*;
pub use headers::*;
pub use tools::*;
/// Core protocol failures use the shared descriptive boundary error.
pub type McpError = crate::ReifyError;
/// Supported stateless MCP revision.
pub const PROTOCOL_VERSION: &str = "2026-07-28";
