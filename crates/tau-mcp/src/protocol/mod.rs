//! MCP wire types — JSON-RPC envelopes and method-specific payloads.
//!
//! Per MCP spec revision 2025-03-26 (the version tau v0 targets). Method
//! payloads live in submodules; envelopes live in [`jsonrpc`].

pub mod initialize;
pub mod jsonrpc;
pub mod notifications;
pub mod roots;
pub mod sampling;
pub mod tools;

pub use jsonrpc::{
    JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId,
    JSONRPC_VERSION,
};
