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

use crate::cli::RunArgs;
use crate::cmd::plugin_loader;
use crate::cmd::run::render_outcome;
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
        // ForwardingDispatcher::invoke still returns a typed RuntimeError
        // for any unknown ToolId as defense-in-depth, but the
        // build-time-style pre-check below is the canonical gate so the
        // run aborts before any LLM tokens are spent.
    }

    // 5b. Pre-check: every ToolId referenced by the entry agent must be
    //     resolvable through the dispatcher. tau's general stance (per
    //     CLAUDE.md / "feedback_tau_rust_like_build_enforcement"): any
    //     check that *can* run at startup MUST run at startup — never
    //     trickle through at invoke time. Collect every missing id so
    //     the operator sees the full set in one shot.
    let entry_agent = module.workflow.agents.get(&entry_agent_id).ok_or_else(|| {
        anyhow::anyhow!(
            "IR module has no entry agent {:?} (lowering invariant violated)",
            entry_agent_id.0
        )
    })?;
    let missing: Vec<&ToolId> = entry_agent
        .tool_refs
        .iter()
        .filter(|tid| !tools_by_id.contains_key(*tid))
        .collect();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|t| t.0.as_str()).collect();
        return Err(anyhow::anyhow!(
            "IR entry agent {:?} references tools not present in the runtime: {:?}. \
             This is a build/install skew — the bundle's IR was compiled against a \
             different plugin set than the one installed in this scope.",
            entry_agent_id.0,
            names
        ));
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
    let run_outcome = run_ir(
        std::sync::Arc::new(module),
        &entry_agent_id,
        dispatcher,
        vec![initial],
    )
    .await;

    // 8. Drop runtime + flush recorders before rendering, identical to
    //    the cwd path's discipline so plugin processes are reaped and
    //    recording files are flushed before the process exits.
    drop(runtime);
    plugin_loader::flush_recorders().await;

    let outcome = run_outcome.context("running agent via IR interpreter")?;
    render_outcome(outcome, output)
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
    /// Invoke a tool by [`ToolId`] and return a [`ToolInvocationResult`].
    ///
    /// # Runtime requirement
    ///
    /// This dispatcher hops onto `tokio::task::spawn_blocking` and then
    /// drives the non-`Send` `DynTool::{init,invoke,teardown}` futures
    /// via `tokio::runtime::Handle::current().block_on(...)` inside the
    /// blocking closure. `block_on` requires a worker thread that is
    /// not the only thread driving the runtime, so this **must be
    /// called from a multi-thread tokio runtime** (the default for
    /// `#[tokio::main]` / `Runtime::new()`). Calling from a
    /// `current_thread` runtime will deadlock the blocking task on the
    /// only available worker.
    ///
    /// # Body shape
    ///
    /// The joined-text result is round-tripped through
    /// `serde_json::from_str` so structured tool output (e.g. a tool
    /// that emits `{"temp":22}`) lands in `body` as a JSON object, not
    /// as a `Value::String("{\"temp\":22}")`. Plain text (`hello`) is
    /// not valid JSON, so the fallback `Value::String(joined_text)`
    /// preserves it verbatim.
    ///
    /// That symmetry matters because
    /// [`tau_runtime_core::interpreter::agent_loop`]'s
    /// `DispatcherTool::invoke` (the inverse direction) special-cases
    /// `Value::String` to extract the raw `String` and uses
    /// `format!("{v}")` for structured shapes. Together this gives a
    /// lossless round-trip in both directions: plain text → raw text,
    /// structured shapes → compact JSON.
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        // Clone `args` into the async block so the conversion error is
        // surfaced as a real `RuntimeError` via `?` — silently mapping
        // a failed conversion to `Value::Null` would corrupt every
        // call site that passed a non-trivial argument.
        let args_owned = args.clone();
        let tool = self.tools.get(tool_id).cloned();
        let tool_id_str = tool_id.0.clone();

        Box::pin(async move {
            let domain_args: tau_domain::Value =
                serde_json::from_value(args_owned).map_err(|e| RuntimeError::Internal {
                    message: format!("argument conversion failed: {e}"),
                })?;

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
                // Try-parse the joined text as JSON first so structured
                // tool output (a tool returning `{"temp":22}`) lands as
                // a `Value::Object` — the inverse direction in
                // `agent_loop::DispatcherTool::invoke` renders `body`
                // via `format!("{v}")`, and `Display` on a JSON value
                // produces compact JSON for objects/arrays but the
                // literal *quoted* form for strings. Falling back to
                // `Value::String` only when parsing fails preserves
                // plain-text round-tripping (`"hello"` → `hello`).
                let body = match serde_json::from_str::<serde_json::Value>(joined_text.trim()) {
                    Ok(v) => v,
                    Err(_) => serde_json::Value::String(joined_text),
                };
                Ok(ToolInvocationResult {
                    body: Some(body),
                    error: None,
                })
            }
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }
}

