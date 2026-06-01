//! `tau run --bundle` IR-interpreter dispatch path.
//!
//! When a verified bundle carries an [`tau_pkg::bundle::manifest::IrPayload`]
//! ("v2" bundle), this module decodes the canonical IR bytes, builds a
//! forwarding [`ToolDispatcher`] over the host's plugin registry, and
//! drives [`tau_runtime_core::interpreter::run_ir`] to completion. The
//! result is mapped to stdout / [`crate::cmd::run::AgentFailed`] using
//! the same shape the cwd-based path uses, so callers see identical
//! exit-code semantics regardless of which path executed the run.
//!
//! Legacy v1 bundles (no `ir_payload`) continue to use the cwd-based
//! agent path — see [`crate::cmd::run::run`].

use std::collections::BTreeMap;
use std::future::Future;
use std::io::Read;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_ir::{IrModule, ToolId};
use tau_plugin_protocol::handshake::TraceContext;
use tau_ports::tool::{SessionContext, ToolContent};
use tau_runtime_core::builder::{DynLlmBackend, DynTool};
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_tokio::RunOutcome;

use crate::cli::RunArgs;
use crate::cmd::plugin_loader;
use crate::cmd::run::AgentFailed;
use crate::config::ProjectConfig;
use crate::output::Output;

/// Run a verified v2 bundle through the IR interpreter.
///
/// Picks the first agent in the IR module's `BTreeMap` (alphabetical
/// order) as the entry per the β.2 v0 contract. Future v0.x will infer
/// the entry from a `[workflow]` block.
///
/// Returns `Ok(())` on a `RunOutcome::Completed`, `Err(AgentFailed)` on
/// `RunOutcome::Failed`, and any other kernel/CLI error as a wrapped
/// [`anyhow::Error`] — matching the cwd-based path's error shape so
/// `lib::run_main`'s downcast continues to map exit codes correctly.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_via_ir(
    module: IrModule,
    args: &RunArgs,
    record_protocol: Option<PathBuf>,
    force_passthrough: bool,
    force_adapter_kind: Option<tau_runtime_tokio::process_gate::registry::RegistryKind>,
    output: &mut Output,
) -> anyhow::Result<()> {
    // 1. Pick the entry agent (first BTreeMap key — alphabetical order).
    let entry_agent_id = module
        .workflow
        .agents
        .keys()
        .next()
        .ok_or_else(|| anyhow::anyhow!("IR module has no agents"))?
        .clone();

    // 2. Load the project config from the cwd (proven byte-clean by the
    //    bundle verify gate). The IR module names agents using the IR's
    //    own `AgentId`; the project tau.toml uses `AgentEntry.id`. v0
    //    requires them to agree (lowering preserves the id verbatim).
    let cwd = std::env::current_dir()?;
    let project_path = cwd.join("tau.toml");
    let project = ProjectConfig::from_path(&project_path)
        .with_context(|| format!("project tau.toml required at {project_path:?}"))?;

    let agent_entry = project.agents.get(&entry_agent_id.0).ok_or_else(|| {
        anyhow::anyhow!(
            "IR entry agent {:?} not found in project tau.toml (the IR \
             lowering should keep agent ids verbatim; this is a build/run skew)",
            entry_agent_id.0
        )
    })?;

    // 3. Resolve + install missing packages for this agent.
    let scope = tau_pkg::Scope::resolve(&cwd).context("resolving package scope")?;
    crate::cmd::resolve_helpers::resolve_and_install_for_agent(
        agent_entry,
        &scope,
        args.no_install,
        output,
    )?;

    // 4. Build plugin host options + spawn plugins (same flow the cwd path uses).
    let run_id = format!(
        "tau-run-bundle-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let trace_context = TraceContext::new(run_id, entry_agent_id.0.clone(), "root".to_string());
    let host_options = plugin_loader::build_host_options(
        record_protocol.as_deref(),
        force_passthrough,
        force_adapter_kind,
    );
    let loaded =
        plugin_loader::load_plugins(agent_entry, &scope, trace_context, host_options).await?;
    let runtime = loaded
        .builder
        .build()
        .context("failed to build runtime from spawned plugins")?;

    // 5. Pull Arc<dyn DynLlmBackend> + Arc<dyn DynTool> handles for the
    //    forwarding dispatcher. ToolId.0 == tool.spec.name == DynTool::name()
    //    by lowering construction; see crates/tau-ir/src/lower/parse.rs.
    let llm_backend = runtime
        .llm_backends()
        .values()
        .next()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("runtime has no LLM backend after plugin load"))?;

    let mut tools_by_id: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
    for ir_tool_id in module.workflow.tools.keys() {
        if let Some(handle) = runtime.tools().get(&ir_tool_id.0) {
            tools_by_id.insert(ir_tool_id.clone(), handle.clone());
        }
        // Missing tools surface at invoke time as a typed RuntimeError so
        // the dispatcher is the single point where the error shape is
        // chosen — keeps this assembly step shape-symmetric with the cwd
        // path which also does not pre-validate the tool set.
    }

    let dispatcher = Arc::new(ForwardingDispatcher::new(llm_backend, tools_by_id));

    // 6. Build the initial message vec from --prompt / stdin.
    let prompt_text = match &args.prompt {
        Some(s) => s.clone(),
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("reading initial prompt from stdin")?;
            buf
        }
    };
    let initial = Message::new(
        Address::User,
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text {
            content: prompt_text,
        },
    );

    // 7. Drive the IR interpreter.
    let run_outcome = run_ir(&module, &entry_agent_id, dispatcher, vec![initial]).await;

    // 8. Drop runtime + flush recorders before rendering, identical to
    //    the cwd path's discipline so plugin processes are reaped and
    //    recording files are flushed before the process exits.
    drop(runtime);
    plugin_loader::flush_recorders().await;

    let outcome = run_outcome.context("running agent via IR interpreter")?;
    render_outcome(outcome, output)
}

