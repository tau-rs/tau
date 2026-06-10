//! `DevSession` — owns the loaded project, IR, history, and (Phase 4+)
//! the file watcher + MCP client cache.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_ir::lower::{lower_project, Caches};
use tau_ir::{AgentId, IrModule};
use tau_pkg::project::ProjectConfig;
use tau_plugin_protocol::handshake::TraceContext;
use tau_ports::target::TargetTriple;
use tau_runtime_core::builder::{DynLlmBackend, DynTool};
use tau_runtime_core::interpreter::run_ir;

use crate::cmd::ir_dispatcher::{setup_mcp_runtime, ForwardingDispatcher};
use crate::cmd::plugin_loader;
use crate::cmd::run::render_outcome;

/// All the long-lived state for one `tau dev` invocation.
pub struct DevSession {
    /// Project root (contains `tau.toml`).
    pub project_root: PathBuf,
    /// Parsed + validated project config.
    pub project: ProjectConfig,
    /// Lowered IR module for the current project.
    pub ir: IrModule,
    /// Name of the agent the REPL is currently driving.
    pub current_agent: String,
    /// Multi-turn conversation history (in-memory only in v1).
    pub history: Vec<Message>,
    /// Set true by the file watcher (Phase 4) when a watched file changes.
    /// Cleared by `:reload`.
    pub pending_reload: Arc<AtomicBool>,
    /// Watcher handle — kept alive to keep file-watching active.
    /// `None` if the watcher failed to register at boot (degraded mode).
    pub notify_handle: Option<notify::RecommendedWatcher>,
}

impl fmt::Debug for DevSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevSession")
            .field("project_root", &self.project_root)
            .field("current_agent", &self.current_agent)
            .field("history_len", &self.history.len())
            .field(
                "pending_reload",
                &self
                    .pending_reload
                    .load(std::sync::atomic::Ordering::Relaxed),
            )
            .field(
                "notify_handle",
                &self.notify_handle.as_ref().map(|_| "<watcher>"),
            )
            .finish_non_exhaustive()
    }
}

