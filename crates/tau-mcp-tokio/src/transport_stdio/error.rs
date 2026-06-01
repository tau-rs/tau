//! Error types for the stdio transport.

use thiserror::Error;
use tau_ports::CapabilityError;

/// Failure during stdio MCP server spawn.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StdioSpawnError {
    /// `ProcessCapabilityGate::validate_plan` (which `wrap_spawn` calls
    /// internally) refused the plan.
    #[error("capability gate refused plan: {0}")]
    SandboxRefused(#[from] CapabilityError),
    /// `tokio::process::Command::spawn` failed (binary missing,
    /// permission denied, etc.).
    #[error("tokio spawn failed: {0}")]
    TokioSpawn(String),
}

impl From<std::io::Error> for StdioSpawnError {
    fn from(e: std::io::Error) -> Self {
        StdioSpawnError::TokioSpawn(format!("{e}"))
    }
}

/// Failure during a stdio transport read/write.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StdioTransportError {
    /// I/O error on child stdin/stdout.
    #[error("I/O on stdio transport: {0}")]
    Io(String),
    /// One of the framed JSON-RPC lines was not valid JSON.
    #[error("malformed JSON on stdio transport: {0}")]
    Json(String),
    /// The child process exited mid-conversation.
    #[error("child process exited (status: {status})")]
    ChildExited {
        /// Child exit status as a string (cross-platform).
        status: String,
    },
}

impl From<std::io::Error> for StdioTransportError {
    fn from(e: std::io::Error) -> Self {
        StdioTransportError::Io(format!("{e}"))
    }
}

impl From<serde_json::Error> for StdioTransportError {
    fn from(e: serde_json::Error) -> Self {
        StdioTransportError::Json(format!("{e}"))
    }
}
