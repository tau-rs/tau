//! Multi-turn `MockLlmBackend` fixture for tau-runtime integration tests.
//!
//! Skills-4 foundational test fixture (D3). Lifts the `ScriptedLlm` pattern
//! from `run_with_tool_calls.rs` into `common/` for reuse across the Skills-4
//! e2e tests (T8) and the 5 un-ignored pattern tests (T9).
//!
//! # Builder API
//!
//! ```ignore
//! let backend = MockLlmBackend::new("parent-llm")
//!     .add_tool_call("skill.critic.spawn", serde_json::json!({"message": "review"}))
//!     .add_text("critic returned: ...")
//!     .add_end();
//! ```
//!
//! For a single LLM turn that emits multiple `ToolUse` blocks in one
//! response, use [`MockLlmBackend::add_tool_calls`] (variant
//! [`MockTurn::ToolCalls`]).
//!
//! # Improvement over `ScriptedLlm`
//!
//! `ScriptedLlm` takes a pre-built `Vec<CompletionResponse>` — callers must
//! construct `#[non_exhaustive]` types via the fixtures helpers.
//! `MockLlmBackend` provides a typed `MockTurn` enum so callers express intent
//! at a higher level (text response / tool call / end of script).
//! The struct itself converts `MockTurn` → `CompletionResponse` internally.

use std::collections::VecDeque;
use std::sync::Mutex;

use tau_domain::Value;
use tau_ports::fixtures::{make_completion_response, make_token_usage};
use tau_ports::{
    batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError,
    StopReason, ToolUse,
};

/// A scripted turn for `MockLlmBackend`.
///
/// Each `MockTurn` maps to one call to `LlmBackend::complete` or
/// `LlmBackend::stream`.
#[derive(Debug, Clone)]
pub enum MockTurn {
    /// Plain text response with no tool use. Produces
    /// `StopReason::EndTurn`.
    Text {
        /// The text the model "returns".
        text: String,
    },
    /// A tool-call response. Produces `StopReason::ToolUse`.
    ToolCall {
        /// Tool name (e.g. `"skill.critic.spawn"`).
        name: String,
        /// Tool arguments.
        args: Value,
    },
    /// A multi-tool-call response: a single LLM completion containing
    /// several `ToolUse` blocks. Produces one `CompletionResponse` whose
    /// `tool_uses` vector has one entry per `(name, args)` pair, with
    /// `StopReason::ToolUse`.
    ///
    /// Used to exercise code paths that fan out over multiple tool
    /// blocks in a single turn (e.g. spec §3.9
    /// `llm.tool_use_emitted` firing once per block).
    ToolCalls {
        /// Ordered list of `(tool_name, args)` pairs. Each becomes one
        /// `ToolUse` in the synthesized response, with id `tu_<name>_<idx>`.
        calls: Vec<(String, Value)>,
    },
    /// End-of-script marker: returns an empty text response with
    /// `StopReason::EndTurn`. Callers add this to make it explicit that
    /// no more turns are expected (helps with debugging exhaustion
    /// errors).
    End,
}

/// Multi-turn scripted LLM backend for integration tests.
///
/// Returns scripted turns in FIFO order. Each `complete()` / `stream()`
/// call pops one `MockTurn` and records the incoming `CompletionRequest`.
///
/// Returns `LlmError::Internal` if the script is exhausted.
///
/// `Send + Sync` via `Mutex` interior mutability — safe to hand to a
/// `Runtime` that stores `Arc<dyn DynLlmBackend>`.
#[derive(Debug)]
pub struct MockLlmBackend {
    name: String,
    turns: Mutex<VecDeque<MockTurn>>,
    received_requests: Mutex<Vec<CompletionRequest>>,
}

