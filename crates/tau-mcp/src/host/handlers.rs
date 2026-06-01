//! `HostHandlers` trait — host-side response to server-initiated
//! requests (sampling + roots in v0).
//!
//! v0 ships two real inbound handlers (sampling, roots) plus a
//! default-deny baseline impl ([`DefaultDenyHandlers`]). PR-5 wires the
//! real impl in `tau-cli` carrying the agent's `LlmBackend` and the
//! per-server `sampling.models` allowlist + `roots` declaration.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use thiserror::Error;

use crate::protocol::roots::Root;
use crate::protocol::sampling::{SamplingCreateMessageRequest, SamplingCreateMessageResponse};

/// Error returned by an inbound handler to refuse a server request.
///
/// Surfaces as an MCP `JsonRpcError` payload to the server with code
/// = `-32000` (custom error range).
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum InboundError {
    /// Server requested sampling but the host has no models allowlisted.
    #[error("sampling refused: allowlist is empty")]
    SamplingNotAllowed,
    /// Server requested sampling with a model that's not in the
    /// allowlist.
    #[error("sampling refused: model {requested:?} not in allowlist")]
    SamplingModelRefused {
        /// The model the server asked for.
        requested: String,
    },
    /// Server requested roots but the host's roots list is empty
    /// (semantically the same as "no fs visibility granted").
    #[error("roots returned []: no roots declared")]
    RootsEmpty,
    /// Backend invocation (LLM call) failed.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Type alias for a boxed-future returning a result.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Host-side handlers for inbound (server-initiated) MCP requests.
///
/// One impl per contracted MCP server. Concrete impl lives in PR-5
/// (`tau-cli::cmd::run::ir_dispatcher::WiredHostHandlers` or similar)
/// and composes the agent's `LlmBackend` + per-server allowlist.
pub trait HostHandlers: Send + Sync {
    /// Handle a `sampling/createMessage` request from the server.
    fn sampling<'a>(
        &'a self,
        req: SamplingCreateMessageRequest,
    ) -> BoxFuture<'a, Result<SamplingCreateMessageResponse, InboundError>>;

    /// Handle a `roots/list` request from the server.
    fn roots<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Root>, InboundError>>;
}

/// Default-deny baseline impl: refuses every inbound request.
///
/// Suitable as a starting point for tests that don't need to exercise
/// inbound handlers. PR-5's production impl follows the same trait
/// shape but composes real backends.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultDenyHandlers;

impl HostHandlers for DefaultDenyHandlers {
    fn sampling<'a>(
        &'a self,
        _req: SamplingCreateMessageRequest,
    ) -> BoxFuture<'a, Result<SamplingCreateMessageResponse, InboundError>> {
        Box::pin(async { Err(InboundError::SamplingNotAllowed) })
    }

    fn roots<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Root>, InboundError>> {
        // Default-deny for roots returns an EMPTY list, not an error —
        // per the spec, `roots/list` returning `[]` is a valid response
        // meaning "host grants no fs visibility." Servers must accept
        // that gracefully.
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::sampling::{ModelPreferences, SamplingContent, SamplingMessage};
    use alloc::string::ToString;
    use alloc::vec;

    fn sample_request() -> SamplingCreateMessageRequest {
        SamplingCreateMessageRequest {
            messages: vec![SamplingMessage {
                role: "user".to_string(),
                content: SamplingContent::Text {
                    text: "x".to_string(),
                },
            }],
            model_preferences: Some(ModelPreferences::default()),
            system_prompt: None,
            include_context: None,
            max_tokens: None,
            additional: alloc::collections::BTreeMap::new(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_deny_sampling_refuses() {
        let h = DefaultDenyHandlers;
        let r = h.sampling(sample_request()).await;
        assert!(matches!(r, Err(InboundError::SamplingNotAllowed)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_deny_roots_returns_empty() {
        let h = DefaultDenyHandlers;
        let r = h.roots().await.expect("ok");
        assert!(r.is_empty());
    }
}
