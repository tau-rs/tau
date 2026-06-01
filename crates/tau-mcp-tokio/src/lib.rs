//! tau-mcp-tokio — tokio runtime + transports for tau-mcp.
//!
//! PR-2 ships the stdio transport + host lifecycle. PR-3 adds HTTP +
//! cassette-as-transport. PR-5 wires the `McpBridge` ToolDispatcher.

pub mod bridge;
pub mod host_lifecycle;
pub mod transport_http;
pub mod transport_stdio;

pub use host_lifecycle::{
    open, HandshakeError, LifecycleError, McpClient, McpClientOptions, McpUrl, UrlParseError,
};
pub use transport_stdio::{McpStdioServer, StdioSpawnError, StdioTransportError};
