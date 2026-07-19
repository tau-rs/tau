//! Host-shell free-function wrapper for `Runtime::spawn_root_agent_inner`.
//!
//! Pre-β.1.3.5c this module hosted a `RuntimeShellExt` extension trait
//! providing `run`, `run_with_history`, `run_streaming`, `invoke_tool`,
//! `spawn_root_agent`, and `run_default` on
//! [`tau_runtime_core::Runtime`]. Those methods now live as inherent
//! methods on the kernel `Runtime` directly; only the JSONL-persistence
//! wiring of `spawn_root_agent` remained tokio-shell-specific, so this
//! module shrinks to a single free function that builds the subscriber
//! from the scope_root and delegates to the kernel inner.

use std::path::PathBuf;
use std::sync::Arc;

use tau_domain::{AgentDefinition, Message, PackageManifest};
use tau_ports::{RunBudget, RunSnapshot};

use crate::builder::Runtime;
use crate::error::RuntimeError;

/// Multi-agent orchestrated run entry point (ROADMAP §9, v1).
///
/// Builds the JSONL run-log subscriber from `scope_root` and delegates
/// to [`Runtime::spawn_root_agent_inner`].
pub async fn spawn_root_agent_with_scope(
    runtime: Arc<Runtime>,
    root_agent_def: AgentDefinition,
    root_manifest: PackageManifest,
    initial_message: Message,
    budget: RunBudget,
    scope_root: PathBuf,
) -> Result<RunSnapshot, RuntimeError> {
    // Mint the run-id up front so the same id flows into the JSONL path and the
    // RunState. Use the tokio shell's real clock/random so every orchestrated run
    // gets a unique, wall-clock-ordered ULID. (The previous MockClock +
    // DeterministicRandom::seeded(0) minted a FIXED run-id on every call — a
    // latent collision bug — and forced `tau-ports/test-fixtures` into the
    // production dependency graph.)
    let clock: Arc<dyn tau_ports::Clock> = Arc::new(crate::TokioClock);
    let random: Arc<dyn tau_ports::RandomSource> = Arc::new(crate::OsRandom);
    let run_id = tau_runtime_core::ids::ulid(&clock, &random);

    // Build the JSONL subscriber via the tokio mpsc channel.
    let log_path = crate::orchestration::persistence::run_log_path(&scope_root, &run_id);
    let subscriber = crate::orchestration::trace_mpsc::channel_with_writer(log_path);

    // Delegate to the core's inner. Passing `run_id` keeps the inner's
    // RunState in sync with the JSONL log path we just constructed.
    let scope_root_str = scope_root.to_string_lossy().into_owned();
    let skill_resolver: std::sync::Arc<dyn tau_ports::SkillResolver> = std::sync::Arc::new(
        crate::skill_resolver_impl::TauPkgSkillResolver::new(scope_root.clone()),
    );
    runtime
        .spawn_root_agent_inner(
            root_agent_def,
            root_manifest,
            initial_message,
            budget,
            vec![subscriber],
            Some(clock),
            Some(random),
            Some(run_id),
            Some(scope_root_str),
            Some(skill_resolver),
        )
        .await
        .map_err(RuntimeError::Core)
}
