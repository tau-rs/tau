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
//! - `canned_tool_call: Option<{ name, input }>` — on the FIRST
//!   `llm.complete` call only, emit a tool-use block for `name` with
//!   `input` and `StopReason::ToolUse`. The agent loop dispatches the
//!   tool and calls back; turn 2 returns text as usual, ending the run.
//!   Lets a test drive a real tool round trip without a live provider.
//!   Checked only when `tool_calls` has no scripted entry for that turn
//!   (`tool_calls` takes precedence when both are configured).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::Deserialize;
use tau_plugin_sdk::{run_llm_backend_with_config, ConfigError, Configure, SdkError};
use tau_ports::{
    batch_to_stream,
    fixtures::{make_completion_response, make_tool_use},
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, StopReason,
    ToolUse,
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
    /// Emit this tool call on the first `llm.complete` turn (checked only
    /// when `tool_calls` has no scripted entry for that turn).
    #[serde(default)]
    canned_tool_call: Option<CannedToolCall>,
}

/// A tool-use block for the plugin to emit on its first completion turn.
#[derive(Debug, Deserialize)]
struct CannedToolCall {
    /// Tool name — must match a `ToolSpec::name` the agent was given.
    name: String,
    /// Arguments passed to the tool. Defaults to `null`.
    #[serde(default = "null_value")]
    input: tau_domain::Value,
}

/// `tau_domain::Value` has no `Default` impl; serde needs one for `input`.
fn null_value() -> tau_domain::Value {
    tau_domain::Value::Null
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
    /// turn's response shape: `(turn_index, text, tool_uses, stop_reason)`.
    /// Shared by `complete` and `stream` via [`Self::next_response`].
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
    ) -> Result<(usize, String, Vec<ToolUse>, StopReason), LlmError> {
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
        let text = self
            .config
            .script
            .get(i)
            .cloned()
            .unwrap_or_else(|| self.config.canned_text.clone());
        if scripted_calls.is_empty() {
            Ok((i, text, Vec::new(), StopReason::EndTurn))
        } else {
            let tool_uses = scripted_calls
                .into_iter()
                .enumerate()
                .map(|(j, call)| {
                    let tool_name = call.name.clone();
                    let input = serde_json::from_value::<tau_domain::Value>(call.args)
                        .unwrap_or_else(|e| {
                            panic!(
                                "echo-llm: tool_calls[{i}][{j}] for tool {tool_name:?} has \
                                 args that failed to deserialize into tau_domain::Value: {e}"
                            )
                        });
                    ToolUse::new(format!("echo_tool_{i}_{j}"), call.name, input)
                })
                .collect();
            Ok((i, String::new(), tool_uses, StopReason::ToolUse))
        }
    }

    /// The full canned response for one turn. `complete` and `stream`
    /// MUST share this: the runtime drives `stream`
    /// (`tau-runtime-core/src/stream.rs`), so a tool call wired only into
    /// `complete` would never reach an agent loop.
    ///
    /// `canned_tool_call` fires on the first turn only — the agent loop
    /// dispatches the tool and calls back, and turn 2 returns plain text
    /// to end the run. Emitting it every turn would spin to the turn cap.
    /// Checked only when `tool_calls` produced no scripted call for that
    /// turn — `tool_calls` takes precedence when both are configured.
    async fn next_response(&self, method: &str) -> Result<CompletionResponse, LlmError> {
        let (turn, text, tool_uses, stop_reason) = self.next_turn(method).await?;
        if !tool_uses.is_empty() {
            return Ok(make_completion_response(text, tool_uses, stop_reason, None));
        }
        match (turn, self.config.canned_tool_call.as_ref()) {
            (0, Some(call)) => Ok(make_completion_response(
                text,
                vec![make_tool_use(
                    "echo-llm-tool-use-0".to_string(),
                    call.name.clone(),
                    call.input.clone(),
                )],
                StopReason::ToolUse,
                None,
            )),
            _ => Ok(make_completion_response(text, Vec::new(), stop_reason, None)),
        }
    }
}

impl LlmBackend for EchoLlm {
    fn name(&self) -> &str {
        "echo-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.next_response("llm.complete").await
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.next_response("llm.stream").await?;
        Ok(batch_to_stream(resp))
    }
}

#[tokio::main]
async fn main() -> Result<(), SdkError> {
    run_llm_backend_with_config::<EchoLlm>(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) — additivity guarantee (#631 I4): a config that sets neither
    /// `tool_calls` nor `canned_tool_call` — every config written before
    /// this branch — must still produce exactly today's shape: the
    /// scripted/canned text, no tool calls, `StopReason::EndTurn`. This
    /// branch's whole premise is that existing configs behave
    /// byte-identically; this test is what actually checks that premise
    /// rather than asserting it in a doc comment.
    #[tokio::test]
    async fn empty_config_yields_canned_text_no_tools_end_turn() {
        let echo = EchoLlm::from_config(EchoConfig {
            canned_text: "hello".to_string(),
            ..Default::default()
        })
        .unwrap();

        let (turn, text, tool_uses, stop_reason) = echo.next_turn("llm.complete").await.unwrap();
        assert_eq!(turn, 0);
        assert_eq!(text, "hello");
        assert!(tool_uses.is_empty());
        assert_eq!(stop_reason, StopReason::EndTurn);
    }

    /// (b) — a scripted turn 0 yields a `ToolUse` with the right
    /// name/args and `StopReason::ToolUse`.
    #[tokio::test]
    async fn scripted_turn_zero_yields_a_tool_use() {
        let echo = EchoLlm::from_config(EchoConfig {
            tool_calls: vec![vec![ScriptedToolCall {
                name: "weather.get_forecast".to_string(),
                args: serde_json::json!({"location": "Paris"}),
            }]],
            ..Default::default()
        })
        .unwrap();

        let (turn, text, tool_uses, stop_reason) = echo.next_turn("llm.complete").await.unwrap();
        assert_eq!(turn, 0);
        assert_eq!(text, "");
        assert_eq!(stop_reason, StopReason::ToolUse);
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].name, "weather.get_forecast");
        let expected_input: tau_domain::Value =
            serde_json::from_value(serde_json::json!({"location": "Paris"})).unwrap();
        assert_eq!(tool_uses[0].input, expected_input);
    }

    /// (c) — turn 1 falls back to text/`EndTurn`, which is what
    /// terminates the agent loop (only turn 0 is scripted above).
    #[tokio::test]
    async fn turn_after_scripted_call_falls_back_to_text_end_turn() {
        let echo = EchoLlm::from_config(EchoConfig {
            tool_calls: vec![vec![ScriptedToolCall {
                name: "weather.get_forecast".to_string(),
                args: serde_json::Value::Null,
            }]],
            canned_text: "done".to_string(),
            ..Default::default()
        })
        .unwrap();

        // Turn 0: consumes the scripted call.
        let _ = echo.next_turn("llm.complete").await.unwrap();
        // Turn 1: tool_calls has no entry for this index -> falls back
        // exactly like a config that never sets tool_calls at all.
        let (turn, text, tool_uses, stop_reason) = echo.next_turn("llm.complete").await.unwrap();
        assert_eq!(turn, 1);
        assert_eq!(text, "done");
        assert!(tool_uses.is_empty());
        assert_eq!(stop_reason, StopReason::EndTurn);
    }
}