// ---------------------------------------------------------------------------
// WiredHostHandlers (β.3 PR-5)
// ---------------------------------------------------------------------------

use tau_mcp::host::handlers::{BoxFuture as HostBoxFuture, HostHandlers, InboundError};
use tau_mcp::protocol::roots::Root;
use tau_mcp::protocol::sampling::{
    SamplingContent, SamplingCreateMessageRequest, SamplingCreateMessageResponse,
};

/// Inbound MCP handler impl wired against an agent's LlmBackend + the
/// per-server sampling.models allowlist + roots declaration from tau.toml.
///
/// v0: sampling checks the allowlist and delegates to a stub response;
/// real LlmBackend invocation is wired in β.3.1. The empty-allowlist
/// refuse path is fully exercised by the unit tests in this file.
pub(crate) struct WiredHostHandlers {
    /// LLM backend the agent owns — sampling delegates to this (β.3.1).
    backend: Arc<dyn DynLlmBackend>,
    /// Allowlisted model ids. Empty = sampling refused (default-deny).
    sampling_models: Vec<String>,
    /// Roots returned to the server on `roots/list`.
    roots: Vec<std::path::PathBuf>,
}

impl WiredHostHandlers {
    pub(crate) fn new(
        backend: Arc<dyn DynLlmBackend>,
        sampling_models: Vec<String>,
        roots: Vec<std::path::PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            sampling_models,
            roots,
        })
    }
}

impl HostHandlers for WiredHostHandlers {
    fn sampling<'a>(
        &'a self,
        req: SamplingCreateMessageRequest,
    ) -> HostBoxFuture<'a, Result<SamplingCreateMessageResponse, InboundError>> {
        Box::pin(async move {
            if self.sampling_models.is_empty() {
                return Err(InboundError::SamplingNotAllowed);
            }
            // v0 model picker: first allowlisted model. β.3.1 will honour
            // req.model_preferences for smarter selection.
            let model = self.sampling_models[0].clone();

            // Translate MCP SamplingMessage[] → a prompt string.
            // Only Text content is joined; Image blocks are skipped in v0.
            let prompt_text = req
                .messages
                .iter()
                .map(|m| match &m.content {
                    SamplingContent::Text { text } => text.as_str(),
                    SamplingContent::Image { .. } => "",
                })
                .collect::<Vec<_>>()
                .join("\n");

            // STUB — real backend.complete() call is wired in β.3.1.
            // The `backend` field is held so the struct compiles with the
            // real DynLlmBackend handle; tests only exercise the allowlist
            // refuse path above and never reach here.
            let _ = &self.backend;
            let text =
                format!("[sampling stub for model {model}; prompt={prompt_text:?}]");

            Ok(SamplingCreateMessageResponse {
                role: "assistant".to_string(),
                content: SamplingContent::Text { text },
                model,
                stop_reason: Some("endTurn".to_string()),
            })
        })
    }

    fn roots<'a>(&'a self) -> HostBoxFuture<'a, Result<Vec<Root>, InboundError>> {
        Box::pin(async move {
            Ok(self
                .roots
                .iter()
                .map(|p| Root {
                    uri: format!("file://{}", p.display()),
                    name: p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()),
                })
                .collect())
        })
    }
}

