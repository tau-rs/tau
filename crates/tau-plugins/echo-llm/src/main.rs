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
//! - `canned_tool_call: Option<{ name, input }>` — on the FIRST
//!   `llm.complete` call only, emit a tool-use block for `name` with
//!   `input` and `StopReason::ToolUse`. The agent loop dispatches the
//!   tool and calls back; turn 2 returns text as usual, ending the run.
//!   Lets a test drive a real tool round trip without a live provider.

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
};

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
    /// Emit this tool call on the first `llm.complete` turn.
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
    /// canned text plus the 0-based turn index it consumed. Shared by
    /// `complete` and `stream` via [`Self::next_response`].
    async fn next_text(&self, method: &str) -> Result<(usize, String), LlmError> {
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
        let text = self
            .config
            .script
            .get(i)
            .cloned()
            .unwrap_or_else(|| self.config.canned_text.clone());
        Ok((i, text))
    }

    /// The full canned response for one turn. `complete` and `stream`
    /// MUST share this: the runtime drives `stream`
    /// (`tau-runtime-core/src/stream.rs`), so a tool call wired only into
    /// `complete` would never reach an agent loop.
    ///
    /// `canned_tool_call` fires on the first turn only — the agent loop
    /// dispatches the tool and calls back, and turn 2 returns plain text
    /// to end the run. Emitting it every turn would spin to the turn cap.
    async fn next_response(&self, method: &str) -> Result<CompletionResponse, LlmError> {
        let (turn, text) = self.next_text(method).await?;
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
            _ => Ok(make_completion_response(
                text,
                Vec::new(),
                StopReason::EndTurn,
                None,
            )),
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
