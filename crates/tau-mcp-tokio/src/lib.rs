//! tau-mcp-tokio — tokio runtime + transports for tau-mcp.
//!
//! Scaffold only in PR-1. stdio transport + sandbox-gated spawn land in
//! PR-2; Streamable HTTP transport + cassette replay-against-live land in
//! PR-3; the `McpBridge` ToolDispatcher adapter lands in PR-5.

pub mod bridge;
pub mod host_lifecycle;
pub mod transport_http;
pub mod transport_stdio;