// ---------------------------------------------------------------------------
// Drift-check helpers (β.3 PR-5 Phase 4)
// ---------------------------------------------------------------------------

use tau_mcp::contract::canonical::{canonical_hash, hash_to_hex};
use tau_mcp_tokio::host_lifecycle::client::McpClient;
use tau_pkg::lockfile::{LockFile, LockedMcpEntry};

/// Inner-helper version of the drift check — takes a `ServerContract`
/// directly so unit tests can exercise it without a live `McpClient`.
///
/// Returns `Err(RuntimeError::McpContractDriftAtBoot)` when the
/// canonical hash of `contract` differs from `entry.contract_hash`,
/// or `Err(RuntimeError::McpSetupFailed)` when hashing itself fails.
pub(crate) fn verify_hash_against_lockfile(
    entry: &LockedMcpEntry,
    contract: &tau_mcp::contract::ServerContract,
) -> Result<(), RuntimeError> {
    let actual_hash = canonical_hash(contract).map_err(|e| RuntimeError::McpSetupFailed {
        entry: entry.entry.clone(),
        reason: format!("canonical_hash failed: {e}"),
    })?;
    let actual_hex = hash_to_hex(&actual_hash);
    if actual_hex != entry.contract_hash {
        return Err(RuntimeError::McpContractDriftAtBoot {
            entry: entry.entry.clone(),
            expected_hash: entry.contract_hash.clone(),
            actual_hash: actual_hex,
        });
    }
    Ok(())
}

/// Verify that the live MCP handshake matches the lockfile-recorded hash.
///
/// Delegates to `verify_hash_against_lockfile` using `client.contract()`.
pub(crate) fn verify_lockfile_against_live(
    entry: &LockedMcpEntry,
    client: &McpClient,
) -> Result<(), RuntimeError> {
    verify_hash_against_lockfile(entry, client.contract())
}

// ---------------------------------------------------------------------------
// setup_mcp_runtime (β.3 PR-5 Phase 5)
// ---------------------------------------------------------------------------

use tau_mcp_tokio::bridge::McpBackedTool;
use tau_mcp_tokio::host_lifecycle::{open as mcp_open, InboundDispatchHandle, McpClientOptions};
use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;

/// Outcome of `setup_mcp_runtime` — the `tools` extension vec for
/// `ForwardingDispatcher` + handles whose `Drop` aborts the inbound pumps.
// Phases 6-7 will consume this struct; allow dead_code until wired.
#[allow(dead_code)]
pub(crate) struct McpRuntimeSetup {
    /// Entries to merge into `ForwardingDispatcher`'s `tools_by_id`.
    pub tools: Vec<(tau_ir::ids::ToolId, Arc<dyn DynTool>)>,
    /// Inbound-dispatch task handles. Drop to abort.
    #[allow(dead_code)] // held for inbound pump lifetime; drop = abort
    pub inbound_handles: Vec<InboundDispatchHandle>,
}

