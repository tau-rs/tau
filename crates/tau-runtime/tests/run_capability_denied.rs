//! Integration test: agent attempts to invoke a tool whose required
//! capability is NOT granted by its package manifest. The capability
//! check denies the call before `Tool::invoke` runs, and the run loop
//! returns `Ok(RunOutcome::Failed { kind: PolicyDenied })`.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tau_domain::{AgentStatus, Capability, FailureKind, Value};
use tau_ports::fixtures::{
    make_completion_response, make_token_usage, make_tool_result, make_tool_use,
};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime::{RunOutcome, Runtime};

use assert_matches::assert_matches;

/// Single-shot LLM that always emits the same canned response.
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

/// Tool that declares a non-empty `capabilities()` list and counts
/// `invoke()` calls. The kernel's capability check should reject the
/// dispatch BEFORE invoke runs; we assert the counter stays at zero.
struct RestrictedTool {
    schema: ToolSpec,
    required_caps: Vec<Capability>,
    invoke_count: Arc<AtomicUsize>,
}

impl Tool for RestrictedTool {
    type Session = ();

    fn name(&self) -> &str {
        &self.schema.name
    }

    fn schema(&self) -> ToolSpec {
        self.schema.clone()
    }

    fn capabilities(&self) -> &[Capability] {
        &self.required_caps
    }

    async fn init(&self, _ctx: SessionContext) -> Result<(), ToolError> {
        Ok(())
    }

    async fn invoke(&self, _: &mut (), _args: Value) -> Result<ToolResult, ToolError> {
        // If we ever reach here, capability enforcement is broken.
        self.invoke_count.fetch_add(1, Ordering::SeqCst);
        Ok(make_tool_result(Vec::new(), false))
    }

    async fn teardown(&self, _: ()) -> Result<(), ToolError> {
        Ok(())
    }
}

/// Build an `fs.read` capability via the canonical TOML deserialization
/// path. Variant-level `#[non_exhaustive]` blocks struct-literal
/// construction of `FsCapability::Read { paths }` from outside
/// `tau-domain`; the manifest deserializer is the only public path.
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

#[tokio::test]
async fn capability_denied_returns_policy_denied_failure() {
    // LLM emits a single tool_use targeting the restricted tool.
    let response = make_completion_response(
        String::new(),
        vec![make_tool_use(
            "u1".into(),
            "restricted-reader".into(),
            Value::Null,
        )],
        StopReason::ToolUse,
        Some(make_token_usage(4, 2)),
    );
    let llm = OneShotLlm {
        name: "gpt-4".into(),
        response,
    };

    // Tool requires fs.read on /etc/passwd; the agent's manifest grants
    // nothing. Capability check must deny.
    let invoke_count = Arc::new(AtomicUsize::new(0));
    let restricted = RestrictedTool {
        schema: common::empty_tool_spec("restricted-reader"),
        required_caps: vec![fs_read_cap(&["/etc/passwd"])],
        invoke_count: invoke_count.clone(),
    };

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_tool(restricted)
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("read /etc/passwd");

    let outcome = runtime
        .run(agent_def, manifest, initial, common::run_options())
        .await
        .expect("agent-level failures flow through Ok(RunOutcome::Failed)");

    assert_matches!(
        outcome,
        RunOutcome::Failed {
            status: AgentStatus::Failed { kind, detail, .. },
            total_turns,
            all_messages,
            ..
        } => {
            assert_eq!(kind, FailureKind::PolicyDenied);

            // The denial detail is `CapabilityDenial`'s `Display`; assert it
            // mentions the missing capability.
            let detail = detail.expect("detail should be set");
            assert!(
                detail.contains("/etc/passwd") || detail.contains("fs.read"),
                "detail should mention the denied capability; got {detail:?}"
            );

            // The denial happens during turn 1, so total_turns >= 1. Conversation
            // history retains the initial user message at minimum.
            assert!(total_turns >= 1, "got total_turns = {total_turns}");
            assert!(!all_messages.is_empty(), "conversation history preserved");
        }
    );

    // Critical: the tool's `invoke` was NEVER reached.
    assert_eq!(
        invoke_count.load(Ordering::SeqCst),
        0,
        "capability check must reject before Tool::invoke runs"
    );
}
