//! Shared, transport-independent foundations for Reify.
//!
//! HTTP headers borrow their backing map; JSON domain values own their data.
//! No runtime, network connections, or session storage is required.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
pub mod cache;
pub mod domain;
pub mod error;
pub mod mcp;
pub use error::ReifyError;
