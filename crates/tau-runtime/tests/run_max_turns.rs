//! Integration test: agent loop exceeds the `max_turns` cap when the
//! LLM keeps emitting tool_uses indefinitely. The run loop returns
//! `Ok(RunOutcome::Failed { kind: OutOfResources })`.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tau_domain::{AgentStatus, FailureKind, Value};
use tau_ports::fixtures::{make_completion_response, make_token_usage, make_tool_use, MockTool};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, StopReason,
};
use tau_runtime::{RunOutcome, Runtime};

use assert_matches::assert_matches;

/// LLM that always returns a single canned response with a tool_use,
/// no matter how many times it's called. Combined with a permissive
/// tool, this makes the run loop iterate forever — bounded only by
/// `RunOptions::max_turns`.
struct InfiniteToolUseLlm {
    name: String,
    response: CompletionResponse,
    calls: Arc<AtomicU32>,
}

impl LlmBackend for InfiniteToolUseLlm {
    fn name(&self) -> &str {
        &self.name
    }
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

#[tokio::test]
async fn max_turns_exceeded_returns_out_of_resources() {
    // Always-tool_use response; never EndTurn.
    let response = make_completion_response(
        String::new(),
        vec![make_tool_use("u1".into(), "noop".into(), Value::Null)],
        StopReason::ToolUse,
        Some(make_token_usage(1, 1)),
    );
    let calls = Arc::new(AtomicU32::new(0));
    let llm = InfiniteToolUseLlm {
        name: "gpt-4".into(),
        response,
        calls: calls.clone(),
    };

    // Default-config `MockTool` returns `Ok(ToolResult { content: vec![], is_error: false })`
    // and declares no required capabilities, so dispatch always succeeds.
    let noop_tool = MockTool::new("noop", common::empty_tool_spec("noop"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(noop_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("loop forever");

    // Cap the loop at 3. `RunOptions` is `#[non_exhaustive]`, so we
    // build via Default + field mutation.
    let options = {
        let mut o = common::run_options();
        o.max_turns = 3;
        o
    };

    let outcome = runtime
        .run(agent_def, manifest, initial, options)
        .await
        .expect("agent-level failures flow through Ok(RunOutcome::Failed)");

    assert_matches!(
        outcome,
        RunOutcome::Failed {
            status: AgentStatus::Failed { kind, .. },
            total_turns,
            all_messages,
            ..
        } => {
            assert_eq!(kind, FailureKind::OutOfResources);
            assert_eq!(total_turns, 3, "loop should hit max_turns exactly");

            // Lower bound: initial user msg + 3 turns × (tool_call + tool_result) = 7.
            // The kernel skips an empty assistant text turn (this LLM never
            // emits text), so this is the canonical count.
            assert!(
                all_messages.len() >= 7,
                "expected >= 7 messages, got {}",
                all_messages.len()
            );
        }
    );

    // LLM was called once per turn, exactly `max_turns` times.
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
