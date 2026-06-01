//! Subprocess stdio MCP transport.
//!
//! `McpStdioServer` wraps a sandboxed `tokio::process::Child` plus
//! line-delimited JSON-RPC framing on its stdin/stdout. It impls
//! `tau_mcp::transport::Transport`.

pub mod error;
pub mod framer;
pub mod server;
pub mod spawn;

pub use error::{StdioSpawnError, StdioTransportError};
pub use server::McpStdioServer;
pub use spawn::spawn;
