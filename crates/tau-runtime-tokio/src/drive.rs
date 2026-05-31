//! Tokio-shell zero-config entry: drive a [`Runtime`] with the production
//! [`TokioClock`] + [`OsRandom`] defaults wired into
//! [`tau_runtime_core::options::RunOptions`].
//!
//! This is the canonical production entry point for the tokio host shell.
//! [`crate::runtime_ext::spawn_root_agent_with_scope`] retains
//! `MockClock` + `DeterministicRandom` for tests that depend on
//! deterministic run IDs.

use std::path::PathBuf;
use std::sync::Arc;

use tau_domain::{AgentDefinition, Message, PackageManifest};
use tau_ports::{Clock, RandomSource, RunBudget, RunSnapshot};

use crate::builder::Runtime;
use crate::error::RuntimeError;
use crate::{OsRandom, TokioClock};

/// Drive a multi-agent orchestrated run with tokio-shell defaults.
///
/// Mints the run-id from [`OsRandom`], stamps wall-clock from
/// [`TokioClock`], builds the JSONL run-log subscriber from `scope_root`,
/// and delegates to [`Runtime::spawn_root_agent_inner`].
///
/// CLI / workflow callers reach this through
/// [`crate::runtime_ext::spawn_root_agent_with_scope`] only when they want
/// deterministic test fixtures; production paths should call `drive` so
/// run IDs and timestamps reflect real wall-clock.
pub async fn drive(
    runtime: Arc<Runtime>,
    root_agent_def: AgentDefinition,
    root_manifest: PackageManifest,
    initial_message: Message,
    budget: RunBudget,
    scope_root: PathBuf,
) -> Result<RunSnapshot, RuntimeError> {
    let clock: Arc<dyn Clock> = Arc::new(TokioClock);
    let random: Arc<dyn RandomSource> = Arc::new(OsRandom);

    let run_id = tau_runtime_core::ids::ulid(&clock, &random);

    let log_path = crate::orchestration::persistence::run_log_path(&scope_root, &run_id);
    let subscriber = crate::orchestration::trace_mpsc::channel_with_writer(log_path);

    let scope_root_str = scope_root.to_string_lossy().into_owned();
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
        )
        .await
        .map_err(RuntimeError::Core)
}
