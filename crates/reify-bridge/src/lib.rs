//! Loopback HTTP forwarding with explicit, domain-scoped developer credentials.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
pub mod server;
pub mod store;
pub use server::{
    BridgeServer, ForwardRequest, ForwardResponse, ForwardTransport, HttpTransport, ResponseBody,
};
