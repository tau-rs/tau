//! Integration test: tools whose required capabilities aren't satisfied
//! by the agent package's grants are filtered out of
//! `CompletionRequest.tools`.
//!
//! Verifies sub-project 5's first additive amendment to tau-runtime
//! (spec §3.10 of `docs/superpowers/specs/0005-tau-cli.md`): the kernel
//! pre-filters the tool list exposed to the LLM so that the model never
//! sees tools whose capabilities the agent's package can't satisfy.
//! Capability enforcement at invoke time stays as defense-in-depth (see
//! `run_capability_denied.rs`).

mod common;

use std::sync::{Arc, Mutex};

use tau_domain::{Capability, Value};
use tau_ports::fixtures::{make_completion_response, make_tool_spec, MockTool};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime::{RunOptions, RunOutcome, Runtime};

/// Build an `fs.read` capability via the canonical TOML deserialization
/// path. Variant-level `#[non_exhaustive]` blocks struct-literal
/// construction of `FsCapability::Read { paths }` from outside
/// `tau-domain`, so we round-trip through the manifest wire form.
fn fs_read_cap(paths: &[&str]) -> Capability {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        cap: Capability,
    }
    let paths_toml = paths
        .iter()
        .map(|p| format!("\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let toml_body = format!(
        r#"[cap]
kind = "fs.read"
paths = [{paths_toml}]
"#
    );
    toml::from_str::<Wrapper>(&toml_body)
        .expect("test fs.read capability TOML must parse")
        .cap
}

/// Tool that overrides `capabilities()` to require `fs.read /tmp/**`.
/// `invoke` is `unreachable!` — if the filter works, the LLM never sees
/// this tool and the run loop never dispatches to it.
struct FsReadTool {
    name: String,
    spec: ToolSpec,
    caps: Vec<Capability>,
}

impl FsReadTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            spec: make_tool_spec(
                name.to_string(),
                format!("{name} tool"),
                Value::Object(Default::default()),
            ),
            caps: vec![fs_read_cap(&["/tmp/**"])],
        }
    }
}

impl Tool for FsReadTool {
    type Session = ();

    fn name(&self) -> &str {
        &self.name
    }
    fn schema(&self) -> ToolSpec {
        self.spec.clone()
    }
    fn capabilities(&self) -> &[Capability] {
        &self.caps
    }
    async fn init(&self, _ctx: SessionContext) -> Result<(), ToolError> {
        Ok(())
    }
    async fn invoke(&self, _: &mut (), _: Value) -> Result<ToolResult, ToolError> {
        unreachable!("filter should prevent invocation");
    }
    async fn teardown(&self, _: ()) -> Result<(), ToolError> {
        Ok(())
    }
}

/// LLM that records every received [`CompletionRequest`] and returns a
/// canned response. We use this (rather than `MockLlmBackend`) so the
/// invocations vector is shared via `Arc<Mutex>` across the runtime
/// boundary and the test assertions.
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
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

#[tokio::test]
async fn filtered_tool_not_exposed_in_completion_request() {
    // Canned response: a single-turn EndTurn so the loop terminates after
    // one LLM call and we can inspect `request.tools`.
    let resp = make_completion_response("done".into(), Vec::new(), StopReason::EndTurn, None);
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let llm = RecordingLlm {
        name: "gpt-4".into(),
        response: resp,
        invocations: invocations.clone(),
    };

    // `echo` has no required capabilities — the agent's empty grant
    // trivially satisfies it.
    let echo_spec = make_tool_spec(
        "echo".into(),
        "echo".into(),
        Value::Object(Default::default()),
    );

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(MockTool::new("echo", echo_spec))
        .with_tool(FsReadTool::new("fs-read"))
        .build()
        .expect("build runtime");

    // Agent package declares NO capabilities; `fs-read` requires
    // `fs.read /tmp/**`, so the filter should drop it.
    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("Hi");

    let outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("run succeeded");

    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let recorded = invocations.lock().expect("RecordingLlm mutex poisoned");
    let request = recorded.first().expect("at least one LLM call");
    let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        tool_names,
        vec!["echo"],
        "fs-read should be filtered out of CompletionRequest.tools",
    );
}
