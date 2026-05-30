//! Integration test: [`Runtime::run_with_history`] threads a prior
//! conversation into the LLM-call shape before the new
//! `initial_message`.
//!
//! Scenario: simulate REPL turn 3 — two prior turns (user → agent →
//! user → agent — wait, the history we thread in is whatever messages
//! the REPL has accumulated; here we use one user + one agent message)
//! plus a fresh user prompt. We assert the kernel forwards all three to
//! the LLM as `CompletionRequest.messages` with the new prompt last.
//!
//! Uses a hand-rolled `RecordingLlm` (instead of `MockLlmBackend`) so
//! we can inspect the exact `CompletionRequest` the run loop hands to
//! the backend. `MockLlmBackend::invocations` records the `Vec<Message>`
//! shape but not the projected `LlmProviderMessage` list, which is the
//! observable behavior under test here.

mod common;

use std::sync::{Arc, Mutex};

use tau_domain::{Address, Message, MessagePayload};
use tau_ports::fixtures::make_completion_response;
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, StopReason,
};
use tau_runtime::{RunOptions, RunOutcome, Runtime};

use assert_matches::assert_matches;

/// LLM backend that records every `CompletionRequest` for inspection
/// and replies with a single canned `CompletionResponse`.
#[derive(Clone)]
struct RecordingLlm {
    name: String,
    response: CompletionResponse,
    invocations: Arc<Mutex<Vec<CompletionRequest>>>,
}

impl LlmBackend for RecordingLlm {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.invocations
            .lock()
            .expect("RecordingLlm mutex poisoned")
            .push(req);
        Ok(self.response.clone())
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        // run_with_history now delegates to run_streaming_with_history,
        // so stream() is the live path. Record the request (same as
        // complete()) and return the canned response as a batch stream.
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

#[tokio::test]
async fn run_with_history_threads_prior_messages() {
    // Canned response: a single text block, no tool_uses, EndTurn —
    // the simplest possible turn so we can isolate the history-threading
    // assertion without chasing tool dispatch behavior.
    let resp = make_completion_response(
        "follow-up answer".into(),
        Vec::new(),
        StopReason::EndTurn,
        None,
    );
    let invocations = Arc::new(Mutex::new(Vec::<CompletionRequest>::new()));
    let llm = RecordingLlm {
        name: "gpt-4".into(),
        response: resp,
        invocations: invocations.clone(),
    };

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .build()
        .expect("build runtime");

    // Two prior turns of history (user → agent). The fixture
    // `agent_address()` mints a stable `AgentInstanceId` so both prior
    // messages are clearly assistant-authored.
    let agent_addr = common::agent_address();
    let history = vec![
        Message::new(
            Address::User,
            agent_addr.clone(),
            MessagePayload::Text {
                content: "First question".into(),
            },
        ),
        Message::new(
            agent_addr.clone(),
            Address::User,
            MessagePayload::Text {
                content: "First answer".into(),
            },
        ),
    ];

    // New user prompt for turn 3.
    let initial = Message::new(
        Address::User,
        agent_addr,
        MessagePayload::Text {
            content: "Follow-up".into(),
        },
    );

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();

    let outcome = runtime
        .run_with_history(agent_def, manifest, history, initial, common::run_options())
        .await
        .expect("run succeeded");

    // Outcome shape: a single LLM call (no tool_uses) → Completed in one
    // turn. The history was preloaded for that single call.
    assert_matches!(
        outcome,
        RunOutcome::Completed { total_turns, .. } => {
            assert_eq!(total_turns, 1, "single LLM call, no tool_uses");
        }
    );

    // The kernel must have forwarded all three messages to the LLM:
    // 2 prior history entries + 1 new initial_message, in that order.
    let recorded = invocations.lock().expect("RecordingLlm mutex poisoned");
    assert_eq!(recorded.len(), 1, "exactly one LLM call for this turn");
    let request = &recorded[0];
    assert_eq!(
        request.messages.len(),
        3,
        "history threading: 2 prior + 1 new initial_message",
    );
}

#[tokio::test]
async fn run_calls_run_with_history_with_empty_history() {
    // Smoke test: `Runtime::run` is now a thin wrapper over
    // `run_with_history` with `history = vec![]`. Verify the wrapper
    // produces a single LLM call whose `messages` contains exactly the
    // initial user prompt — proving the empty-history pre-load is a
    // no-op and existing callers keep working unchanged.
    let resp = make_completion_response("hello".into(), Vec::new(), StopReason::EndTurn, None);
    let invocations = Arc::new(Mutex::new(Vec::<CompletionRequest>::new()));
    let llm = RecordingLlm {
        name: "gpt-4".into(),
        response: resp,
        invocations: invocations.clone(),
    };

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("Hi");

    let outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    assert_matches!(
        outcome,
        RunOutcome::Completed { total_turns, .. } => {
            assert_eq!(total_turns, 1);
        }
    );

    let recorded = invocations.lock().expect("RecordingLlm mutex poisoned");
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].messages.len(),
        1,
        "Runtime::run with empty history forwards only the initial message",
    );
}