/// Map a [`RunOutcome`] to stdout (human / JSON) + [`AgentFailed`] per the
/// cwd path's contract. Kept inline here so the bundle path doesn't fork
/// off into its own subtly-different rendering — both paths emit the
/// same `{"outcome":"completed", ...}` / `{"outcome":"failed", ...}`
/// JSON shape.
fn render_outcome(outcome: RunOutcome, output: &mut Output) -> anyhow::Result<()> {
    match outcome {
        RunOutcome::Completed {
            ref final_message,
            total_turns,
            ref token_usage,
            ..
        } => {
            if output.is_json() {
                let payload = serde_json::json!({
                    "outcome": "completed",
                    "final_message": format_message_text(&final_message.payload),
                    "total_turns": total_turns,
                    "token_usage": {
                        "input_tokens": token_usage.input_tokens,
                        "output_tokens": token_usage.output_tokens,
                    },
                });
                output.json(&payload)?;
            } else {
                let text = format_message_text(&final_message.payload);
                output.human(&text)?;
            }
            Ok(())
        }
        RunOutcome::Failed {
            ref status,
            total_turns,
            ref token_usage,
            ..
        } => {
            if output.is_json() {
                let payload = serde_json::json!({
                    "outcome": "failed",
                    "status": format!("{status:?}"),
                    "total_turns": total_turns,
                    "token_usage": {
                        "input_tokens": token_usage.input_tokens,
                        "output_tokens": token_usage.output_tokens,
                    },
                });
                output.json(&payload)?;
            } else {
                output.error(format!("agent failed: {status:?}"))?;
            }
            Err(AgentFailed.into())
        }
        _ => Err(anyhow::anyhow!("unknown RunOutcome variant")),
    }
}

