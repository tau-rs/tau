//! Streamable HTTP MCP transport (PR-3).
//!
//! Per MCP spec rev 2025-03-26, the Streamable HTTP transport uses
//! POST for client→server messages and either application/json or
//! text/event-stream for server→client responses.

pub mod dial;
pub mod error;
pub mod guard;
pub mod server;
pub mod session;
pub mod sse;

pub use error::{HttpSpawnError, HttpTransportError};
pub use server::McpHttpServer;