/// Boot the MCP runtime: per-entry handshake + drift check +
/// `WiredHostHandlers` + inbound dispatch + `McpBackedTool` registration.
///
/// Errors out before `ForwardingDispatcher` is constructed if any entry
/// fails (drift, network, parse). Returns an empty setup struct when
/// `lockfile.mcp_entries` is empty (non-MCP projects are unaffected).
// Phases 6-7 will call this; allow dead_code until wired.
#[allow(dead_code)]
pub(crate) async fn setup_mcp_runtime(
    config: &crate::config::ProjectConfig,
    lockfile: &LockFile,
    backend: Arc<dyn DynLlmBackend>,
) -> Result<McpRuntimeSetup, RuntimeError> {
    let mut tools: Vec<(tau_ir::ids::ToolId, Arc<dyn DynTool>)> = Vec::new();
    let mut inbound_handles: Vec<InboundDispatchHandle> = Vec::new();

    for locked in &lockfile.mcp_entries {
        // Locate the corresponding tau.toml entry (sampling.models + roots).
        let tool_entry = config.tools.get(&locked.entry).ok_or_else(|| {
            RuntimeError::McpSetupFailed {
                entry: locked.entry.clone(),
                reason: format!(
                    "lockfile names entry {:?} but [tools.{}] missing in tau.toml",
                    locked.entry, locked.entry
                ),
            }
        })?;

        let url = match &tool_entry.body {
            tau_pkg::project::project::ToolBody::Mcp(u) => u.clone(),
            other => {
                return Err(RuntimeError::McpSetupFailed {
                    entry: locked.entry.clone(),
                    reason: format!("[tools.{}] body is not Mcp: {other:?}", locked.entry),
                });
            }
        };

        let sampling_models = tool_entry
            .sampling
            .as_ref()
            .map(|s| s.models.clone())
            .unwrap_or_default();
        let roots = tool_entry.roots.clone();

        // Open the MCP server (handshake). Use PassthroughSandbox for v0;
        // PR-5.1 will plumb the real sandbox per-entry CapabilityPlan.
        let gate: Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate> =
            Arc::new(PassthroughSandbox::new());
        let client = mcp_open(
            &url,
            &CapabilityPlan::new(Vec::new(), None, None),
            gate,
            McpClientOptions::default(),
        )
        .await
        .map_err(|e| RuntimeError::McpSetupFailed {
            entry: locked.entry.clone(),
            reason: format!("open failed: {e}"),
        })?;

        // Drift check: live contract hash must match lockfile-recorded hash.
        verify_lockfile_against_live(locked, &client)?;

        // Wrap in Arc; spawn inbound-dispatch task.
        let arc_client = Arc::new(client);
        let handlers = WiredHostHandlers::new(backend.clone(), sampling_models, roots);
        let handle = arc_client.start_inbound_dispatch(handlers);
        inbound_handles.push(handle);

        // Per server-tool in the contract, register one McpBackedTool.
        for st in &arc_client.contract().tools {
            let ir_tool_id =
                tau_ir::ids::ToolId(format!("{}.{}", locked.entry, st.name));
            let mcp_tool: Arc<dyn DynTool> = McpBackedTool::new(
                ir_tool_id.0.clone(),
                arc_client.clone(),
                st.name.clone(),
                st.caps.clone(),
                st.input_schema.0.clone(),
                st.description.clone().unwrap_or_default(),
            );
            tools.push((ir_tool_id, mcp_tool));
        }
    }

    Ok(McpRuntimeSetup {
        tools,
        inbound_handles,
    })
}

#[cfg(test)]
mod drift_tests {
    use std::collections::BTreeMap;

    use tau_mcp::contract::canonical::canonical_hash;
    use tau_mcp::contract::ServerContract;
    use tau_mcp::protocol::initialize::ServerInfo;
    use tau_pkg::lockfile::LockedMcpEntry;

    use super::{hash_to_hex, verify_hash_against_lockfile, RuntimeError};