/// Project a [`MessagePayload`] to a single text string for display.
/// Mirror of `run::format_message_text` — kept private here to avoid a
/// pub-visibility leak just for one helper.
fn format_message_text(payload: &MessagePayload) -> String {
    match payload {
        MessagePayload::Text { content } => content.clone(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// ForwardingDispatcher
// ---------------------------------------------------------------------------

/// A [`ToolDispatcher`] that forwards `invoke(tool_id, args)` to the
/// corresponding `Arc<dyn DynTool>` in the host's plugin registry.
///
/// The dispatcher owns:
/// * `backend`: the LLM-backend handle the interpreter hands to each
///   agent-loop construction (see `agent_loop::run_agent`).
/// * `tools`: a `BTreeMap<ToolId, Arc<dyn DynTool>>` from IR ids to
///   real plugin tool handles. Lookup is by ToolId (which matches
///   `DynTool::name()` by lowering construction).
///
/// Unknown ToolIds surface as `RuntimeError::Internal` — there is no
/// silent fallback. A v2 bundle whose IR references a tool not in the
/// runtime is a build/install skew (the verify gate should catch it,
/// but defense-in-depth is cheap).
pub(crate) struct ForwardingDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    tools: BTreeMap<ToolId, Arc<dyn DynTool>>,
}

impl ForwardingDispatcher {
    pub(crate) fn new(
        backend: Arc<dyn DynLlmBackend>,
        tools: BTreeMap<ToolId, Arc<dyn DynTool>>,
    ) -> Self {
        Self { backend, tools }
    }
}

impl ToolDispatcher for ForwardingDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        // Convert serde_json::Value → tau_domain::Value via serde round-trip
        // (matching agent_loop::DispatcherTool's reverse conversion).
        let domain_args: tau_domain::Value =
            serde_json::from_value(args.clone()).unwrap_or(tau_domain::Value::Null);

        let tool = self.tools.get(tool_id).cloned();
        let tool_id_str = tool_id.0.clone();

        Box::pin(async move {
            let tool = tool.ok_or_else(|| RuntimeError::Internal {
                message: format!(
                    "ForwardingDispatcher: no tool registered for IR ToolId {tool_id_str:?}"
                ),
            })?;

            // Mint a fresh SessionContext per invocation. The IR-driven
            // path does not currently thread agent grants / deny entries
            // through to plugin tools — this is symmetric with the
            // cwd-based path at v0 (per-tool capability overrides are
            // a deferred feature in plugin_loader.rs).
            let ctx = SessionContext::new(AgentInstanceId::new(), uuid::Uuid::new_v4(), None);

            // `DynTool::{init,invoke,teardown}` return non-`Send` futures
            // (their concrete impls — e.g. `IpcTool` in
            // `tau-runtime-tokio::plugin_host` — never explicitly annotate
            // `+ Send` in the trait-object return type). The interpreter's
            // `ToolDispatcher::invoke` contract, however, requires a
            // `+ Send` future so `run_ir` can be Sync across worker
            // boundaries.
            //
            // To bridge: hop onto `tokio::task::spawn_blocking` (which
            // takes a `Send + 'static` closure that returns a `Send`
            // value) and drive the non-`Send` futures inside via
            // `Handle::current().block_on(...)`. The closure's captures
            // (`Arc<dyn DynTool>`, `SessionContext`, `tau_domain::Value`)
            // are all `Send + 'static`, and the resulting `ToolResult` is
            // also `Send`, so the spawn_blocking signature is satisfied.
            let tool_for_blocking = tool.clone();
            let ctx_for_blocking = ctx.clone();
            let tool_id_str_for_blocking = tool_id_str.clone();
            let blocking = tokio::task::spawn_blocking(move || {
                tokio::runtime::Handle::current().block_on(async move {
                    tool_for_blocking
                        .init(ctx_for_blocking.clone())
                        .await
                        .map_err(|e| RuntimeError::Internal {
                            message: format!(
                                "tool {tool_id_str_for_blocking:?} init failed: {e:?}"
                            ),
                        })?;
                    let mut session: () = ();
                    let result = tool_for_blocking
                        .invoke(&ctx_for_blocking, &mut session, domain_args)
                        .await
                        .map_err(|e| RuntimeError::Internal {
                            message: format!(
                                "tool {tool_id_str_for_blocking:?} invoke failed: {e:?}"
                            ),
                        });
                    // Best-effort teardown — never let cleanup turn a
                    // successful invoke into a failure.
                    let _ = tool_for_blocking.teardown(session).await;
                    result
                })
            });

            let result = blocking.await.map_err(|join_err| RuntimeError::Internal {
                message: format!(
                    "tool {tool_id_str:?} blocking task joined with error: {join_err}"
                ),
            })??;

            // Joined text from every content block (Json blocks degrade
            // to their Debug form to keep the contract simple — v0
            // tool plugins overwhelmingly return Text).
            let joined_text = result
                .content
                .iter()
                .map(|c| match c {
                    ToolContent::Text { text } => text.clone(),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join("");

            if result.is_error {
                Ok(ToolInvocationResult {
                    body: None,
                    error: Some(joined_text),
                })
            } else {
                // Wrap the joined text in a JSON String — symmetric with
                // agent_loop::DispatcherTool's text-body construction in
                // the reverse direction.
                Ok(ToolInvocationResult {
                    body: Some(serde_json::Value::String(joined_text)),
                    error: None,
                })
            }
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tau_ports::fixtures::MockLlmBackend;
    use tau_ports::tool::{Tool, ToolContent, ToolResult};
    use tau_ports::{ToolError, ToolSpec};

    /// Minimal stateless tool that records the args it received and
    /// echoes them back as a JSON-serialised text block.
    struct EchoTool {
        recorded: Arc<Mutex<Vec<tau_domain::Value>>>,
    }

    impl Tool for EchoTool {
        type Session = ();

        fn name(&self) -> &str {
            "echo"
        }

        fn schema(&self) -> ToolSpec {
            // ToolSpec is #[non_exhaustive] — construct via serde to
            // mirror the rest of the codebase's escape-hatch pattern.
            serde_json::from_value(serde_json::json!({
                "name": "echo",
                "description": "echo args back",
                "input_schema": tau_domain::Value::Object(Default::default()),
            }))
            .expect("ToolSpec must deserialize")
        }

        async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
            Ok(())
        }

        async fn invoke(
            &self,
            _session: &mut Self::Session,
            args: tau_domain::Value,
        ) -> Result<ToolResult, ToolError> {
            self.recorded.lock().unwrap().push(args.clone());
            let text = serde_json::to_string(&args).unwrap_or_else(|_| "null".to_string());
            Ok(ToolResult::new(vec![ToolContent::Text { text }], false))
        }

        async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn forwarding_dispatcher_invokes_registered_tool_and_returns_body() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let tool = EchoTool {
            recorded: recorded.clone(),
        };
        let dyn_tool: Arc<dyn DynTool> = Arc::new(tool);
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));

        let mut tools: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        tools.insert(ToolId("echo".into()), dyn_tool);
        let disp = ForwardingDispatcher::new(backend, tools);

        let args = serde_json::json!({"hello": "world"});
        let res = disp
            .invoke(&ToolId("echo".into()), &args)
            .await
            .expect("invoke must succeed");

        assert!(
            res.error.is_none(),
            "expected success result, got error = {:?}",
            res.error
        );
        let body = res.body.expect("body must be Some on success");
        // EchoTool serialises the args back; the result is wrapped as a JSON String.
        let body_str = body.as_str().expect("body should be a JSON string");
        assert!(
            body_str.contains("hello") && body_str.contains("world"),
            "echoed body should contain the args; got {body_str}"
        );

        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "tool should be invoked exactly once");
    }

    #[tokio::test]
    async fn forwarding_dispatcher_unknown_tool_yields_internal_error() {
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let disp = ForwardingDispatcher::new(backend, BTreeMap::new());

        let err = match disp
            .invoke(&ToolId("missing".into()), &serde_json::Value::Null)
            .await
        {
            Ok(_) => panic!("unknown tool must error"),
            Err(e) => e,
        };
        let msg = format!("{err:?}");
        assert!(
            msg.contains("missing") && msg.contains("ForwardingDispatcher"),
            "error message should name the missing tool and dispatcher; got {msg}"
        );
    }

    #[tokio::test]
    async fn forwarding_dispatcher_llm_backend_returns_owned_handle() {
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("the-backend"));
        let disp = ForwardingDispatcher::new(backend, BTreeMap::new());

        let handle = disp.llm_backend();
        assert_eq!(handle.name(), "the-backend");
    }

    /// A tool that returns `is_error: true` so we can confirm the
    /// dispatcher routes it through `ToolInvocationResult.error`.
    struct ErroringTool;

    impl Tool for ErroringTool {
        type Session = ();

        fn name(&self) -> &str {
            "boom"
        }

        fn schema(&self) -> ToolSpec {
            serde_json::from_value(serde_json::json!({
                "name": "boom",
                "description": "always errors",
                "input_schema": tau_domain::Value::Object(Default::default()),
            }))
            .expect("ToolSpec must deserialize")
        }

        async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
            Ok(())
        }

        async fn invoke(
            &self,
            _session: &mut Self::Session,
            _args: tau_domain::Value,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::new(
                vec![ToolContent::Text {
                    text: "tool said no".into(),
                }],
                true,
            ))
        }

        async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn forwarding_dispatcher_semantic_error_routes_to_error_field() {
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let mut tools: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        tools.insert(ToolId("boom".into()), Arc::new(ErroringTool));
        let disp = ForwardingDispatcher::new(backend, tools);

        let res = disp
            .invoke(&ToolId("boom".into()), &serde_json::Value::Null)
            .await
            .expect("dispatcher call succeeds (the tool semantically errors)");
        assert!(res.body.is_none(), "body must be None on is_error=true");
        assert_eq!(res.error.as_deref(), Some("tool said no"));
    }
}
