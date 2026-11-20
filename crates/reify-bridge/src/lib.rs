//! Loopback HTTP forwarding with explicit, domain-scoped developer credentials.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
pub mod client;
pub mod server;
pub use client::BridgeClient;
mod session;
pub mod store;
pub use server::{
    BridgeServer, ForwardRequest, ForwardResponse, ForwardTransport, HttpTransport, ResponseBody,
};