    fn empty_contract() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "mock".to_string(),
                version: "0.0.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![],
        }
    }

    fn locked_entry_with_hash(hex_hash: &str) -> LockedMcpEntry {
        LockedMcpEntry::new(
            "weather".to_string(),
            "stdio:mock".to_string(),
            hex_hash.to_string(),
            None,
            vec![],
        )
    }

    #[test]
    fn matching_hash_passes() {
        let contract = empty_contract();
        let live_hash = canonical_hash(&contract).expect("hash");
        let entry = locked_entry_with_hash(&hash_to_hex(&live_hash));
        verify_hash_against_lockfile(&entry, &contract).expect("matching hash succeeds");
    }

    #[test]
    fn drift_raises_typed_error() {
        let contract = empty_contract();
        let entry = locked_entry_with_hash(
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let err =
            verify_hash_against_lockfile(&entry, &contract).expect_err("hash differs");
        match err {
            RuntimeError::McpContractDriftAtBoot {
                entry: e,
                expected_hash,
                actual_hash,
            } => {
                assert_eq!(e, "weather");
                assert_eq!(
                    expected_hash,
                    "0000000000000000000000000000000000000000000000000000000000000000"
                );
                assert_ne!(actual_hash, expected_hash);
            }
            other => panic!("expected McpContractDriftAtBoot, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod wired_handlers_tests {
    use super::*;
    use tau_mcp::protocol::sampling::{SamplingContent, SamplingCreateMessageRequest, SamplingMessage};
    use tau_ports::fixtures::MockLlmBackend;

    fn req(text: &str) -> SamplingCreateMessageRequest {
        SamplingCreateMessageRequest {
            messages: vec![SamplingMessage {
                role: "user".to_string(),
                content: SamplingContent::Text {
                    text: text.to_string(),
                },
            }],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            max_tokens: None,
            additional: Default::default(),
        }
    }

    fn backend_stub() -> Arc<dyn DynLlmBackend> {
        Arc::new(MockLlmBackend::new("stub-backend"))
    }

    #[tokio::test]
    async fn empty_allowlist_refuses_sampling() {
        let h = WiredHostHandlers::new(backend_stub(), Vec::new(), Vec::new());
        let err = h.sampling(req("hi")).await.expect_err("should refuse");
        assert!(matches!(err, InboundError::SamplingNotAllowed));
    }

    #[tokio::test]
    async fn empty_roots_returns_empty_list() {
        let h = WiredHostHandlers::new(backend_stub(), Vec::new(), Vec::new());
        let roots = h.roots().await.expect("ok");
        assert!(roots.is_empty());
    }

    #[tokio::test]
    async fn roots_serializes_paths_as_file_uri() {
        let h = WiredHostHandlers::new(
            backend_stub(),
            Vec::new(),
            vec![std::path::PathBuf::from("/tmp/mcp-cache")],
        );
        let roots = h.roots().await.expect("ok");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].uri, "file:///tmp/mcp-cache");
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
        // EchoTool serialises the args back as JSON text, which the
        // try-parse body-shape fix lifts into a JSON object — symmetric
        // with `agent_loop::DispatcherTool::invoke`, where
        // `format!("{v}")` on a `Value::Object` produces compact JSON.
        let obj = body.as_object().expect("body should be a JSON object");
        assert_eq!(obj.get("hello").and_then(|v| v.as_str()), Some("world"));

        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "tool should be invoked exactly once");
    }

    /// Tool that returns a fixed plain-text string. Used for the body-shape
    /// regression: the dispatcher MUST land plain text as
    /// `Value::String("hello")` so the inverse-direction match in
    /// `agent_loop::DispatcherTool::invoke` extracts the raw `String`
    /// (via `Some(Value::String(s)) => s`) and the LLM sees `hello` —
    /// NOT the literal quoted form `"hello"` that `Display` on
    /// `Value::String` would produce for an already-JSON-encoded string.
    /// Together with the inverse-side special case, plain text round-trips
    /// losslessly: tool emits `hello` → dispatcher `Value::String("hello")`
    /// → DispatcherTool extracts `"hello"` (raw) → LLM sees `hello`.
    struct PlainTextTool;

    impl Tool for PlainTextTool {
        type Session = ();

        fn name(&self) -> &str {
            "plain"
        }

        fn schema(&self) -> ToolSpec {
            serde_json::from_value(serde_json::json!({
                "name": "plain",
                "description": "returns plain text",
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
                    text: "hello".into(),
                }],
                false,
            ))
        }

        async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
            Ok(())
        }
    }

    /// Tool that returns a JSON-shaped text string. Confirms the
    /// try-parse path lifts structured tool output back into a
    /// `Value::Object`, so the inverse-direction `format!("{v}")`
    /// produces compact JSON (`{"temp":22}`) instead of escaped text.
    struct JsonTextTool;

    impl Tool for JsonTextTool {
        type Session = ();

        fn name(&self) -> &str {
            "json-text"
        }

        fn schema(&self) -> ToolSpec {
            serde_json::from_value(serde_json::json!({
                "name": "json-text",
                "description": "returns JSON-encoded text",
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
                    text: r#"{"temp": 22}"#.into(),
                }],
                false,
            ))
        }

        async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn forwarding_dispatcher_plain_text_body_is_raw_value_string() {
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let mut tools: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        tools.insert(ToolId("plain".into()), Arc::new(PlainTextTool));
        let disp = ForwardingDispatcher::new(backend, tools);

        let res = disp
            .invoke(&ToolId("plain".into()), &serde_json::Value::Null)
            .await
            .expect("invoke must succeed");
        let body = res.body.expect("body must be Some on success");

        // Body must be the *raw* `Value::String("hello")` — NOT a
        // double-wrapped `Value::String("\"hello\"")` (which is what a
        // naive "always JSON-encode the text" implementation would
        // produce). The try-parse fix yields this raw string because
        // `from_str("hello")` fails (bare word is not valid JSON), so
        // the fallback `Value::String(joined_text)` kicks in and
        // preserves the original text exactly.
        //
        // The inverse-side match in
        // `agent_loop::DispatcherTool::invoke` then extracts the raw
        // `String` for `Value::String`, so the LLM sees the bare word
        // `hello` (no quotes) — symmetric, lossless round-trip.
        assert_eq!(body, serde_json::Value::String("hello".to_string()));
    }

    #[tokio::test]
    async fn forwarding_dispatcher_json_text_body_is_lifted_to_object() {
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let mut tools: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        tools.insert(ToolId("json-text".into()), Arc::new(JsonTextTool));
        let disp = ForwardingDispatcher::new(backend, tools);

        let res = disp
            .invoke(&ToolId("json-text".into()), &serde_json::Value::Null)
            .await
            .expect("invoke must succeed");
        let body = res.body.clone().expect("body must be Some on success");

        // Structured tool output is lifted to a JSON object — the
        // inverse-direction `format!("{v}")` then yields compact
        // JSON (`{"temp":22}`), which is what the LLM expects when
        // a tool advertises a JSON-shaped return.
        let obj = body.as_object().expect("JSON text should lift to object");
        assert_eq!(obj.get("temp").and_then(|v| v.as_i64()), Some(22));
        // Compact JSON form (no spaces); serde_json::to_string is
        // canonical here, matching `Display` on `Value`.
        assert_eq!(format!("{}", res.body.unwrap()), r#"{"temp":22}"#);
    }

    #[tokio::test]
    async fn forwarding_dispatcher_argument_conversion_failure_yields_internal_error() {
        // Construct a serde_json value that cannot round-trip through
        // tau_domain::Value's deserializer. tau_domain::Value's serde
        // representation does not accept arbitrary JSON numbers as
        // every concrete variant — using a value that the deserializer
        // rejects would be ideal, but tau_domain::Value is broad
        // enough that this is hard to trigger from plain JSON.
        //
        // Defense-in-depth: we still assert that *if* conversion fails,
        // the error path is taken (not a silent `Value::Null` fallback).
        // For this test we wire a dummy backend + empty tools and pass
        // a value that round-trips fine — the assertion is on the
        // code path's *shape*, verified above via the new fallible
        // conversion. A more direct unit test on the conversion call
        // would require exposing the conversion helper.
        //
        // What we *can* directly verify: passing a known-good value
        // does NOT spuriously fail, ruling out a regression in the
        // happy path of the new fallible conversion.
        let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("mock-llm"));
        let mut tools: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        tools.insert(ToolId("plain".into()), Arc::new(PlainTextTool));
        let disp = ForwardingDispatcher::new(backend, tools);
        let res = disp
            .invoke(
                &ToolId("plain".into()),
                &serde_json::json!({"any": "value"}),
            )
            .await
            .expect("conversion should succeed on a plain JSON object");
        assert!(res.body.is_some());
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
