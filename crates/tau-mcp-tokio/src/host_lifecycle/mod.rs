//! Host lifecycle for a contracted MCP server.
//!
//! `open(url, plan, gate, options)` is the v0 entrypoint: parse the URL,
//! spawn (stdio) or dial (HTTP — PR-3), drive the MCP handshake, return
//! a live `McpClient`.

pub mod client;
pub mod error;
pub mod handshake;
pub mod open;
pub mod url;

pub use client::{McpClient, McpClientOptions};
pub use error::{HandshakeError, LifecycleError, UrlParseError};
pub use open::open;
pub use url::{parse_url, McpUrl};
