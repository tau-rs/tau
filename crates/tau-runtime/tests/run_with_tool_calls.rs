//! Integration test: agent runs across two turns — one with a
//! `tool_use` block resolved against a registered tool, one with a
//! plain text response that ends the loop.

mod common;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tau_runtime::RuntimeShellExt;

use tau_domain::{Address, MessagePayload, Value};
use tau_ports::fixtures::{make_completion_response, make_token_usage, make_tool_use, MockTool};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, StopReason,
};
use tau_runtime::{RunOutcome, Runtime};

use assert_matches::assert_matches;

/// LLM mock with a per-call response queue. `MockLlmBackend` returns a
/// single canned response for every `complete()` call, which doesn't
/// fit a multi-turn scenario where turn 1 emits a `tool_use` and turn
/// 2 emits text. We pop from a queue instead.
///
/// `invocations` is shared via `Arc<AtomicU32>` so the test can keep an
/// observation handle after handing the backend to the runtime
/// (`Runtime::with_llm_backend` consumes the value into an
/// `Arc<dyn DynLlmBackend>`).
struct ScriptedLlm {
    name: String,
    responses: Mutex<VecDeque<CompletionResponse>>,
    invocations: Arc<AtomicU32>,
}

impl ScriptedLlm {
    fn new(name: &str, responses: Vec<CompletionResponse>, invocations: Arc<AtomicU32>) -> Self {
        Self {
            name: name.to_string(),
            responses: Mutex::new(responses.into()),
            invocations,
        }
    }
}

impl LlmBackend for ScriptedLlm {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .expect("responses mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "ScriptedLlm: no more scripted responses".into(),
            })
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let resp = self
            .responses
            .lock()
            .expect("responses mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "ScriptedLlm: no more scripted responses".into(),
            })?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

#[tokio::test]
async fn run_with_tool_calls_across_two_turns() {
    // Turn 1: tool_use(echo, args = "hi"), no text, StopReason::ToolUse.
    let turn1 = make_completion_response(
        String::new(),
        vec![make_tool_use(
            "u1".into(),
            "echo".into(),
            Value::String("hi".into()),
        )],
        StopReason::ToolUse,
        Some(make_token_usage(7, 3)),
    );
    // Turn 2: plain text "done", no tool_uses, StopReason::EndTurn.
    let turn2 = make_completion_response(
        "done".into(),
        Vec::new(),
        StopReason::EndTurn,
        Some(make_token_usage(11, 5)),
    );

    let invocations = Arc::new(AtomicU32::new(0));
    let llm = ScriptedLlm::new("gpt-4", vec![turn1, turn2], invocations.clone());

    // The default `MockTool` returns `Ok(ToolResult { content: vec![], is_error: false })`
    // when no `with_result` is configured — exactly what we want here.
    let echo_tool = MockTool::new("echo", common::empty_tool_spec("echo"));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(echo_tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("Hi");

    let outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    // kept as let-else: many independent assertions follow, a block-form
    // assert_matches! would bury them inside a single match arm.
    let RunOutcome::Completed {
        final_message,
        all_messages,
        total_turns,
        token_usage,
        ..
    } = outcome
    else {
        panic!("expected RunOutcome::Completed, got {outcome:?}");
    };

    assert_eq!(total_turns, 2, "tool-use turn + final text turn");

    // Conversation history: initial user msg + tool_call + tool_result + final text.
    // Turn 1 emits NO assistant text (text is empty), so the run loop
    // skips appending an assistant Text message before the tool_call;
    // see `append_assistant_response` in run.rs.
    assert_eq!(
        all_messages.len(),
        4,
        "user + tool_call + tool_result + assistant text"
    );

    // [0] user-authored: Text("Hi")
    assert_eq!(all_messages[0].sender, Address::User);
    assert!(matches!(
        &all_messages[0].payload,
        MessagePayload::Text { content } if content == "Hi"
    ));

    // [1] agent → tool: ToolCall { args = "hi" }
    assert!(matches!(all_messages[1].sender, Address::Agent(_)));
    assert_eq!(all_messages[1].recipient, Address::Tool("echo".into()));
    assert_matches!(
        &all_messages[1].payload,
        MessagePayload::ToolCall { args } => {
            assert_eq!(*args, Value::String("hi".into()));
        }
    );

    // [2] tool → agent: ToolResult { body = ... }
    assert_eq!(all_messages[2].sender, Address::Tool("echo".into()));
    assert!(matches!(all_messages[2].recipient, Address::Agent(_)));
    assert!(matches!(
        all_messages[2].payload,
        MessagePayload::ToolResult { .. }
    ));

    // [3] agent → user: Text("done")
    assert!(matches!(all_messages[3].sender, Address::Agent(_)));
    assert_eq!(all_messages[3].recipient, Address::User);
    assert!(matches!(
        &all_messages[3].payload,
        MessagePayload::Text { content } if content == "done"
    ));

    // Final message is the text.
    assert!(matches!(
        &final_message.payload,
        MessagePayload::Text { content } if content == "done"
    ));

    // Token usage is summed across both turns.
    assert_eq!(token_usage.input_tokens, 7 + 11);
    assert_eq!(token_usage.output_tokens, 3 + 5);

    // LLM was called exactly twice.
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        2,
        "ScriptedLlm.complete() was called once per turn"
    );
}