impl DevSession {
    /// Load + validate + lower a project into a fresh session.
    pub async fn load(project_root: PathBuf, agent_override: Option<String>) -> Result<Self> {
        let tau_toml_path = project_root.join("tau.toml");
        let toml_bytes = std::fs::read(&tau_toml_path)
            .with_context(|| format!("read {}", tau_toml_path.display()))?;
        let toml_str = std::str::from_utf8(&toml_bytes)
            .with_context(|| format!("{} is not UTF-8", tau_toml_path.display()))?;
        let project =
            ProjectConfig::parse_str(toml_str).map_err(|e| anyhow!("parse tau.toml: {e}"))?;

        // `project.agents` is a `BTreeMap<String, AgentEntry>`, so `.keys()` iterates
        // in alphabetical order — the first key is the alphabetical default.
        let current_agent = match agent_override {
            Some(name) => {
                if !project.agents.contains_key(&name) {
                    return Err(anyhow!("agent `{name}` not in tau.toml"));
                }
                name
            }
            None => project
                .agents
                .keys()
                .next()
                .ok_or_else(|| anyhow!("tau.toml declares no agents"))?
                .clone(),
        };

        let ir = lower_project_to_ir(&project).context("lower project to IR")?;

        let pending_reload = Arc::new(AtomicBool::new(false));

        let notify_handle = match crate::cmd::dev::watcher::spawn(
            &project_root,
            &project,
            pending_reload.clone(),
        ) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("warning: file watcher unavailable ({e}); use :reload manually");
                None
            }
        };

        Ok(Self {
            project_root,
            project,
            ir,
            current_agent,
            history: Vec::new(),
            pending_reload,
            notify_handle,
        })
    }

    /// Name of the agent the REPL is currently driving.
    pub fn current_agent_name(&self) -> &str {
        &self.current_agent
    }

    /// Read-only access to the conversation history.
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Apply pending manifest changes.
    ///
    /// Re-parses `tau.toml`, re-lowers IR, and updates `self.project` and
    /// `self.ir`. Conversation history is intentionally preserved so that
    /// the REPL retains multi-turn context across hot-reloads.
    ///
    /// MCP clients are created per-turn in [`run_turn`] (the `_mcp_lifetime`
    /// binding), so there are no long-lived MCP handles to drop here —
    /// the next call to `run_turn` automatically picks up the new config.
    ///
    /// Returns:
    /// - `Ok(true)`  — manifest reloaded successfully.
    /// - `Ok(false)` — `pending_reload` was not set; nothing to do.
    /// - `Err(e)`    — parse or lowering error; old config is kept intact
    ///   and `pending_reload` is restored so the user can fix and retry.
    pub async fn reload(&mut self) -> anyhow::Result<bool> {
        use std::sync::atomic::Ordering;

        if !self.pending_reload.swap(false, Ordering::AcqRel) {
            return Ok(false);
        }

        let tau_toml_path = self.project_root.join("tau.toml");

        let bytes = match std::fs::read(&tau_toml_path) {
            Ok(b) => b,
            Err(e) => {
                self.pending_reload.store(true, Ordering::Release);
                return Err(anyhow!("read tau.toml: {e}"));
            }
        };

        let toml_str = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(e) => {
                self.pending_reload.store(true, Ordering::Release);
                return Err(anyhow!("tau.toml is not UTF-8: {e}"));
            }
        };

        let new_project = match ProjectConfig::parse_str(toml_str) {
            Ok(p) => p,
            Err(e) => {
                self.pending_reload.store(true, Ordering::Release);
                return Err(anyhow!("parse tau.toml: {e}"));
            }
        };

        let new_ir = match lower_project_to_ir(&new_project) {
            Ok(ir) => ir,
            Err(e) => {
                self.pending_reload.store(true, Ordering::Release);
                return Err(anyhow!("lower IR: {e}"));
            }
        };

        // If the current agent was removed in the new manifest, fall back
        // to the first agent (alphabetical order) so the session stays valid.
        if !new_project.agents.contains_key(&self.current_agent) {
            if let Some(first) = new_project.agents.keys().next() {
                self.current_agent = first.clone();
            }
        }

        self.project = new_project;
        self.ir = new_ir;
        // history intentionally NOT touched
        Ok(true)
    }

    /// Watch-mode counterpart to [`reload`][Self::reload].
    ///
    /// Has the same effect (re-parse + re-lower + keep history) but signals
    /// "auto-applied by `--watch`" rather than "user typed `:reload`". The thin
    /// wrapper makes the watch-mode path explicit at call sites.
    pub async fn auto_reload_if_pending(&mut self) -> anyhow::Result<bool> {
        self.reload().await
    }

    /// Run one turn against the current agent.
    ///
    /// Mirrors the construction in `crate::cmd::ir_dispatcher::run_via_ir`:
    /// 1. Resolve the agent entry from `self.project`.
    /// 2. Load the per-scope lockfile + plugins via [`plugin_loader`].
    /// 3. Boot MCP servers from the project config + lockfile.
    /// 4. Build a [`ForwardingDispatcher`].
    /// 5. Append the user prompt to `self.history` and call [`run_ir`].
    /// 6. Append the agent's reply to `self.history` and render the outcome.
    ///
    /// History is preserved across turns so the REPL has multi-turn context.
    /// On error the history is left unchanged (the failed turn is not appended).
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        output: &mut crate::output::Output,
    ) -> Result<()> {
        // 1. Resolve the agent entry.
        let agent_entry = self
            .project
            .agents
            .get(&self.current_agent)
            .ok_or_else(|| {
                anyhow!(
                    "agent `{}` not in project (was it removed between loads?)",
                    self.current_agent
                )
            })?
            .clone();

        // 2. Resolve the package scope from the project root.
        let scope =
            tau_pkg::Scope::resolve(&self.project_root).context("resolving package scope")?;

        // 3. Build plugin host options (no recording, no forced sandbox in dev mode).
        let run_id = format!(
            "tau-dev-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let trace_context =
            TraceContext::new(run_id, self.current_agent.clone(), "dev-root".to_string());
        let host_options = plugin_loader::build_host_options(
            None,  // no --record-protocol in dev mode
            false, // no --no-sandbox override
            None,  // no forced adapter kind
        );
        let loaded =
            plugin_loader::load_plugins(&agent_entry, &scope, trace_context, host_options).await?;
        let runtime = loaded
            .builder
            .build()
            .context("failed to build runtime from spawned plugins")?;

        // 4. Pull DynLlmBackend + DynTool handles.
        let llm_backend: Arc<dyn DynLlmBackend> =
            runtime
                .llm_backends()
                .values()
                .next()
                .cloned()
                .ok_or_else(|| anyhow!("runtime has no LLM backend after plugin load"))?;

        let mut tools_by_id: BTreeMap<tau_ir::ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        for ir_tool_id in self.ir.workflow.tools.keys() {
            if let Some(handle) = runtime.tools().get(&ir_tool_id.0) {
                tools_by_id.insert(ir_tool_id.clone(), handle.clone());
            }
        }

        // 5. Boot MCP servers (from lockfile + project config) when a lockfile exists.
        //    Non-MCP projects (or projects not yet locked) skip this step safely.
        let lockfile_path = scope.lockfile_path();
        let _mcp_lifetime; // keep inbound-dispatch handles alive for the full turn
        if lockfile_path.exists() {
            let lockfile = tau_pkg::lockfile::LockFile::load(&lockfile_path)
                .with_context(|| format!("loading lockfile at {}", lockfile_path.display()))?;
            let mcp_setup = setup_mcp_runtime(&self.project, &lockfile, llm_backend.clone())
                .await
                .map_err(|e| anyhow!("MCP setup failed: {e}"))?;
            for (id, tool) in mcp_setup.tools {
                tools_by_id.insert(id, tool);
            }
            _mcp_lifetime = mcp_setup.inbound_handles;
        } else {
            _mcp_lifetime = Vec::new();
        }

        // 6. Build dispatcher, append the user turn to history, and run IR.
        let dispatcher = Arc::new(ForwardingDispatcher::new(llm_backend, tools_by_id));
        let entry_id = AgentId(self.current_agent.clone());
        let initial = Message::new(
            Address::User,
            Address::Agent(AgentInstanceId::new()),
            MessagePayload::Text {
                content: prompt.to_string(),
            },
        );
        self.history.push(initial);
        let history_snapshot = self.history.clone();

        let run_outcome = run_ir(
            Arc::new(self.ir.clone()),
            &entry_id,
            dispatcher,
            history_snapshot,
        )
        .await;

        drop(runtime);
        plugin_loader::flush_recorders().await;

        let outcome = run_outcome.context("running agent via IR interpreter")?;
        render_outcome(outcome, output)
    }
}

