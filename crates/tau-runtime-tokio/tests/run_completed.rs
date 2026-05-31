//! Integration test: agent completes a single turn with a text
//! response (no tool_uses) and the run loop returns
//! `RunOutcome::Completed`.

mod common;

use std::sync::Arc;

use tau_domain::MessagePayload;
use tau_ports::fixtures::{make_completion_response, make_token_usage, MockLlmBackend};
use tau_ports::StopReason;
use tau_runtime_tokio::{RunOutcome, Runtime};

use assert_matches::assert_matches;

#[tokio::test]
async fn run_completes_with_text_response() {
    // 1. Canned LLM response: a single text block, no tool_uses, EndTurn.
    let resp = make_completion_response(
        "hello world".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(5, 10)),
    );
    // Wrap in Arc so we can keep an observation handle after the
    // runtime consumes the backend, and `verify_invocation_count` the
    // mock at the end of the test.
    let llm = Arc::new(MockLlmBackend::new("gpt-4").with_response(resp));

    // 2. Build the runtime with just the LLM backend.
    let runtime = Runtime::builder()
        .with_dyn_llm_backend(llm.clone())
        .build()
        .expect("build runtime");

    // 3. Compose the agent definition + manifest + initial message.
    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("Hi");

    // 4. Run.
    let outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    // 5. Assert the happy-path shape.
    assert_matches!(
        outcome,
        RunOutcome::Completed {
            final_message,
            all_messages,
            total_turns,
            token_usage,
            ..
        } => {
            assert_eq!(total_turns, 1, "single LLM call, no tool_uses");
            assert_eq!(
                all_messages.len(),
                2,
                "initial user msg + assistant text response"
            );
            assert_matches!(
                &final_message.payload,
                MessagePayload::Text { content } => {
                    assert_eq!(content, "hello world");
                }
            );
            assert_eq!(token_usage.input_tokens, 5);
            assert_eq!(token_usage.output_tokens, 10);
        }
    );

    // The LLM mock should have been invoked exactly once for the single
    // text-only turn (no tool_uses → no follow-up turn).
    llm.verify_invocation_count(1);
}
