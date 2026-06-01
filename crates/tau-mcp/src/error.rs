//! Error type for tau-mcp.

use alloc::string::String;
use thiserror::Error;

/// Errors surfaced by the tau-mcp protocol + transport layer.
///
/// Categories:
/// - [`McpError::Serde`] — JSON serde failure (envelope shape, payload
///   shape).
/// - [`McpError::Transport`] — transport-level failure (I/O, framing,
///   closed peer).
/// - [`McpError::Protocol`] — MCP-protocol violation (unexpected message
///   id, missing required field after deserialization).
/// - [`McpError::ContractDrift`] — runtime re-hash of `tools/list` does
///   not match the pinned `contract_hash`.
/// - [`McpError::Refused`] — host handler refused an inbound request
///   (e.g. sampling refused due to empty allowlist).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpError {
    /// JSON (de)serialization error.
    #[error("MCP serde error: {0}")]
    Serde(String),

    /// Transport-level error.
    #[error("MCP transport error: {0}")]
    Transport(String),

    /// Protocol violation.
    #[error("MCP protocol error: {0}")]
    Protocol(String),

    /// Contract hash drifted vs the pinned/lockfile value.
    #[error("MCP contract drift: observed {observed}, expected {expected}")]
    ContractDrift {
        /// Observed (live re-hashed) contract hash, lowercase hex.
        observed: String,
        /// Expected (pinned / lockfile) contract hash, lowercase hex.
        expected: String,
    },

    /// Host handler refused an inbound request.
    #[error("MCP inbound refused: {0}")]
    Refused(String),
}

impl From<serde_json::Error> for McpError {
    fn from(e: serde_json::Error) -> Self {
        McpError::Serde(alloc::format!("{e}"))
    }
}