impl MockLlmBackend {
    /// Create a new backend with the given name and an empty turn queue.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            turns: Mutex::new(VecDeque::new()),
            received_requests: Mutex::new(Vec::new()),
        }
    }

    /// Push a scripted turn onto the queue. Returns `self` for chaining.
    pub fn add_turn(self, turn: MockTurn) -> Self {
        self.turns
            .lock()
            .expect("MockLlmBackend turns mutex poisoned")
            .push_back(turn);
        self
    }

    /// Convenience: add a plain-text turn.
    pub fn add_text(self, text: &str) -> Self {
        self.add_turn(MockTurn::Text {
            text: text.to_string(),
        })
    }

    /// Convenience: add a tool-call turn.
    pub fn add_tool_call(self, name: &str, args: Value) -> Self {
        self.add_turn(MockTurn::ToolCall {
            name: name.to_string(),
            args,
        })
    }

    /// Convenience: add a single turn whose response contains multiple
    /// `ToolUse` blocks. Each `(name, args)` pair becomes one ToolUse
    /// in the synthesized `CompletionResponse`.
    pub fn add_tool_calls<S, I>(self, calls: I) -> Self
    where
        S: Into<String>,
        I: IntoIterator<Item = (S, Value)>,
    {
        let calls = calls
            .into_iter()
            .map(|(name, args)| (name.into(), args))
            .collect();
        self.add_turn(MockTurn::ToolCalls { calls })
    }

    /// Convenience: add an explicit end-of-script turn.
    pub fn add_end(self) -> Self {
        self.add_turn(MockTurn::End)
    }

    /// Return all `CompletionRequest`s this backend has received, in
    /// order. Useful for asserting what the parent LLM saw at each turn
    /// (e.g. that a tool result was appended to the context).
    pub fn received_requests(&self) -> Vec<CompletionRequest> {
        self.received_requests
            .lock()
            .expect("MockLlmBackend received_requests mutex poisoned")
            .clone()
    }

    /// Assert the backend has been invoked exactly `expected` times.
    ///
    /// Call before the test ends to catch tests that silently never
    /// exercise the mock (a class of false positive where the assertion
    /// passes because the code under test never reached the LLM call
    /// site). `#[track_caller]` so the panic points at the test, not
    /// inside this helper.
    #[track_caller]
    pub fn verify_invocation_count(&self, expected: usize) {
        let actual = self
            .received_requests
            .lock()
            .expect("MockLlmBackend received_requests mutex poisoned")
            .len();
        assert_eq!(
            actual, expected,
            "MockLlmBackend({}) invocation count mismatch: expected {}, got {}",
            self.name, expected, actual,
        );
    }

    /// Assert the scripted turn queue was fully consumed (no leftover
    /// scripted turns). Catches tests that exit early or over-provision
    /// the script — either way a sign the script and the code under
    /// test have drifted apart.
    #[track_caller]
    pub fn verify_fully_consumed(&self) {
        let remaining = self
            .turns
            .lock()
            .expect("MockLlmBackend turns mutex poisoned")
            .len();
        assert_eq!(
            remaining, 0,
            "MockLlmBackend({}) had {} scripted turns left unconsumed — \
             test exited early or script is over-provisioned",
            self.name, remaining,
        );
    }

    /// Record the incoming request + pop the next scripted turn.
    fn pop_next(&self, req: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        // Record first (even on exhaustion, to aid debugging).
        self.received_requests
            .lock()
            .expect("MockLlmBackend received_requests mutex poisoned")
            .push(req.clone());

        let turn = self
            .turns
            .lock()
            .expect("MockLlmBackend turns mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: format!("MockLlmBackend({:?}): script exhausted", self.name),
            })?;

        Ok(match turn {
            MockTurn::Text { text } => make_completion_response(
                text,
                vec![],
                StopReason::EndTurn,
                Some(make_token_usage(10, 10)),
            ),
            MockTurn::ToolCall { name, args } => {
                let id = format!("tu_{name}");
                make_completion_response(
                    String::new(),
                    vec![ToolUse::new(id, name, args)],
                    StopReason::ToolUse,
                    Some(make_token_usage(10, 10)),
                )
            }
            MockTurn::ToolCalls { calls } => {
                let tool_uses = calls
                    .into_iter()
                    .enumerate()
                    .map(|(idx, (name, args))| {
                        let id = format!("tu_{name}_{idx}");
                        ToolUse::new(id, name, args)
                    })
                    .collect();
                make_completion_response(
                    String::new(),
                    tool_uses,
                    StopReason::ToolUse,
                    Some(make_token_usage(10, 10)),
                )
            }
            MockTurn::End => make_completion_response(
                String::new(),
                vec![],
                StopReason::EndTurn,
                Some(make_token_usage(1, 0)),
            ),
        })
    }
}

