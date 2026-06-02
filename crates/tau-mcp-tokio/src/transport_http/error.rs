//! Error types for the Streamable HTTP transport.

use thiserror::Error;
use url::Host;

/// Failure during HTTP MCP server dial.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpSpawnError {
    /// URL has no host component (e.g. `http:///foo`).
    #[error("HTTP URL has no host: {url}")]
    NoHost {
        /// URL we tried to dial.
        url: String,
    },
    /// `reqwest::ClientBuilder::build` failed (TLS init, etc.).
    #[error("reqwest client construction failed: {0}")]
    ClientBuild(String),
}

/// Failure during an HTTP request/response cycle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpTransportError {
    /// Outbound request URL host did not match the pinned host.
    #[error("URL host {actual:?} does not match pinned host {pinned:?}")]
    HostPinViolation {
        /// Host the caller tried to contact.
        actual: String,
        /// Pinned host from the original URL.
        pinned: Host<String>,
    },
    /// `reqwest::Client::execute` failed (network, TLS, etc.).
    #[error("HTTP send failed: {0}")]
    Send(String),
    /// HTTP server returned non-2xx.
    #[error("HTTP server returned {status}: {body}")]
    Status {
        /// Status code.
        status: u16,
        /// Response body (truncated if large).
        body: String,
    },
    /// SSE frame parse failure.
    #[error("SSE parse error: {0}")]
    SseParse(String),
    /// JSON-RPC message decode failure.
    #[error("JSON-RPC decode failure: {0}")]
    JsonDecode(String),
    /// Inbound channel send/recv error (typically transport shutdown).
    #[error("inbound channel error: {0}")]
    Channel(String),
}

impl From<serde_json::Error> for HttpTransportError {
    fn from(e: serde_json::Error) -> Self {
        HttpTransportError::JsonDecode(format!("{e}"))
    }
}
