//! Error types for the host_lifecycle layer.

use thiserror::Error;

/// Failure to parse an MCP server URL from `[tools.<name>] mcp = "..."`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UrlParseError {
    /// Empty URL after stripping whitespace.
    #[error("MCP URL is empty")]
    Empty,
    /// URL scheme is not recognized in v0 (`stdio:` lands in PR-2,
    /// `http`/`https` land in PR-3, all others are rejected).
    #[error("unsupported MCP URL scheme: {scheme:?}")]
    UnsupportedScheme {
        /// The scheme observed (e.g. `"ws"`, `"file"`).
        scheme: String,
    },
    /// `stdio:` URL had an empty command after the prefix.
    #[error("stdio: URL has empty command after prefix")]
    EmptyStdioCommand,
    /// `cassette:` URL had no path after the prefix (e.g. `cassette:`).
    #[error("cassette URL has empty path")]
    EmptyCassettePath,
}

/// Failure during the MCP handshake (initialize / tools/list).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HandshakeError {
    /// Transport-level error during handshake.
    #[error("transport error during handshake: {0}")]
    Transport(String),
    /// Server returned an error response to `initialize` or `tools/list`.
    #[error("server returned error during handshake: code={code} message={message}")]
    ServerError {
        /// JSON-RPC error code.
        code: i32,
        /// JSON-RPC error message.
        message: String,
    },
    /// Handshake exceeded the configured timeout.
    #[error("handshake timed out after {millis}ms")]
    Timeout {
        /// Configured timeout in milliseconds.
        millis: u64,
    },
    /// Server's response shape was malformed.
    #[error("malformed handshake response: {0}")]
    Malformed(String),
}

/// Failure during host_lifecycle::open.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LifecycleError {
    /// URL parse failure.
    #[error("URL parse: {0}")]
    UrlParse(#[from] UrlParseError),
    /// Subprocess spawn failure (stdio transport).
    #[error("stdio spawn: {0}")]
    StdioSpawn(#[from] crate::transport_stdio::StdioSpawnError),
    /// HTTP dial failure (Streamable HTTP transport).
    #[error("http dial: {0}")]
    HttpSpawn(#[from] crate::transport_http::HttpSpawnError),
    /// Handshake failure.
    #[error("handshake: {0}")]
    Handshake(#[from] HandshakeError),
    /// Cassette IO error (file read failure).
    #[error("cassette IO error at {path}")]
    Io {
        /// Path to the cassette file.
        path: std::path::PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// Cassette parse error (invalid JSONL format).
    #[error("cassette parse error: {source}")]
    CassetteParse {
        /// Underlying replay error.
        #[source]
        source: tau_mcp::cassette::ReplayError,
    },
}
