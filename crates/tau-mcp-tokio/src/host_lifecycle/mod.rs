//! Host lifecycle for a contracted MCP server.
//!
//! `open(url, plan, gate, options)` is the v0 entrypoint: parse the URL,
//! spawn (stdio) or dial (HTTP — PR-3), drive the MCP handshake, return
//! a live `McpClient`.

pub mod client;
pub mod error;
pub mod handshake;
pub mod inbound_dispatch;
pub mod open;
pub mod url;

pub use client::{McpClient, McpClientOptions};
pub use error::{HandshakeError, LifecycleError, UrlParseError};
pub use inbound_dispatch::{spawn_inbound_dispatch, InboundDispatchHandle, INBOUND_REFUSED_ERROR_CODE};
pub use open::open;
pub use url::{parse_url, McpUrl};
