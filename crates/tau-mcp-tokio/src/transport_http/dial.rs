//! `dial(url, options) → Arc<McpHttpServer>` — HTTP transport dial entrypoint.
//!
//! Composes URL host extraction → reqwest::Client build → HttpClientGuard
//! → McpHttpServer construction. Called by host_lifecycle::open() for
//! `http:` and `https:` URLs.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, instrument};
use url::Url;

use crate::transport_http::error::HttpSpawnError;
use crate::transport_http::guard::HttpClientGuard;
use crate::transport_http::server::McpHttpServer;

/// Options for HTTP dial.
#[derive(Debug, Clone)]
pub struct HttpDialOptions {
    /// Per-request timeout for the reqwest client.
    pub request_timeout: Duration,
    /// User-Agent string sent with every request.
    pub user_agent: String,
}

impl Default for HttpDialOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            user_agent: concat!("tau-mcp-tokio/", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
}

/// Dial an HTTP MCP server. Returns a ready `Arc<McpHttpServer>` that
/// `host_lifecycle::open` then drives through the MCP handshake.
#[instrument(name = "mcp_http_dial", skip(options), fields(url = %url))]
pub fn dial(
    url: Url,
    options: HttpDialOptions,
) -> Result<Arc<McpHttpServer>, HttpSpawnError> {
    let pinned_host = url.host().ok_or_else(|| HttpSpawnError::NoHost {
        url: url.to_string(),
    })?;
    let pinned_host = match pinned_host {
        url::Host::Domain(d) => url::Host::Domain(d.to_string()),
        url::Host::Ipv4(a) => url::Host::Ipv4(a),
        url::Host::Ipv6(a) => url::Host::Ipv6(a),
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(options.request_timeout)
        .user_agent(options.user_agent)
        .build()
        .map_err(|e| HttpSpawnError::ClientBuild(format!("{e}")))?;
    let guard = HttpClientGuard::new(client, pinned_host);
    info!(host = %guard.pinned_host(), "constructed pinned HTTP client");
    Ok(McpHttpServer::new(guard, url))
}
