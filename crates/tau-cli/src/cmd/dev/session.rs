//! `DevSession` — owns the loaded project, IR, history, and (Phase 4+)
//! the file watcher + MCP client cache.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tau_domain::Message;
use tau_ir::lower::{lower_project, Caches};
use tau_ir::IrModule;
use tau_pkg::project::ProjectConfig;
use tau_ports::target::TargetTriple;

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
        let project = ProjectConfig::parse_str(toml_str)
            .map_err(|e| anyhow!("parse tau.toml: {e}"))?;

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

        let notify_handle =
            match crate::cmd::dev::watcher::spawn(&project_root, &project, pending_reload.clone())
            {
                Ok(w) => Some(w),
                Err(e) => {
                    eprintln!(
                        "warning: file watcher unavailable ({e}); use :reload manually"
                    );
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
