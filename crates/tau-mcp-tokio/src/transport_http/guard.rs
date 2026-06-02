//! Pinned-host newtype around `reqwest::Client`.
//!
//! Enforces the spec §9 invariant: every outbound HTTP request must
//! go to the pinned MCP server host. Combined with
//! `redirect::Policy::none()` on the inner client, this guarantees the
//! `net.http` capability's `host` field is honored at the wire — a
//! 3xx redirect to a different host fails closed, and any code path
//! that constructs a different URL is refused before the request
//! leaves the process.

use reqwest::{Client, Request, RequestBuilder, Response};
use url::{Host, Url};

use crate::transport_http::error::HttpTransportError;

/// HTTP client guard pinned to a single host.
#[derive(Debug, Clone)]
pub struct HttpClientGuard {
    /// Inner reqwest client (constructed with `redirect::Policy::none()`).
    inner: Client,
    /// Pinned host extracted from the MCP server URL at dial time.
    pinned_host: Host<String>,
}

impl HttpClientGuard {
    /// Construct from an already-built client + a pinned host.
    pub fn new(inner: Client, pinned_host: Host<String>) -> Self {
        Self { inner, pinned_host }
    }

    /// Get the pinned host (for diagnostics + tests).
    pub fn pinned_host(&self) -> &Host<String> {
        &self.pinned_host
    }

    /// Borrow the inner client. Use ONLY for building requests via
    /// `Client::request(...)`; always send via [`HttpClientGuard::send`].
    pub fn inner(&self) -> &Client {
        &self.inner
    }

    /// Validate the request URL's host against the pinned host, then
    /// execute the request via the inner client.
    pub async fn send(
        &self,
        request: Request,
    ) -> Result<Response, HttpTransportError> {
        let url = request.url().clone();
        self.check_host(&url)?;
        self.inner
            .execute(request)
            .await
            .map_err(|e| HttpTransportError::Send(format!("{e}")))
    }

    /// Convenience: build + send a POST request to the given URL.
    pub fn post(&self, url: Url) -> RequestBuilder {
        self.inner.post(url)
    }

    /// Check that `url`'s host matches the pinned host.
    pub fn check_host(&self, url: &Url) -> Result<(), HttpTransportError> {
        let actual = url
            .host()
            .ok_or_else(|| HttpTransportError::HostPinViolation {
                actual: "<no host>".to_string(),
                pinned: self.pinned_host.clone(),
            })?;
        // url::Host<&str> vs url::Host<String> — normalize.
        let actual_owned: Host<String> = match actual {
            Host::Domain(d) => Host::Domain(d.to_string()),
            Host::Ipv4(a) => Host::Ipv4(a),
            Host::Ipv6(a) => Host::Ipv6(a),
        };
        if actual_owned != self.pinned_host {
            return Err(HttpTransportError::HostPinViolation {
                actual: format!("{actual_owned}"),
                pinned: self.pinned_host.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(pinned: &str) -> HttpClientGuard {
        let host = Host::parse(pinned).expect("parse host");
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build client");
        HttpClientGuard::new(client, host)
    }

    #[test]
    fn pinned_host_allowed() {
        let g = guard("example.com");
        let url = Url::parse("https://example.com/path").unwrap();
        g.check_host(&url).expect("same host is allowed");
    }

    #[test]
    fn different_host_refused() {
        let g = guard("example.com");
        let url = Url::parse("https://evil.com/path").unwrap();
        let err = g.check_host(&url).expect_err("different host refused");
        assert!(matches!(err, HttpTransportError::HostPinViolation { .. }));
    }

    #[test]
    fn missing_host_refused() {
        let g = guard("example.com");
        // `file:` URLs have no host — should refuse.
        let url = Url::parse("file:///etc/passwd").unwrap();
        let err = g.check_host(&url).expect_err("missing host refused");
        assert!(matches!(err, HttpTransportError::HostPinViolation { .. }));
    }
}
