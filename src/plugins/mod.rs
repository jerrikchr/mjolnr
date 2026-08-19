//! Plugin protocol and capability module host (ADR-0016).
//!
//! Provides isolated subprocess execution, JSON-RPC 2.0 stdio communication,
//! observer lifecycle hooks, and governed tool routing.

pub mod host;
pub mod jsonrpc;
pub mod transport;

pub use host::PluginHost;
pub use jsonrpc::{
    JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, error_codes,
};
pub use transport::PluginTransport;