impl LlmBackend for MockLlmBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.pop_next(&req)
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.pop_next(&req)?;
        // `batch_to_stream` converts a CompletionResponse into a stream
        // that yields: optional Text chunk → zero-or-more ToolUse chunks
        // → terminal Finish chunk. This is the correct shape for the
        // streaming run loop.
        Ok(batch_to_stream(resp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turns are popped FIFO: first added, first returned.
    #[tokio::test]
    async fn pops_turns_in_order() {
        let backend = MockLlmBackend::new("test")
            .add_text("first")
            .add_text("second")
            .add_end();

        let req = CompletionRequest::new("test".to_string());

        let r1 = backend.complete(req.clone()).await.unwrap();
        assert_eq!(r1.text, "first", "expected first turn");
        assert_eq!(r1.stop_reason, StopReason::EndTurn);

        let r2 = backend.complete(req.clone()).await.unwrap();
        assert_eq!(r2.text, "second", "expected second turn");

        // End turn: empty text, EndTurn.
        let r3 = backend.complete(req.clone()).await.unwrap();
        assert_eq!(r3.text, "", "End turn should produce empty text");
        assert_eq!(r3.stop_reason, StopReason::EndTurn);
    }

    /// Each call records the incoming request.
    #[tokio::test]
    async fn records_received_requests() {
        let backend = MockLlmBackend::new("test").add_text("ok");

        let mut req = CompletionRequest::new("test-model".to_string());
        req.system = Some("system prompt".to_string());

        backend.complete(req.clone()).await.unwrap();

        let recorded = backend.received_requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].model, "test-model");
        assert_eq!(recorded[0].system, Some("system prompt".to_string()));
    }

    /// After the script is exhausted, `complete` returns `LlmError::Internal`.
    #[tokio::test]
    async fn errors_on_exhaustion() {
        let backend = MockLlmBackend::new("test").add_text("only turn");

        let req = CompletionRequest::new("test".to_string());

        // First call consumes the only turn.
        backend.complete(req.clone()).await.unwrap();

        // Second call: script exhausted.
        let result = backend.complete(req.clone()).await;
        assert!(
            matches!(result, Err(LlmError::Internal { .. })),
            "expected Internal error on exhaustion, got {result:?}"
        );
    }

    /// `verify_invocation_count` passes on the exact count, panics
    /// otherwise.
    #[tokio::test]
    async fn verify_invocation_count_happy_path() {
        let backend = MockLlmBackend::new("test").add_text("a").add_text("b");
        let req = CompletionRequest::new("m".to_string());

        backend.verify_invocation_count(0); // never called yet

        backend.complete(req.clone()).await.unwrap();
        backend.verify_invocation_count(1);

        backend.complete(req.clone()).await.unwrap();
        backend.verify_invocation_count(2);
    }

    /// `verify_invocation_count` panics with a useful message when the
    /// count is wrong.
    #[tokio::test]
    #[should_panic(expected = "invocation count mismatch: expected 3, got 1")]
    async fn verify_invocation_count_panics_on_mismatch() {
        let backend = MockLlmBackend::new("test").add_text("one");
        backend
            .complete(CompletionRequest::new("m".to_string()))
            .await
            .unwrap();
        backend.verify_invocation_count(3);
    }

    /// `verify_fully_consumed` passes after every scripted turn has
    /// been popped.
    #[tokio::test]
    async fn verify_fully_consumed_happy_path() {
        let backend = MockLlmBackend::new("test").add_text("one").add_end();
        let req = CompletionRequest::new("m".to_string());
        backend.complete(req.clone()).await.unwrap();
        backend.complete(req.clone()).await.unwrap();
        backend.verify_fully_consumed();
    }

    /// `add_tool_calls` produces a single response whose `tool_uses`
    /// vector has one entry per `(name, args)` pair, in order, with
    /// `StopReason::ToolUse`.
    #[tokio::test]
    async fn add_tool_calls_produces_multi_tool_response() {
        let backend = MockLlmBackend::new("test").add_tool_calls(vec![
            ("alpha", Value::String("a".into())),
            ("beta", Value::String("b".into())),
        ]);
        let req = CompletionRequest::new("m".to_string());
        let resp = backend.complete(req).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.tool_uses.len(), 2);
        assert_eq!(resp.tool_uses[0].name, "alpha");
        assert_eq!(resp.tool_uses[1].name, "beta");
        // Ids are unique-per-index to avoid collisions when the same
        // tool name appears twice in one turn.
        assert_ne!(resp.tool_uses[0].id, resp.tool_uses[1].id);
    }

    /// `verify_fully_consumed` panics when scripted turns remain.
    #[tokio::test]
    #[should_panic(expected = "scripted turns left unconsumed")]
    async fn verify_fully_consumed_panics_on_leftover() {
        let backend = MockLlmBackend::new("test")
            .add_text("only one of two will be consumed")
            .add_text("this one is leftover");
        backend
            .complete(CompletionRequest::new("m".to_string()))
            .await
            .unwrap();
        backend.verify_fully_consumed();
    }
}