/// Shared helper: lower a `ProjectConfig` to an `IrModule`.
///
/// Uses `TargetTriple::PASSTHROUGH` (no capability-fit gate) so that `tau
/// dev` works on any host regardless of sandbox tier. All three caches are
/// stubs (`|_| None`) — MCP contracts and skill hashes are not pinned in
/// dev mode.
///
/// Used by both `load` (Phase 2) and the `:reload` command (Phase 5).
fn lower_project_to_ir(project: &ProjectConfig) -> Result<IrModule> {
    let caches = Caches {
        native_tool: &|_| None,
        mcp_contract: &|_| None,
        skill: &|_| None,
    };
    lower_project(project, &TargetTriple::PASSTHROUGH, &caches)
        .map_err(|e| anyhow!("IR lowering failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn minimal_project() -> assert_fs::TempDir {
        let tmp = assert_fs::TempDir::new().expect("tmpdir");
        tmp.child("tau.toml")
            .write_str(
                r#"
[project]
name = "dev-test"

[agents.fan-monitor]
display_name = "Fan Monitor"
package      = "fan-monitor@^0.1"
llm_backend  = "anthropic"
prompt.system = "Test agent"
"#,
            )
            .expect("write");
        tmp
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_succeeds_for_minimal_project() {
        let tmp = minimal_project();
        let session = DevSession::load(tmp.path().to_path_buf(), None)
            .await
            .expect("load");
        assert_eq!(session.current_agent_name(), "fan-monitor");
        assert!(session.history().is_empty(), "fresh session has no history");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_fails_for_missing_tau_toml() {
        let tmp = assert_fs::TempDir::new().expect("tmpdir");
        let err = DevSession::load(tmp.path().to_path_buf(), None)
            .await
            .expect_err("should fail");
        let msg = format!("{err}");
        assert!(
            msg.contains("tau.toml") || msg.contains("not found") || msg.contains("No such file"),
            "expected tau.toml mention, got: {msg}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_with_override_agent_picks_it() {
        let tmp = assert_fs::TempDir::new().expect("tmpdir");
        tmp.child("tau.toml")
            .write_str(
                r#"
[project]
name = "dev-test"

[agents.first]
display_name = "First Agent"
package      = "first@^0.1"
llm_backend  = "anthropic"
prompt.system = "First"

[agents.second]
display_name = "Second Agent"
package      = "second@^0.1"
llm_backend  = "anthropic"
prompt.system = "Second"
"#,
            )
            .expect("write");
        let session = DevSession::load(tmp.path().to_path_buf(), Some("second".into()))
            .await
            .expect("load");
        assert_eq!(session.current_agent_name(), "second");
    }
}
