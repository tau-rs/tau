//! Integration tests: kernel-level operational failures of `Runtime::run`.
//!
//! Per ADR-0006, kernel-level failures (plugin/dispatch problems) flow
//! through `Err(RuntimeError)` rather than `Ok(RunOutcome::Failed)`.
//! Scenarios covered:
//!
//! - `llm_backend_not_registered`: agent's `llm_backend` doesn't match
//!   any registered backend.
//! - `tool_not_registered`: the LLM emits a tool_use for an unknown tool.
//! - `tool_args_validation_failure_yields_recoverable_tool_error`: a
//!   tool_use with args that fail input-schema validation does NOT
//!   surface as `RuntimeError::PluginContractViolation` (the original
//!   Phase-1 premise was revised). Validation failures are *recoverable*:
//!   the run writes a `MessagePayload::ToolError { kind: "tool_args_validation", .. }`
//!   into the conversation, yields a `RunEvent::ToolCallCompleted` with
//!   `Err(reason)`, skips the tool's `invoke`, and continues. The
//!   scripted LLM then "self-corrects" with a plain text turn and the
//!   run reaches `RunOutcome::Completed`.

mod common;

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tau_domain::{Capability, MessagePayload, PackageName, Value};
use tau_ports::fixtures::{
    make_completion_response, make_token_usage, make_tool_spec, make_tool_use, MockLlmBackend,
};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, ToolError, ToolResult, ToolSpec,
};
use tau_runtime_tokio::{builder::DynTool, error::CoreRuntimeError, RunEvent, RunOutcome, Runtime};

use assert_matches::assert_matches;
use futures_core::Stream;

/// Single-shot LLM (re-used across the tests in this file).
struct OneShotLlm {
    name: String,
    response: CompletionResponse,
}

impl LlmBackend for OneShotLlm {
    fn name(&self) -> &str {
        &self.name
    }
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.response.clone())
    }
    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

#[tokio::test]
async fn llm_backend_not_registered() {
    // Register one backend...
    let runtime = Runtime::builder()
        .with_llm_backend(MockLlmBackend::new("different-backend"))
        .build()
        .expect("build runtime");

    // ...but ask the runtime to drive an agent that wants a different one.
    let mut agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    agent_def.llm_backend = PackageName::from_str("missing-backend").expect("valid name");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("hello");

    let result = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await;

    let err = result.unwrap_err();
    assert_matches!(
        err,
        CoreRuntimeError::LlmBackendNotRegistered {
            agent_id, backend, ..
        } => {
            assert_eq!(agent_id, "agent-1");
            assert_eq!(backend, "missing-backend");
        }
    );
}

#[tokio::test]
async fn tool_not_registered() {
    // LLM tells the runtime to call a tool that isn't registered.
    let response = make_completion_response(
        String::new(),
        vec![make_tool_use(
            "u1".into(),
            "nonexistent".into(),
            Value::Null,
        )],
        StopReason::ToolUse,
        Some(make_token_usage(2, 1)),
    );
    let llm = OneShotLlm {
        name: "gpt-4".into(),
        response,
    };

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        // Intentionally NO `with_tool` calls.
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call the missing tool");

    let result = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await;

    let err = result.unwrap_err();
    assert_matches!(
        err,
        CoreRuntimeError::ToolNotRegistered {
            tool_name,
            registered,
            ..
        } => {
            assert_eq!(tool_name, "nonexistent");
            assert!(
                registered.is_empty(),
                "no tools registered; got {registered:?}"
            );
        }
    );
}

// ---------------------------------------------------------------------------
// tool-args validation: recoverable-error path
// ---------------------------------------------------------------------------

/// A test-local tool with a strict input_schema (requires `text: string`)
/// and an invocation counter. The counter must remain at 0 when the
/// validator rejects malformed args before the run loop reaches
/// `invoke()` — that's the invariant this test pins.
struct CountingTool {
    name: String,
    schema: ToolSpec,
    invoke_calls: Arc<AtomicUsize>,
    caps: Vec<Capability>,
}

impl CountingTool {
    fn new(name: &str, invoke_calls: Arc<AtomicUsize>) -> Self {
        // Strict schema: object with required "text: string" property.
        // additionalProperties left default so {"foo": 1} fails purely on
        // the missing "text" requirement (less coupling to schema dialect).
        let schema_json = serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        });
        let schema_value: Value =
            serde_json::from_str(&serde_json::to_string(&schema_json).expect("serialize schema"))
                .expect("round-trip schema into tau_domain::Value");
        let schema = make_tool_spec(
            name.to_string(),
            format!("counting tool {name}"),
            schema_value,
        );
        Self {
            name: name.to_string(),
            schema,
            invoke_calls,
            caps: Vec::new(),
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

impl DynTool for CountingTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> ToolSpec {
        self.schema.clone()
    }

    fn capabilities(&self) -> &[Capability] {
        &self.caps
    }

    fn init<'a>(&'a self, _ctx: SessionContext) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move { Ok(()) })
    }

    fn invoke<'a>(
        &'a self,
        _ctx: &'a SessionContext,
        _session: &'a mut (),
        _args: Value,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        let counter = self.invoke_calls.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::new(Vec::new(), false))
        })
    }

    fn teardown<'a>(&'a self, _session: ()) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Collect all `RunEvent`s from a stream until it terminates (mirrors the
/// helper in `run_streaming_e2e.rs` — kept inline to avoid coupling the
/// `common/` module to streaming-only helpers).
async fn collect_events(stream: impl Stream<Item = RunEvent> + 'static) -> Vec<RunEvent> {
    let mut stream = Box::pin(stream);
    let mut events = Vec::new();
    loop {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            None => break,
            Some(event) => {
                let is_terminal = matches!(
                    &event,
                    RunEvent::RunCompleted { .. } | RunEvent::FatalError { .. }
                );
                events.push(event);
                if is_terminal {
                    break;
                }
            }
        }
    }
    events
}

