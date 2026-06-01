//! Host-side handlers for server-initiated MCP requests.
//!
//! When an MCP server sends an inbound request (sampling, roots, etc.),
//! tau-mcp-tokio's inbound dispatch routes it through an impl of
//! [`HostHandlers`]. v0 ships the two handlers the philosophy doc stars:
//! sampling (delegated inference) and roots (capability gate at fs
//! boundary). Default-deny baseline impl is [`DefaultDenyHandlers`].

pub mod handlers;

pub use handlers::{DefaultDenyHandlers, HostHandlers, InboundError};
