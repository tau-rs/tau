//! `Transport` trait — implemented by `tau-mcp-tokio` for stdio and HTTP.
//!
//! Defined here so the protocol layer + host loop are transport-agnostic
//! (γ.5 wasm / embassy shells can implement this trait against their own
//! I/O without taking a tokio dep).

use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;

use crate::error::McpError;
use crate::protocol::JsonRpcMessage;

/// A bidirectional MCP transport.
///
/// `send_message` writes one MCP message to the wire (any framing the
/// transport requires happens inside the impl). `next_message` reads the
/// next inbound message; returns `Ok(None)` if the transport has cleanly
/// closed.
pub trait Transport: Send + Sync {
    /// Send one MCP message to the peer.
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;

    /// Read the next inbound MCP message. Returns `Ok(None)` on clean
    /// close.
    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>;
}
