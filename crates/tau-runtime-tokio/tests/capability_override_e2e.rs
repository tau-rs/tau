//! End-to-end smoke test: validates the project tau.toml capability
//! override pipeline closes correctly through `Runtime::run`.
//!
//! Three scenarios:
//!
//! - **Narrow allow denies path in package but outside override:** the
//!   package grants `<parent>/**`; the project override narrows allow
//!   to `<parent>/sub/**`. The agent attempts to read a file at
//!   `<parent>/foo` (in-package, out-of-override). Expected:
//!   `Err(RuntimeError::Tool(ToolError::BadArgs))` with reason
//!   "not in capability scope".
//!
//! - **Deny carve-out denies path admitted by allow:** the package
//!   grants `<parent>/**`; project allow is unchanged (None) but deny
//!   lists the exact tempfile path. Agent attempts to read it. Expected:
//!   same scope-violation error.
//!
//! - **Expanding override rejects at runtime:** the package grants
//!   only `/var/definitely-not-the-tmpfile-dir/**`; the override allow
//!   tries to widen to `/etc/**` (not a subset). Expected:
//!   `Err(RuntimeError::CapabilityOverrideExpands { kind, reason })`.
//!
//! Gated `#[cfg(unix)]` (matches `tool_plugin_e2e.rs`): Windows breaks
//! when tempfile paths with backslashes get embedded into TOML strings.
//! Plugin-level deny enforcement is also exercised cross-platform via
//! the per-plugin integration tests in `crates/tau-plugins/fs-read/tests/invoke.rs`.

#![cfg(unix)]

mod common;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use fs_read_plugin_lib::plugin::{FsReadPlugin, FsReadSession};
use tau_domain::Value;
use tau_plugin_sdk::Configure;
use tau_ports::{
    fixtures::{make_completion_response, make_token_usage, make_tool_use},
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, SessionContext,
    StopReason, Tool, ToolError, ToolResult, ToolSpec,
};
use tau_runtime::{builder::DynTool, error::CoreRuntimeError, CapabilityOverride, Runtime};

use assert_matches::assert_matches;

// ---------------------------------------------------------------------------
// InProcessFsRead: DynTool adapter bridging FsReadSession → ()
// (verbatim from tool_plugin_e2e.rs — see module-level rationale there)
// ---------------------------------------------------------------------------

struct InProcessFsRead {
    plugin: FsReadPlugin,
    session: tokio::sync::Mutex<Option<FsReadSession>>,
}

impl InProcessFsRead {
    fn new() -> Self {
        let plugin = FsReadPlugin::from_config(Default::default())
            .expect("FsReadPlugin::from_config must not fail with default config");
        Self {
            plugin,
            session: tokio::sync::Mutex::new(None),
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

impl DynTool for InProcessFsRead {
    fn name(&self) -> &str {
        Tool::name(&self.plugin)
    }

    fn schema(&self) -> ToolSpec {
        Tool::schema(&self.plugin)
    }

    fn capabilities(&self) -> &[tau_domain::Capability] {
        Tool::capabilities(&self.plugin)
    }

    fn init<'a>(&'a self, ctx: SessionContext) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let s = Tool::init(&self.plugin, ctx).await?;
            *self.session.lock().await = Some(s);
            Ok(())
        })
    }

    fn invoke<'a>(
        &'a self,
        _ctx: &'a SessionContext,
        _session: &'a mut (),
        args: Value,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            let s = guard
                .as_mut()
                .expect("InProcessFsRead: init must be called before invoke");
            Tool::invoke(&self.plugin, s, args).await
        })
    }

    fn teardown<'a>(&'a self, _session: ()) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let taken = self.session.lock().await.take();
            if let Some(s) = taken {
                Tool::teardown(&self.plugin, s).await?;
            }
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// LLM fixtures
// ---------------------------------------------------------------------------

/// One-shot LLM: emits a single `tool_use { fs-read, path }`. Used when
/// the runtime errors before reaching a second turn.
struct OneShotFsReadLlm {
    target_path: String,
}

impl LlmBackend for OneShotFsReadLlm {
    fn name(&self) -> &str {
        "test-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let args: Value = serde_json::from_value(serde_json::json!({ "path": self.target_path }))
            .expect("args JSON must round-trip to tau_domain::Value");
        let tool_use = make_tool_use("call_1".into(), "fs-read".into(), args);
        Ok(make_completion_response(
            String::new(),
            vec![tool_use],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        ))
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

/// Two-turn LLM: turn 1 emits `tool_use { fs-read, path }`, turn 2 emits
/// "done" / EndTurn. Carries unused state for parity with the existing
/// e2e tests; we don't need turn 2 here since all three tests fail at
/// or before the first invoke.
#[allow(dead_code)]
struct ScriptedFsReadLlm {
    responses: Mutex<VecDeque<CompletionResponse>>,
}

impl ScriptedFsReadLlm {
    #[allow(dead_code)]
    fn new(target_path: String) -> Self {
        let args: Value = serde_json::from_value(serde_json::json!({ "path": target_path }))
            .expect("args JSON must round-trip to tau_domain::Value");
        let tool_use = make_tool_use("call_1".into(), "fs-read".into(), args);
        let turn1 = make_completion_response(
            String::new(),
            vec![tool_use],
            StopReason::ToolUse,
            Some(make_token_usage(10, 5)),
        );
        let turn2 = make_completion_response(
            "done".into(),
            Vec::new(),
            StopReason::EndTurn,
            Some(make_token_usage(8, 3)),
        );
        Self {
            responses: Mutex::new(vec![turn1, turn2].into()),
        }
    }
}

impl LlmBackend for ScriptedFsReadLlm {
    fn name(&self) -> &str {
        "test-llm"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.responses
            .lock()
            .expect("responses mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "ScriptedFsReadLlm: no more scripted responses".into(),
            })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = self.complete(req).await?;
        Ok(tau_ports::batch_to_stream(resp))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Narrow allow denies a path that is in the package's grant but outside
/// the project override's narrowed allow-list. The package grants
/// `<parent>/**`; override narrows allow to `<parent>/sub/**`. Agent
/// reads `<parent>/<file>` (in-package, out-of-override).
#[tokio::test]
async fn narrowed_allow_denies_path_outside_override() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();
    std::fs::write(tmpfile.path(), b"secret\n").expect("write tempfile");

    let parent = tmpfile
        .path()
        .parent()
        .expect("tempfile has a parent")
        .to_str()
        .unwrap();
    let package_glob = format!("{parent}/**");
    let narrow_allow = format!("{parent}/sub/**"); // doesn't cover the tempfile

    let llm = OneShotFsReadLlm {
        target_path: path.clone(),
    };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-narrow", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_from_toml(&format!(
        r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["{package_glob}"]
"#
    ));
    let initial = common::user_message("read the file");

