//! `ConformanceDispatcher` — the [`ToolDispatcher`] for the canonical
//! fan-monitor scenario.
//!
//! Routes the scenario's three tools and supplies the scripted
//! [`SequencedLlm`](crate::sequenced_llm::SequencedLlm) backend:
//!
//! - native `read_temp` → deterministic body `32`
//! - native `set_fan`   → deterministic body `{"ok": true}`
//! - MCP `weather`       → real cassette replay via an opened
//!   [`McpClient`] (`client.call_tool("weather", args)`), with the
//!   response's text content blocks mapped deterministically to JSON.
//!
//! Clock/random are intentionally NOT overridden (the trait defaults
//! return `None`); under the crate's `test-fixtures` feature `run_agent`
//! injects `MockClock` / `DeterministicRandom`, which is exactly what a
//! cross-profile conformance run wants — identical, deterministic time
//! and entropy in both dev and wasm profiles.

use std::pin::Pin;
use std::sync::Arc;

use core::future::Future;

use serde_json::{json, Value};

use tau_ir::ToolId;
use tau_mcp::protocol::tools::{ContentBlock, ToolsCallResponse};
use tau_mcp_tokio::host_lifecycle::client::McpClient;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

/// [`ToolDispatcher`] for the fan-monitor conformance scenario.
pub(crate) struct ConformanceDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    weather: Option<Arc<McpClient>>,
}

impl ConformanceDispatcher {
    /// Construct with both native tools and a live MCP `weather` client.
    ///
    /// Used by `DevProfile` (Task 9), which opens the cassette-backed
    /// `weather` server before driving the scenario.
    // used by DevProfile in Task 9
    #[allow(dead_code)]
    pub(crate) fn new(backend: Arc<dyn DynLlmBackend>, weather: Arc<McpClient>) -> Self {
        Self {
            backend,
            weather: Some(weather),
        }
    }

    /// Construct with only the native tools wired (no MCP client).
    ///
    /// Invoking the `weather` tool against this dispatcher surfaces a
    /// [`RuntimeError::Internal`] — a scenario that drives `weather`
    /// without an MCP client wired is a wiring bug and should fail loudly.
    // used by DevProfile in Task 9 (lib-only `--all-targets` pass sees the
    // sole in-crate caller as test-cfg'd and flags this otherwise)
    #[allow(dead_code)]
    pub(crate) fn new_native_only(backend: Arc<dyn DynLlmBackend>) -> Self {
        Self {
            backend,
            weather: None,
        }
    }
}

impl ToolDispatcher for ConformanceDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        // Clone the borrowed inputs into the async move so the returned
        // future is `Send + 'a` without holding the `&ToolId`/`&Value`
        // borrows across the `.await` (mirrors ForwardingDispatcher).
        let name = tool_id.0.clone();
        let args_owned = args.clone();
        let weather = self.weather.clone();

        Box::pin(async move {
            match name.as_str() {
                "read_temp" => Ok(ToolInvocationResult {
                    body: Some(json!(32)),
                    error: None,
                }),
                "set_fan" => Ok(ToolInvocationResult {
                    body: Some(json!({"ok": true})),
                    error: None,
                }),
                "weather" => {
                    let client = weather.ok_or_else(|| RuntimeError::Internal {
                        message: "weather invoked but no MCP client wired".to_string(),
                    })?;
                    let resp = client.call_tool("weather", args_owned).await.map_err(|e| {
                        RuntimeError::Internal {
                            message: format!("MCP weather call_tool failed: {e}"),
                        }
                    })?;
                    Ok(ToolInvocationResult {
                        body: Some(mcp_response_to_json(&resp)),
                        error: None,
                    })
                }
                other => Err(RuntimeError::Internal {
                    message: format!("unknown conformance tool {other:?}"),
                }),
            }
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }
}

/// Map an MCP `tools/call` response to a deterministic JSON body.
///
/// Mirrors `McpBackedTool`'s content handling
/// (`crates/tau-mcp-tokio/src/bridge.rs`): text blocks join in order;
/// non-text blocks degrade to a stable placeholder so the body is
/// byte-deterministic across profiles. The joined text is then parsed
/// as JSON (so a server returning `{"temp":22}` lands as an object),
/// falling back to a JSON string when the text is not valid JSON.
fn mcp_response_to_json(resp: &ToolsCallResponse) -> Value {
    let joined: String = resp
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::Image { mime_type, .. } => {
                format!("[image/{mime_type} — not rendered in v0]")
            }
            ContentBlock::Resource { .. } => "[resource — not rendered in v0]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("");

    match serde_json::from_str::<Value>(joined.trim()) {
        Ok(v) => v,
        Err(_) => Value::String(joined),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tau_runtime_core::interpreter::tool_dispatch::ToolDispatcher;

    #[tokio::test(flavor = "current_thread")]
    async fn native_tools_return_deterministic_bodies() {
        let llm = Arc::new(crate::sequenced_llm::SequencedLlm::new("mock-llm", vec![]));
        let d = ConformanceDispatcher::new_native_only(llm);
        let r = d
            .invoke(&tau_ir::ToolId("read_temp".into()), &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(r.body, Some(serde_json::json!(32)));
        let r = d
            .invoke(
                &tau_ir::ToolId("set_fan".into()),
                &serde_json::json!({"on":true}),
            )
            .await
            .unwrap();
        assert_eq!(r.body, Some(serde_json::json!({"ok": true})));
        assert!(d
            .invoke(&tau_ir::ToolId("nope".into()), &serde_json::json!({}))
            .await
            .is_err());
    }

    #[test]
    fn mcp_response_text_blocks_join_and_parse_json() {
        let resp = ToolsCallResponse {
            content: vec![ContentBlock::Text {
                text: "{\"forecast\":\"sunny\"}".to_string(),
            }],
            is_error: None,
        };
        assert_eq!(
            mcp_response_to_json(&resp),
            serde_json::json!({"forecast": "sunny"})
        );
    }

    #[test]
    fn mcp_response_plain_text_falls_back_to_string() {
        let resp = ToolsCallResponse {
            content: vec![ContentBlock::Text {
                text: "sunny".to_string(),
            }],
            is_error: None,
        };
        assert_eq!(
            mcp_response_to_json(&resp),
            serde_json::Value::String("sunny".to_string())
        );
    }
}
