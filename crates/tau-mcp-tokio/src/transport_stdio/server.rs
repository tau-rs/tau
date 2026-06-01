//! `McpStdioServer` — stdio MCP server handle.
//!
//! Owns the spawned `tokio::process::Child` plus a `JsonLineFramer`
//! over its stdin/stdout. Impls `tau_mcp::transport::Transport`.

use std::pin::Pin;
use std::sync::Arc;

use tau_mcp::protocol::JsonRpcMessage;
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::transport_stdio::error::StdioTransportError;
use crate::transport_stdio::framer::JsonLineFramer;

/// Live stdio MCP server.
///
/// Constructed by `host_lifecycle::open` after `spawn` completes;
/// passed by `Arc` into the `McpClient`.
pub struct McpStdioServer {
    framer: Mutex<JsonLineFramer<ChildStdout, ChildStdin>>,
    _child: Mutex<Child>,
}

impl McpStdioServer {
    /// Construct from a spawned child. Steals stdin/stdout out of the
    /// child handle.
    ///
    /// # Errors
    ///
    /// - Returns `McpError::Transport` if the child's stdin or stdout
    ///   were not piped (caller bug — `spawn` is supposed to pipe them).
    pub fn from_child(mut child: Child) -> Result<Arc<Self>, McpError> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdin pipe".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdout pipe".into()))?;
        let framer = JsonLineFramer::new(stdout, stdin);
        Ok(Arc::new(Self {
            framer: Mutex::new(framer),
            _child: Mutex::new(child),
        }))
    }
}

impl Transport for McpStdioServer {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let mut framer = self.framer.lock().await;
            framer
                .write_message(msg)
                .await
                .map_err(convert_transport_error)
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut framer = self.framer.lock().await;
            framer
                .read_message()
                .await
                .map_err(convert_transport_error)
        })
    }
}

fn convert_transport_error(e: StdioTransportError) -> McpError {
    match e {
        StdioTransportError::Io(s) => McpError::Transport(s),
        StdioTransportError::Json(s) => McpError::Serde(s),
        StdioTransportError::ChildExited { status } => {
            McpError::Transport(format!("child exited: {status}"))
        }
    }
}