    let mut options = common::run_options();
    options.capability_resolver = Some(
        tau_runtime::capability_resolver_impl::resolver_from_overrides(vec![
            CapabilityOverride::new(
                "fs.read".to_string(),
                Some(vec![narrow_allow]),
                Vec::new(),
                None,
            ),
        ]),
    );

    let err = runtime
        .run(agent_def, manifest, initial, options)
        .await
        .expect_err("plugin scope check must surface as Err(RuntimeError)");

    assert_matches!(
        err,
        CoreRuntimeError::Tool(ToolError::BadArgs { reason }) => {
            assert!(
                reason.contains("not in capability scope"),
                "narrowed allow must reject with scope-violation message; got {reason:?}"
            );
        }
    );
}

/// Deny carve-out denies a path the allow-list would otherwise admit.
/// Package grants `<parent>/**`; override leaves allow unchanged (None)
/// but adds the exact tempfile path to deny. Plugin's deny-after-allow
/// check rejects the call.
#[tokio::test]
async fn deny_carve_out_denies_admitted_path() {
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();
    std::fs::write(tmpfile.path(), b"secret\n").expect("write tempfile");

    let parent = tmpfile
        .path()
        .parent()
        .expect("tempfile has a parent")
        .to_str()
        .unwrap();
    let package_glob = format!("{parent}/**");

    let llm = OneShotFsReadLlm {
        target_path: path.clone(),
    };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-deny", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_from_toml(&format!(
        r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["{package_glob}"]
"#
    ));
    let initial = common::user_message("read the file");

    let mut options = common::run_options();
    options.capability_resolver = Some(
        tau_runtime::capability_resolver_impl::resolver_from_overrides(vec![
            CapabilityOverride::new(
                "fs.read".to_string(),
                None, // allow unchanged
                vec![path.clone()],
                None,
            ),
        ]),
    );

    let err = runtime
        .run(agent_def, manifest, initial, options)
        .await
        .expect_err("plugin deny check must surface as Err(RuntimeError)");

    assert_matches!(
        err,
        CoreRuntimeError::Tool(ToolError::BadArgs { reason }) => {
            assert!(
                reason.contains("not in capability scope"),
                "deny carve-out must reject with scope-violation message; got {reason:?}"
            );
        }
    );
}

/// Expanding override fails the run with CapabilityOverrideExpands.
/// Package grants `/var/definitely-not-the-tmpfile-dir/**`; override
/// allow tries to widen to `/etc/**` (not a subset). The runtime
/// re-check via `compute_effective` rejects before the loop starts.
#[tokio::test]
async fn expanding_override_rejects_at_runtime() {
    // A real tempfile is never read; the run fails before invoke.
    let tmpfile = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_str().unwrap().to_string();

    let llm = OneShotFsReadLlm { target_path: path };
    let tool: Arc<dyn DynTool> = Arc::new(InProcessFsRead::new());

    let runtime = Runtime::builder()
        .with_llm_backend(llm)
        .with_dyn_tool(tool)
        .build()
        .expect("Runtime builds");

    let agent_def = common::agent_def("agent-expand", "test-agent", "test-pkg@0.1.0", "test-llm");
    let manifest = common::manifest_from_toml(
        r#"
name = "test-pkg"
version = "0.1.0"
description = "test package"
authors = []
source = "https://example.com/test.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/var/definitely-not-the-tmpfile-dir/**"]
"#,
    );
    let initial = common::user_message("read the file");

    let mut options = common::run_options();
    options.capability_resolver = Some(
        tau_runtime::capability_resolver_impl::resolver_from_overrides(vec![
            CapabilityOverride::new(
                "fs.read".to_string(),
                Some(vec!["/etc/**".to_string()]), // not a subset of /var/...
                Vec::new(),
                None,
            ),
        ]),
    );

    let err = runtime
        .run(agent_def, manifest, initial, options)
        .await
        .expect_err("expanding override must fail at runtime");

    assert_matches!(
        err,
        CoreRuntimeError::CapabilityOverrideExpands { kind, reason } => {
            assert_eq!(kind, "fs.read");
            assert!(
                reason.contains("not a subset"),
                "expand-rejected reason must mention subset; got {reason:?}"
            );
        }
    );
}