/// Pins the Phase-1 design: when a `tool_use` carries args that fail
/// input-schema validation, the run loop does NOT terminate with
/// `RuntimeError::PluginContractViolation`. Instead it:
///
/// 1. writes a `MessagePayload::ToolError { kind: "tool_args_validation", .. }`
///    into the conversation,
/// 2. yields a `RunEvent::ToolCallCompleted { result: Err(reason), .. }`,
/// 3. skips `Tool::invoke`,
/// 4. continues so the LLM can self-correct.
///
/// Asserted here via a two-turn scripted LLM (bad-args tool_use, then a
/// plain text turn) and a `CountingTool` whose `invoke()` increments an
/// atomic counter — which must remain at zero.
#[tokio::test]
async fn tool_args_validation_failure_yields_recoverable_tool_error() {
    // Use the multi-turn `common::MockLlmBackend`:
    //   Turn 1 = tool_use("strict-tool", {"foo": 1}) — schema requires "text".
    //   Turn 2 = plain text "self-corrected" so the run reaches Completed
    //            rather than getting stuck on max_turns.
    let bad_args_json = serde_json::json!({ "foo": 1 });
    let bad_args: Value =
        serde_json::from_str(&serde_json::to_string(&bad_args_json).expect("serialize bad args"))
            .expect("round-trip bad args into tau_domain::Value");
    let llm = common::MockLlmBackend::new("gpt-4")
        .add_tool_call("strict-tool", bad_args)
        .add_text("after seeing the validation error I'll just describe it instead");

    let invoke_calls = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn DynTool> = Arc::new(CountingTool::new("strict-tool", invoke_calls.clone()));

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("call the strict tool");

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run_streaming should not fail at construction time");

    let events = collect_events(stream).await;

    // -- 1. Trace contains ToolCallCompleted with Err(reason) mentioning
    //       validation. --
    let validation_err_event = events.iter().find_map(|e| match e {
        RunEvent::ToolCallCompleted {
            name,
            result: Err(reason),
            ..
        } if name == "strict-tool" => Some(reason.clone()),
        _ => None,
    });
    let reason = validation_err_event.expect(
        "expected a RunEvent::ToolCallCompleted with result=Err for the strict-tool tool_use",
    );
    // The validator emits the ADR-0010 §4 template (`You sent:`,
    // `Expected (input_schema):`, `Specific issue(s):`); the missing
    // required field is reported in the `Specific issue(s)` block.
    assert!(
        reason.contains("Specific issue"),
        "validation reason must include the ADR-0010 'Specific issue' marker; got: {reason}"
    );
    assert!(
        reason.contains("text"),
        "validation reason must mention the missing required field name 'text'; got: {reason}"
    );

    // -- 2. The terminal RunCompleted event carries RunOutcome::Completed. --
    let outcome = events
        .iter()
        .rev()
        .find_map(|e| match e {
            RunEvent::RunCompleted { outcome } => Some(outcome.clone()),
            _ => None,
        })
        .expect("expected a terminal RunEvent::RunCompleted");
    // The scripted text turn lets the run finish cleanly.
    let all_messages = match outcome {
        RunOutcome::Completed { all_messages, .. } => all_messages,
        other => panic!("expected RunOutcome::Completed, got {other:?}"),
    };

    // -- 3. Conversation contains a MessagePayload::ToolError with
    //       kind=="tool_args_validation". --
    let validation_msg = all_messages.iter().find_map(|m| match &m.payload {
        MessagePayload::ToolError { kind, message, .. } if kind == "tool_args_validation" => {
            Some((kind.clone(), message.clone()))
        }
        _ => None,
    });
    let (kind, message) = validation_msg.expect(
        "conversation must contain a MessagePayload::ToolError with kind=tool_args_validation",
    );
    assert_eq!(kind, "tool_args_validation");
    assert!(
        message.contains("Specific issue"),
        "ToolError message must follow the ADR-0010 template; got: {message}"
    );

    // -- 4. The tool's invoke() was NEVER called: validation gates it. --
    assert_eq!(
        invoke_calls.load(Ordering::SeqCst),
        0,
        "Tool::invoke must NOT be called when input-schema validation rejects the args"
    );
}
