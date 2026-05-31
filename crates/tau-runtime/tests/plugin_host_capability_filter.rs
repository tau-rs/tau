//! Verifies that the kernel's capability filter (see
//! `Runtime::run_with_history` in `run.rs`, sub-project 5 amendment)
//! still applies even when the registered `LlmBackend` is an
//! [`IpcLlmBackend`] dispatched over the wire.
//!
//! Architecture per spec §7.5: the filter runs in
//! `Runtime::run_with_history` BEFORE the IPC adapter is called; the
//! adapter only sees the already-filtered `CompletionRequest.tools`.
//! This test wires up an IPC-backed runtime and asserts the
//! `CompletionRequest` reaching the wire carries only the tools the
//! agent's package can satisfy.

#![cfg(feature = "test-support")]

mod common;

use std::sync::Arc;
use std::time::Duration;
use tau_runtime::RuntimeShellExt;

use tau_domain::{Capability, Value};
use tau_plugin_protocol::test_support::FakeStdioPeer;
use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
use tau_ports::fixtures::{make_completion_response, make_tool_spec, MockTool};
use tau_ports::{
    CompletionRequest, SessionContext, StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime::plugin_host::__internals::{DynAsyncWriter, IpcLlmBackend, PluginProcess};
use tau_runtime::{RunOutcome, Runtime};
use tokio::io::DuplexStream;

/// Build a [`PluginProcess`] paired with a [`FakeStdioPeer`].
/// Mirror of the helper in `plugin_host_ipc_llm.rs`; duplicated here
/// rather than threaded through `tests/common/mod.rs` because the
/// helper depends on the `test-support` feature gate which isn't
/// universal across the integration tests.
fn paired_process(plugin_name: &str) -> (Arc<PluginProcess>, FakeStdioPeer) {
    let (peer_read_half, sut_write_half) = tokio::io::duplex(64 * 1024);
    let (sut_read_half, peer_write_half) = tokio::io::duplex(64 * 1024);
    let peer = FakeStdioPeer {
        reader: FramedReader::new(peer_read_half, FramerOptions::default()),
        writer: FramedWriter::new(peer_write_half),
    };
    let sut_reader: FramedReader<DuplexStream> =
        FramedReader::new(sut_read_half, FramerOptions::default());
    let sut_writer: FramedWriter<DynAsyncWriter> =
        FramedWriter::new(Box::new(sut_write_half) as DynAsyncWriter);

    let process = PluginProcess::new_for_test(
        plugin_name.to_string(),
        sut_reader,
        sut_writer,
        Duration::from_secs(2),
    );
    (process, peer)
}

/// Build an `fs.read` capability via the canonical TOML deserialization
/// path. Variant-level `#[non_exhaustive]` blocks struct-literal
/// construction of `FsCapability::Read { paths }` from outside
/// `tau-domain`, so we round-trip through the manifest wire form.
/// (Mirrored from `tests/run_filters_unauthorized_tools.rs`.)
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

/// Tool that requires `fs.read /tmp/**`. Cribbed from
/// `tests/run_filters_unauthorized_tools.rs`. `invoke` is `unreachable!`
/// — if the filter works, the LLM never sees this tool and the run
/// loop never dispatches to it.
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

#[tokio::test]
async fn capability_filter_applies_when_llm_backend_is_ipc() {
    // Wire up an IPC-backed LlmBackend named `gpt-4` plus two tools:
    // - `echo` (no required capabilities — admitted)
    // - `fs-read` (requires `fs.read /tmp/**` — filtered out because
    //   the agent's package declares no capabilities)
    let (process, mut peer) = paired_process("gpt-4");
    let backend: Arc<dyn tau_runtime::builder::DynLlmBackend> =
        Arc::new(IpcLlmBackend::new("gpt-4".to_string(), process));

    let echo_spec = make_tool_spec(
        "echo".into(),
        "echo".into(),
        Value::Object(Default::default()),
    );

    let runtime = Runtime::builder()
        .with_dyn_llm_backend(backend)
        .with_tool(MockTool::new("echo", echo_spec))
        .with_tool(FsReadTool::new("fs-read"))
        .build()
        .expect("build runtime");

    let agent_def = common::agent_def("agent-1", "test-agent", "test-pkg@0.1.0", "gpt-4");
    let manifest = common::manifest_with_no_capabilities();
    let initial = common::user_message("Hi");

    // Drive the run loop and the peer side concurrently. The peer
    // receives one llm.complete request, asserts the wire-recorded
    // `CompletionRequest.tools` excludes `fs-read`, and replies with a
    // single-turn EndTurn so the loop terminates.
    let run_fut = runtime.run(agent_def, manifest, initial, common::run_options());
    let peer_fut = async {
        let (msgid, params_bytes) = peer.expect_request("llm.complete").await;
        let parsed: Vec<CompletionRequest> =
            rmp_serde::from_slice(&params_bytes).expect("params decode");
        assert_eq!(parsed.len(), 1);
        let tool_names: Vec<&str> = parsed[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            tool_names,
            vec!["echo"],
            "fs-read should be filtered out of CompletionRequest.tools \
             before the IPC adapter sends it",
        );
        let resp = make_completion_response("done".into(), Vec::new(), StopReason::EndTurn, None);
        peer.send_response(msgid, &resp).await.unwrap();
    };
    let (run_outcome, ()) = tokio::join!(run_fut, peer_fut);
    let outcome = run_outcome.expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
}
