//! `tau workflow {list, run, log, resume}` — workflow lifecycle commands.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use tau_pkg::Scope;
use tau_workflow::{StepKind, Workflow};

use crate::cli::WorkflowSubcommand;
use crate::output::Output;

pub mod list;
pub mod log;
pub mod resume;
pub mod run;

/// Dispatch a workflow subcommand.
///
/// `workflow_run_id` is the pre-minted ULID built by
/// `run_main::prepare_workflow_run_layer`. The CLI mints it before
/// installing the global tracing subscriber so that the
/// `WorkflowRunLogLayer` (attached to that subscriber) and the runner
/// (called below) agree on the on-disk JSONL path. The argument is only
/// meaningful for `WorkflowSubcommand::Run`.
pub async fn dispatch(
    sub: WorkflowSubcommand,
    workflow_run_id: Option<String>,
    output: &mut Output,
) -> anyhow::Result<()> {
    match sub {
        WorkflowSubcommand::List => list::run(output),
        WorkflowSubcommand::Run(args) => run::run(&args, workflow_run_id, output).await,
        WorkflowSubcommand::Log(args) => log::run(&args, output).await,
        WorkflowSubcommand::Resume(args) => resume::run(&args, output).await,
    }
}

/// Build the `{agent_id → (AgentDefinition, PackageManifest)}` map for every
/// agent referenced by the workflow (either as an `agent.run` target or as the
/// default-agent for a `tool.call` step).
pub(crate) fn build_agents_map(
    workflow: &Workflow,
    cwd: &Path,
    scope: &Scope,
) -> anyhow::Result<BTreeMap<String, (tau_domain::AgentDefinition, tau_domain::PackageManifest)>> {
    use std::collections::BTreeSet;
    let mut needed: BTreeSet<String> = BTreeSet::new();
    for step in &workflow.steps {
        match &step.kind {
            StepKind::AgentRun { agent, .. } => {
                needed.insert(agent.clone());
            }
            StepKind::ToolCall { .. } => {
                if let Some(a) = workflow.default_agent.clone() {
                    needed.insert(a);
                }
            }
        }
    }

    let project_path = cwd.join("tau.toml");
    let project = crate::config::ProjectConfig::from_path(&project_path)
        .with_context(|| format!("project tau.toml required at {project_path:?}"))?;

    let mut out = BTreeMap::new();
    for agent_id in needed {
        let entry = project.agents.get(&agent_id).ok_or_else(|| {
            anyhow::anyhow!(
                "workflow references agent {:?} which is not declared in tau.toml",
                agent_id
            )
        })?;
        let (agent_def, manifest) = crate::config::build_agent_definition(entry, cwd, scope)
            .with_context(|| format!("resolving agent {:?}", agent_id))?;
        out.insert(agent_id, (agent_def, manifest));
    }
    Ok(out)
}

/// Build a `tau_runtime_tokio::Runtime` for use by the workflow runner.
///
/// Mirrors the plugin-loading sequence from `crates/tau-cli/src/cmd/run.rs`:
///
/// 1. Pick the first agent entry from `agents` to source plugin configuration.
///    For v1 workflows all agents are assumed to share the same LLM backend
///    and tools; if they differ, the first agent's plugins are used and the
///    others will rely on the same runtime instance.
/// 2. Call `plugin_loader::load_plugins` with default host options (no
///    `--record-protocol`, no `--no-sandbox` / `--sandbox` override).
/// 3. Build the runtime via `RuntimeBuilder::build`.
///
/// Sandbox selection honours the scope's `[sandbox]` config in `.tau/config.toml`
/// exactly as it would for `tau run`.
pub(crate) async fn build_runtime_for_workflow(
    _cwd: &Path,
    scope: &Scope,
    agents: &BTreeMap<String, (tau_domain::AgentDefinition, tau_domain::PackageManifest)>,
) -> anyhow::Result<tau_runtime_tokio::Runtime> {
    // Pick the first agent entry so we can source plugin config.
    // In a typical workflow all agents share one LLM backend; even when they
    // differ, the runtime's kernel is agent-agnostic — the LLM backend plugin
    // loaded here is what actually gets invoked for every agent.run step.
    let first_agent_id = agents.keys().next().ok_or_else(|| {
        anyhow::anyhow!("workflow has no agent-run or tool-call steps requiring an agent")
    })?;

    // Reload the project config to get the raw AgentEntry (which carries the
    // llm_backend name, requires.tools, capability_overrides, and config block).
    let cwd = std::env::current_dir()?;
    let project_path = cwd.join("tau.toml");
    let project = crate::config::ProjectConfig::from_path(&project_path)
        .with_context(|| format!("project tau.toml required at {project_path:?}"))?;

    let entry = project.agents.get(first_agent_id).ok_or_else(|| {
        anyhow::anyhow!(
            "agent {:?} not found in tau.toml (internal error: was present at build_agents_map time)",
            first_agent_id
        )
    })?;

    // Resolve + install missing tools for the first agent.
    // (Subsequent agents with the same tool set are implicitly covered.)
    // Use a null/discarded output so plugin-loader status lines don't
    // interleave with the caller's output stream.
    let mut null_output = crate::output::Output::with_writers(
        Box::new(std::io::sink()),
        Box::new(std::io::sink()),
        false,
        true,
        crate::output::ColorChoice::Never,
    );
    crate::cmd::resolve_helpers::resolve_and_install_for_agent(
        entry,
        scope,
        false, // no_install = false: auto-install if needed
        &mut null_output,
    )?;

    let run_id = format!(
        "tau-workflow-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let trace_context = tau_plugin_protocol::handshake::TraceContext::new(
        run_id,
        first_agent_id.clone(),
        "workflow".to_string(),
    );

    // Default host options: no recording, no forced sandbox override.
    let host_options = crate::cmd::plugin_loader::build_host_options(None, false, None);

    let loaded = crate::cmd::plugin_loader::load_plugins(entry, scope, trace_context, host_options)
        .await
        .with_context(|| format!("loading plugins for agent {:?}", first_agent_id))?;

    loaded
        .builder
        .build()
        .context("failed to build runtime from spawned plugins")
}
