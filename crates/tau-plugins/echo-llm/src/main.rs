//! Toy `LlmBackend` plugin that replays canned responses from config.
//!
//! Used by tau-cli integration tests to exercise the plugin loading
//! mechanism end-to-end without depending on a real LLM provider.
//!
//! # Configuration
//!
//! Configurable via the handshake `config` field (set in
//! `[agents.<id>.config]` of the project tau.toml):
//!
//! - `canned_text: String` — single canned text returned by every
//!   `llm.complete` call. Default: empty string.
//! - `script: Vec<String>` — multi-turn script. Indexed by an internal
//!   atomic counter that increments on each `complete` call. If the
//!   counter exceeds the script length, falls back to `canned_text`.
//! - `crash_after_handshake: bool` — if `true`, panic at the start of
//!   any `complete`/`stream` call. The handshake itself completes; the
//!   panic surfaces at first dispatch. Used by failure-path tests.
//! - `delay_response_ms: Option<u64>` — sleep this many milliseconds
//!   before responding. Used by handshake/timeout tests.
//! - `error_on_method: Option<String>` — return `Err(LlmError::Internal)`
//!   when this method is called (e.g. `"llm.complete"` or
//!   `"llm.stream"`).
//! - `tool_calls: Vec<Vec<ScriptedToolCall>>` — per-turn scripted tool
//!   calls, indexed by the SAME atomic turn counter as `script`. When
//!   turn `i` has a non-empty entry, that turn returns
//!   `StopReason::ToolUse` with those calls (and empty text) instead of
//!   the usual `script`/`canned_text` + `StopReason::EndTurn`. Turns
//!   with no scripted calls (including every turn of a config that
//!   never sets this field — the default is an empty `Vec`) are exactly
//!   today's text-only behavior. Each entry is `{ name, args }`; `args`
//!   defaults to `null`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::Deserialize;
use tau_plugin_sdk::{run_llm_backend_with_config, ConfigError, Configure, SdkError};
use tau_ports::{
    batch_to_stream, fixtures::make_completion_response, CompletionRequest, CompletionResponse,
    CompletionStream, LlmBackend, LlmError, StopReason, ToolUse,
};

/// One scripted tool call for a turn (config-driven).
#[derive(Debug, Clone, Default, Deserialize)]
struct ScriptedToolCall {
    /// Tool name the mock backend claims to call (e.g.
    /// `"weather.get_forecast"`).
    #[serde(default)]
    name: String,
    /// Tool arguments, passed through as-is. Defaults to `null`.
    #[serde(default)]
    args: serde_json::Value,
}

/// Static configuration consumed from the handshake `config` field.
#[derive(Debug, Default, Deserialize)]
struct EchoConfig {
    /// Single canned text returned by every `llm.complete` call.
    #[serde(default)]
    canned_text: String,
    /// Multi-turn script; indexed by an atomic turn counter.
    #[serde(default)]
    script: Vec<String>,
    /// If `true`, panic at the start of any `complete`/`stream` call.
    #[serde(default)]
    crash_after_handshake: bool,
    /// Sleep this many milliseconds before responding. Used by
    /// handshake/timeout tests.
    #[serde(default)]
    delay_response_ms: Option<u64>,
    /// Return `Err(LlmError::Internal)` when this method is called.
    #[serde(default)]
    error_on_method: Option<String>,
    /// Per-turn scripted tool calls; see the module docs. Empty (the
    /// default) preserves exactly today's text-only output for every
    /// existing config.
    #[serde(default)]
    tool_calls: Vec<Vec<ScriptedToolCall>>,
}

/// Toy `LlmBackend` plugin.
struct EchoLlm {
    config: EchoConfig,
    turn: AtomicUsize,
}

impl Configure for EchoLlm {
    type Config = EchoConfig;

    fn from_config(config: Self::Config) -> Result<Self, ConfigError> {
        Ok(EchoLlm {
            config,
            turn: AtomicUsize::new(0),
        })
    }
}

impl EchoLlm {
    /// Apply the test-only side effects (`crash_after_handshake`,
    /// `error_on_method`, `delay_response_ms`) and produce the next
    /// turn's response shape: `(text, tool_uses, stop_reason)`. Shared
    /// by `complete` and `stream`.
    ///
    /// Reads the turn counter exactly ONCE (`fetch_add`) so the
    /// `tool_calls` and `script` lookups for this turn stay
    /// index-synchronized — a second `fetch_add` here would silently
    /// desync the two scripts.
    ///
    /// When `tool_calls[i]` is non-empty, this turn returns those calls
    /// with `StopReason::ToolUse` and empty text — mirroring how a real
    /// provider replies to a tool-use turn. Otherwise (including every
    /// turn of a config that never sets `tool_calls`) it falls back to
    /// exactly today's behavior: `script[i]` (or `canned_text`) with
    /// `StopReason::EndTurn`.
    async fn next_turn(
        &self,
        method: &str,
    ) -> Result<(String, Vec<ToolUse>, StopReason), LlmError> {
        if self.config.crash_after_handshake {
            panic!("echo-llm crash_after_handshake = true (test-only mode)");
        }
        if self.config.error_on_method.as_deref() == Some(method) {
            return Err(LlmError::Internal {
                message: format!("echo-llm error_on_method test mode tripped on {method}"),
            });
        }
        if let Some(ms) = self.config.delay_response_ms {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let i = self.turn.fetch_add(1, Ordering::Relaxed);

        let scripted_calls = self.config.tool_calls.get(i).cloned().unwrap_or_default();
        if scripted_calls.is_empty() {
            let text = self
                .config
                .script
                .get(i)
                .cloned()
                .unwrap_or_else(|| self.config.canned_text.clone());
            Ok((text, Vec::new(), StopReason::EndTurn))
        } else {
            let tool_uses = scripted_calls
                .into_iter()
                .enumerate()
                .map(|(j, call)| {
                    let input =
                        serde_json::from_value(call.args).unwrap_or(tau_domain::Value::Null);
                    ToolUse::new(format!("echo_tool_{i}_{j}"), call.name, input)
                })
                .collect();
            Ok((String::new(), tool_uses, StopReason::ToolUse))
        }
    }
}

impl LlmBackend for EchoLlm {
    fn name(&self) -> &str {
        "echo-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let (text, tool_uses, stop_reason) = self.next_turn("llm.complete").await?;
        Ok(make_completion_response(text, tool_uses, stop_reason, None))
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let (text, tool_uses, stop_reason) = self.next_turn("llm.stream").await?;
        let resp = make_completion_response(text, tool_uses, stop_reason, None);
        Ok(batch_to_stream(resp))
    }
}

#[tokio::main]
async fn main() -> Result<(), SdkError> {
    run_llm_backend_with_config::<EchoLlm>(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).await
}
