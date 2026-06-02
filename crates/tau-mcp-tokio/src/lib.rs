//! tau-mcp-tokio — tokio runtime + transports for tau-mcp.

pub mod bridge;
pub mod host_lifecycle;
pub mod resolver;
pub mod transport_http;
pub mod transport_stdio;

pub use host_lifecycle::{
    open, HandshakeError, LifecycleError, McpClient, McpClientOptions, McpUrl, UrlParseError,
};
pub use resolver::{resolve_all, LiveResolved, LiveResolverError, McpEntryInput};
pub use transport_http::{HttpSpawnError, HttpTransportError, McpHttpServer};
pub use transport_stdio::{McpStdioServer, StdioSpawnError, StdioTransportError};
