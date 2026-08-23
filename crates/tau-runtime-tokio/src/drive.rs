//! Tokio-shell zero-config entry: drive a [`Runtime`] with the production
//! [`TokioClock`] + [`OsRandom`] defaults wired into
//! [`tau_runtime_core::options::RunOptions`].
//!
//! This is the canonical production entry point for the tokio host shell.
//! [`crate::runtime_ext::spawn_root_agent_with_scope`] retains
//! `MockClock` + `DeterministicRandom` for tests that depend on
//! deterministic run IDs.

use core::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use tau_domain::{AgentDefinition, Message, PackageManifest};
use tau_ports::{Clock, RandomSource, RunBudget, RunSnapshot, TraceEvent};
use tokio::sync::mpsc::UnboundedReceiver;

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
///
/// This is the non-observing entry point: it attaches only the JSONL
/// run-log writer and awaits the run to completion. Callers that want to
/// render the trace live during the run use [`drive_with_live_trace`].
pub async fn drive(
    runtime: Arc<Runtime>,
    root_agent_def: AgentDefinition,
    root_manifest: PackageManifest,
    initial_message: Message,
    budget: RunBudget,
    scope_root: PathBuf,
) -> Result<RunSnapshot, RuntimeError> {
    let (rx, run) = drive_with_live_trace(
        runtime,
        root_agent_def,
        root_manifest,
        initial_message,
        budget,
        scope_root,
    );
    // No live consumer: drop the receiver so the live subscriber's sends
    // become no-ops (its channel is closed). The JSONL writer subscriber is
    // unaffected, so persistence is identical to the un-observed path.
    drop(rx);
    run.await
}

/// Like [`drive`], but attaches a second, live trace subscriber and hands
/// its receiver back so the caller can render the trace *as the run
/// progresses* (issue #469) rather than reading the JSONL log post-hoc.
///
/// Returns `(rx, run)`:
///
/// - `rx` yields every [`TraceEvent`] as it is emitted; it closes once the
///   run ends and every subscriber (including the one held by the run
///   future) is dropped.
/// - `run` is the run future; awaiting it yields the final
///   [`RunSnapshot`].
///
/// Drive both concurrently — e.g. `tokio::join!(run, printer(rx))`. The
/// JSONL run-log writer is attached exactly as in [`drive`], so
/// persistence is unchanged; the live subscriber is purely additive.
///
/// The returned future is `'static` (it owns every input), so callers may
/// also `tokio::spawn` it if they prefer.
pub fn drive_with_live_trace(
    runtime: Arc<Runtime>,
    root_agent_def: AgentDefinition,
    root_manifest: PackageManifest,
    initial_message: Message,
    budget: RunBudget,
    scope_root: PathBuf,
) -> (
    UnboundedReceiver<TraceEvent>,
    impl Future<Output = Result<RunSnapshot, RuntimeError>>,
) {
    let clock: Arc<dyn Clock> = Arc::new(TokioClock);
    let random: Arc<dyn RandomSource> = Arc::new(OsRandom);

    let run_id = tau_runtime_core::ids::ulid(&clock, &random);

    let log_path = crate::orchestration::persistence::run_log_path(&scope_root, &run_id);
    let writer_subscriber = crate::orchestration::trace_mpsc::channel_with_writer(log_path);

    // Second subscriber: a live receiver handed back to the caller. The
    // orchestration core's `TraceStream` already fans out to N subscribers,
    // so this rides alongside the JSONL writer with no changes to the core.
    let (live_subscriber, rx) = crate::orchestration::trace_mpsc::MpscTraceSubscriber::channel();

    let scope_root_str = scope_root.to_string_lossy().into_owned();
    let skill_resolver: Arc<dyn tau_ports::SkillResolver> = Arc::new(
        crate::skill_resolver_impl::TauPkgSkillResolver::new(scope_root.clone()),
    );

    let run = async move {
        runtime
            .spawn_root_agent_inner(
                root_agent_def,
                root_manifest,
                initial_message,
                budget,
                vec![writer_subscriber, Arc::new(live_subscriber)],
                Some(clock),
                Some(random),
                Some(run_id),
                Some(scope_root_str),
                Some(skill_resolver),
            )
            .await
            .map_err(RuntimeError::Core)
    };

    (rx, run)
}
