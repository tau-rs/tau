//! Engine-sequenced pipeline executor.
//!
//! Runs `IrModule.workflow.pipeline` steps in order, threading each
//! step's output through an [`OutputStore`] so `${steps.<id>.output}`
//! resolves. Agent, tool, and deterministic steps are all supported.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;

use serde_json::Value;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_ir::check::{CheckVerify, OnFail};
use tau_ir::pipeline::StepRun;
use tau_ir::IrModule;

use tracing::{info_span, Instrument};

use crate::error::RuntimeError;
use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::check::{evaluate_deliverable, evaluate_goal};
use crate::interpreter::output_store::OutputStore;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;
use crate::vocabulary::{
    EV_CHECK_EVALUATED, EV_CHECK_RETRY, EV_PIPELINE_STEP_COMPLETED, EV_PIPELINE_STEP_STARTED,
    SPAN_PIPELINE_CHECK, SPAN_PIPELINE_STEP,
};

/// Max `Parallel` branches in flight at once (nginx `worker_connections`
/// analogue). Bounded cooperative fork-join — see ADR-0059.
const PARALLEL_CAP: usize = 8;

/// Terminal outcome of driving a pipeline via [`run_pipeline_suspendable`].
#[derive(Debug)]
pub enum PipelineOutcome {
    /// Ran to the end; carries the accumulated outputs.
    Completed(OutputStore),
    /// Paused at a top-level `Suspend`. State was persisted via the
    /// `SuspensionStore` passed in `SuspendConfig` before returning.
    Suspended {
        /// The run id the suspension was persisted under.
        run_id: String,
        /// The signal name a `--resume --signal <name>` must supply.
        resume_signal: String,
        /// The pipeline-step id of the `Suspend` block that paused.
        step_id: String,
    },
}

/// How a `run_steps` slice terminated. Only the top-level slice (the one
/// `run_pipeline_suspendable` invokes directly, with `suspend: Some(_)`) can
/// terminate `Suspended`; nested slices (`Branch`/`Loop`/`Parallel` bodies)
/// always pass `suspend: None` and must complete or error.
enum StepsFlow {
    /// The slice ran every step.
    Completed,
    /// The slice paused at a `Suspend` step.
    Suspended {
        /// The pipeline-step id of the `Suspend` block.
        step_id: String,
        /// The signal name the resumer must supply.
        resume_signal: String,
    },
}

/// Durable-suspend wiring for the top-level pipeline driver. Carries the
/// run id (the suspension key + the `--resume` handle) and the store a
/// `Suspend` step's snapshot is persisted to.
pub struct SuspendConfig {
    /// Run id used as the suspension key + `--resume` handle.
    pub run_id: String,
    /// Where a pause is durably persisted.
    pub store: Arc<dyn tau_ports::orchestration::SuspensionStore>,
}

/// Rehydrated state for a resumed run: the `OutputStore` snapshot restored
/// from the prior suspension, and the top-level step index to resume at
/// (`step_cursor + 1`, so the `Suspend` step itself is not re-run).
pub struct ResumeState {
    /// The `OutputStore` snapshot restored from the suspension.
    pub store: OutputStore,
    /// Top-level step index to resume at (`step_cursor + 1`).
    pub start_at: usize,
    /// Per-check attempt counts restored from the prior suspension, so a
    /// check's `max_attempts` budget accumulates across resume boundaries
    /// instead of resetting each resume. Fresh runs pass an empty map.
    pub attempts: BTreeMap<String, u32>,
}

/// Internal: passed down to `run_steps` for the top-level slice only. A
/// nested slice (Branch/Loop/Parallel body) receives `None`, which is how
/// `run_steps` tells a real `Suspend` step from one that is out of place.
struct SuspendCtx<'a> {
    run_id: &'a str,
    ir_digest: &'a str,
    store: &'a dyn tau_ports::orchestration::SuspensionStore,
}

/// Drive an `IrModule`'s pipeline, pausing durably at a top-level `Suspend`
/// step rather than erroring.
///
/// The module's `workflow.pipeline` must be `Some` — callers branch on
/// that before dispatching here (see `tau run`). Each step renders its
/// `input` template against the run `input` and prior outputs, runs the
/// step, and records the step's output keyed by its pipeline-step id so
/// later steps can reference it via `${steps.<id>.output}`.
///
/// [`StepRun::Agent`], [`StepRun::Tool`], [`StepRun::Deterministic`],
/// [`StepRun::Check`], [`StepRun::Branch`], [`StepRun::Loop`],
/// [`StepRun::Parallel`], and [`StepRun::Suspend`] steps are all supported.
/// [`StepRun::Dynamic`] is recognized but not yet executed: it errors
/// [`RuntimeError::DynamicRegionRequiresRuntimeGate`] pending the EPIC 4.5
/// runtime gate (membership, attenuation, bounds counters). A
/// `Check` step evaluates a postcondition (goal or deliverable) against the
/// accumulated outputs. A `Branch` evaluates its condition and recurses into
/// the chosen arm (`then`/`otherwise`) against the same shared store; it
/// stores no output of its own. A `Loop` runs its `body` up to `max_iters`
/// times, checking `until` after each pass and threading a failed verdict's
/// rationale into the next pass as feedback; exhausting `max_iters` without
/// `until` holding is a hard error ([`RuntimeError::LoopExhausted`]). Like
/// `Branch`, a `Loop` stores no output of its own. A `Parallel` forks each
/// branch over an isolated read-only snapshot of the store (branches cannot
/// see each other's outputs), drives them with bounded concurrency
/// (`PARALLEL_CAP` in flight at once), and merges each branch's produced
/// outputs back into the shared store in index order once all branches
/// complete; like `Branch` and `Loop`, it stores no output of its own. A
/// top-level `Suspend` step pauses the run: computes the module's
/// canonical-IR digest once (for drift-detection on a future resume), seeds
/// the run's `OutputStore` and start index either fresh (`resume: None`) or
/// from a prior pause (`resume: Some(ResumeState)` — restore-and-continue:
/// the store is rehydrated from the persisted snapshot and the walk resumes
/// at `step_cursor + 1`, so the prefix up to and including the `Suspend`
/// step is never re-run), and — on hitting `Suspend` — durably persists a
/// [`tau_ports::orchestration::PipelineSuspension`] via the `SuspensionStore`
/// in `SuspendConfig` and returns [`PipelineOutcome::Suspended`] rather than
/// erroring. Only a top-level `Suspend` may pause; one nested inside a
/// `Branch`/`Loop`/`Parallel` body is a build-time typecheck error (an
/// [`RuntimeError::Internal`] here would mean that check regressed).
///
/// # Failure handling: abort, or rewind-to-gate retry
///
/// On a failed check the [`RetryPolicy`](tau_ir::check::RetryPolicy)
/// decides what happens. If `on_fail == Abort`, or the attempt count has
/// reached `max_attempts`, the pipeline aborts with
/// [`RuntimeError::CheckFailed`]. Otherwise the loop emits
/// [`EV_CHECK_RETRY`], stashes the judge's rationale as `feedback` keyed by
/// the check's `gate` step and the check's id, and rewinds the loop index to
/// that `gate` step so the forward slice re-runs. On the next pass through the
/// gate's agent step every outstanding rationale registered for *that* gate is
/// injected as a labelled prior turn (`"Previous attempt rejected: (check
/// '<id>') <rationale>"`) so the agent sees *why* it was rejected. Keying by
/// gate + check id keeps concurrently-failing checks distinct (a pass clears
/// only its own check's entry; a fresh failure updates only its own) and
/// confines each check's feedback to its own gate step.
pub async fn run_pipeline_suspendable<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
    suspend: SuspendConfig,
    resume: Option<ResumeState>,
) -> Result<PipelineOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let pipeline = module
        .workflow
        .pipeline
        .clone()
        .ok_or_else(|| RuntimeError::Internal {
            message: "run_pipeline called on a module without a pipeline".to_string(),
        })?;

    // Canonical-IR digest, computed once, for drift-detection on resume.
    let ir_digest = {
        let bytes = tau_ir::to_canonical_bytes(&module);
        tau_ir::asset::asset_hash(&bytes)
    };

    let (mut store, start_at, initial_attempts) = match resume {
        Some(r) => (r.store, r.start_at, r.attempts),
        None => (OutputStore::new(), 0, BTreeMap::new()),
    };

    let ctx = SuspendCtx {
        run_id: &suspend.run_id,
        ir_digest: &ir_digest,
        store: suspend.store.as_ref(),
    };
    let flow = run_steps(
        &module,
        &pipeline.steps,
        &input,
        &mut store,
        &dispatcher,
        None,
        Some(&ctx),
        start_at,
        initial_attempts,
    )
    .await?;
    Ok(match flow {
        StepsFlow::Completed => PipelineOutcome::Completed(store),
        StepsFlow::Suspended {
            step_id,
            resume_signal,
        } => PipelineOutcome::Suspended {
            run_id: suspend.run_id,
            resume_signal,
            step_id,
        },
    })
}

/// Non-suspend convenience wrapper over [`run_pipeline_suspendable`] for
/// callers that cannot pause (e.g. the bundle-run path in v1). Wires a
/// no-op `SuspensionStore` and runs fresh (never resumes); a pipeline that
/// hits a `Suspend` step errors [`RuntimeError::SuspendUnsupported`] instead
/// of pausing. Callers that support HITL suspend/resume use
/// `run_pipeline_suspendable` directly.
pub async fn run_pipeline<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
) -> Result<OutputStore, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    struct NoopSuspensions;
    impl tau_ports::orchestration::SuspensionStore for NoopSuspensions {
        fn persist_suspension(
            &self,
            _: &tau_ports::orchestration::PipelineSuspension,
        ) -> Result<(), tau_ports::orchestration::CheckpointError> {
            Ok(())
        }
        fn load_suspension(
            &self,
            _: &tau_ports::orchestration::RunId,
        ) -> Result<
            Option<tau_ports::orchestration::PipelineSuspension>,
            tau_ports::orchestration::CheckpointError,
        > {
            Ok(None)
        }
    }
    let outcome = run_pipeline_suspendable(
        module,
        input,
        dispatcher,
        SuspendConfig {
            run_id: String::new(),
            store: Arc::new(NoopSuspensions),
        },
        None,
    )
    .await?;
    match outcome {
        PipelineOutcome::Completed(store) => Ok(store),
        PipelineOutcome::Suspended {
            step_id,
            resume_signal,
            ..
        } => Err(RuntimeError::SuspendUnsupported {
            step: step_id,
            resume_signal,
        }),
    }
}

/// Execute a slice of pipeline steps against a shared [`OutputStore`].
///
/// Extracted from [`run_pipeline`] so nested blocks (Branch/Parallel/Loop)
/// can drive their own step slices with the same gate-rewind, feedback, and
/// check-dispatch semantics. `initial_feedback` seeds the per-slice feedback
/// carried into the first gate agent step (used when a nested slice re-runs
/// with a prior rejection rationale); top-level callers pass `None`.
///
/// See [`run_pipeline`]'s docs for the abort / rewind-to-gate retry model —
/// the loop body lives here.
#[allow(clippy::too_many_arguments)]
async fn run_steps<D>(
    module: &Arc<IrModule>,
    steps: &[tau_ir::pipeline::PipelineStep],
    input: &str,
    store: &mut OutputStore,
    dispatcher: &Arc<D>,
    initial_feedback: Option<String>,
    suspend: Option<&SuspendCtx<'_>>,
    start_at: usize,
    initial_attempts: BTreeMap<String, u32>,
) -> Result<StepsFlow, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    // Per-check attempt counter (1-based on first eval), keyed by check id.
    // Seeded from `initial_attempts` on a resumed top-level slice so a check's
    // `max_attempts` budget accumulates across resume boundaries; every
    // nested slice (Branch/Loop/Parallel body) passes an empty map.
    let mut attempts: BTreeMap<String, u32> = initial_attempts;
    // Rejection rationales of failed checks that triggered a rewind, keyed by
    // the gate step they rewind to, then by check id (issue #471). Injecting
    // only the entries whose key matches the current step id scopes each
    // check's feedback to its own gate agent step (never leaking into
    // unrelated agent steps in the rewound slice). Multiple checks gating one
    // step keep distinct entries — a pass clears only its own check's entry, a
    // fresh failure updates only its own — instead of last-writer-wins over a
    // single shared slot.
    let mut feedback: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    // Loop prior-iteration rationale (from `Loop.until`), which has no
    // associated check or gate. Injected once, at the first agent step of this
    // slice, then consumed. Top-level/branch/parallel callers pass `None`.
    let mut pending_initial_feedback: Option<String> = initial_feedback;
    // gate-id -> pipeline-step index, so a failed retryable check can rewind
    // `i` to its gate without re-scanning the pipeline each time.
    let gate_index: BTreeMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(idx, step)| (step.id.0.as_str(), idx))
        .collect();

    // Index loop (not `for`) so a failed retryable check can rewind `i` to a
    // gate step. On the happy path `i` only ever advances by one. `start_at`
    // is nonzero only on a resumed top-level slice (skips the already-run
    // prefix up to and including the prior `Suspend` step); every recursive
    // call passes `0`.
    let mut i = start_at;
    while i < steps.len() {
        let step = &steps[i];

        // Check steps have their own span/event vocabulary and store no
        // output, so dispatch them before the Agent/Tool/Deterministic path.
        if let StepRun::Check(check_id) = &step.run {
            let check =
                module
                    .workflow
                    .checks
                    .get(check_id)
                    .ok_or_else(|| RuntimeError::Internal {
                        message: format!("unknown check {}", check_id.0),
                    })?;

            let check_span = info_span!(SPAN_PIPELINE_CHECK, id = check_id.0.as_str());

            let (verdict, kind) = match &check.verify {
                CheckVerify::Goal {
                    evaluates,
                    predicate,
                } => {
                    let reg = dispatcher.deterministic_registry().ok_or_else(|| {
                        RuntimeError::Internal {
                            message: format!("check {} needs a deterministic registry", check_id.0),
                        }
                    })?;
                    // Bind the reader to a local so the `Arc` temporary lives
                    // across the borrow passed to `evaluate_goal`.
                    let reader = dispatcher.artifact_reader();
                    let verdict = evaluate_goal(
                        evaluates,
                        predicate,
                        store,
                        reader.as_ref().map(|a| a.as_ref()),
                        reg.as_ref(),
                    )?;
                    (verdict, "goal")
                }
                CheckVerify::Deliverable {
                    locus,
                    must_satisfy,
                    judge,
                } => {
                    let reader = dispatcher.artifact_reader();
                    let verdict = Box::pin(evaluate_deliverable(
                        module.clone(),
                        locus,
                        must_satisfy,
                        judge,
                        store,
                        reader.as_ref().map(|a| a.as_ref()),
                        dispatcher.clone(),
                    ))
                    .instrument(check_span.clone())
                    .await?;
                    (verdict, "deliverable")
                }
            };

            // Real (1-based) attempt count for this check.
            let attempt = attempts.get(&check_id.0).copied().unwrap_or(0) + 1;
            attempts.insert(check_id.0.clone(), attempt);

            tracing::info!(
                parent: &check_span,
                name = EV_CHECK_EVALUATED,
                id = check_id.0.as_str(),
                kind = kind,
                verdict = if verdict.met { "pass" } else { "fail" },
                attempt = attempt,
            );

            if verdict.met {
                // Passed: clear only THIS check's feedback under its gate,
                // leaving any sibling checks' outstanding rationales intact.
                // Checks store no output.
                if let Some(inner) = feedback.get_mut(&check.retry.gate.0) {
                    inner.remove(&check_id.0);
                    if inner.is_empty() {
                        feedback.remove(&check.retry.gate.0);
                    }
                }
                i += 1;
                continue;
            }

            // Failed. Abort if the policy says so, or if we've exhausted the
            // attempt budget.
            if check.retry.on_fail == OnFail::Abort || attempt >= check.retry.max_attempts {
                return Err(RuntimeError::CheckFailed {
                    id: check_id.0.clone(),
                    kind: String::from(kind),
                    rationale: verdict.rationale,
                    attempt,
                });
            }

            // Retryable with attempts remaining: rewind to the gate step.
            let gate_idx = *gate_index.get(check.retry.gate.0.as_str()).ok_or_else(|| {
                // Build-time integrity checks (tau-ir typecheck) guarantee the
                // gate names an existing pipeline step, so reaching here is an
                // invariant violation — surface it loudly rather than aborting
                // the run as a plain check failure.
                RuntimeError::Internal {
                    message: format!(
                        "check {} rewinds to unknown gate step {}",
                        check_id.0, check.retry.gate.0
                    ),
                }
            })?;

            tracing::info!(
                parent: &check_span,
                name = EV_CHECK_RETRY,
                id = check_id.0.as_str(),
                rewind_to = check.retry.gate.0.as_str(),
                next_attempt = attempt + 1,
            );

            feedback
                .entry(check.retry.gate.0.clone())
                .or_default()
                .insert(check_id.0.clone(), verdict.rationale);
            i = gate_idx;
            continue;
        }

        // `Branch` blocks have their own early dispatch, mirroring `Check`
        // above: they store no output of their own (the chosen arm's steps
        // record their own outputs into the shared `store` as they run).
        if let StepRun::Branch {
            on,
            then,
            otherwise,
        } = &step.run
        {
            let reg =
                dispatcher
                    .deterministic_registry()
                    .ok_or_else(|| RuntimeError::Internal {
                        message: format!("branch {} needs a deterministic registry", step.id.0),
                    })?;
            let reader = dispatcher.artifact_reader();
            let verdict = crate::interpreter::check::eval_condition(
                on,
                store,
                reader.as_ref().map(|a| a.as_ref()),
                reg.as_ref(),
            )?;
            let arm = if verdict.met { then } else { otherwise };
            match Box::pin(run_steps(
                module,
                arm,
                input,
                store,
                dispatcher,
                None,
                None,
                0,
                BTreeMap::new(),
            ))
            .await?
            {
                StepsFlow::Completed => {}
                StepsFlow::Suspended { .. } => {
                    return Err(RuntimeError::Internal {
                        message: "suspend escaped a nested slice (typecheck should reject)"
                            .to_string(),
                    })
                }
            }
            i += 1;
            continue;
        }

        // `Loop` blocks have their own early dispatch, mirroring `Check` and
        // `Branch` above: they store no output of their own (the body's
        // steps record their own outputs into the shared `store` as they
        // run, each pass overwriting the prior pass's). Runs `body` up to
        // `max_iters` times, checking `until` after each pass; converges as
        // soon as `until` holds, otherwise threads the failed verdict's
        // rationale into the next pass as feedback (same
        // "Previous attempt rejected: <rationale>" idiom `Check`'s
        // rewind-to-gate retry uses). Exhausting `max_iters` without `until`
        // holding is a hard error (ADR-0058 / ADR-0059) — a bounded loop
        // that cannot reach its goal is a failure, not a silent success.
        if let StepRun::Loop {
            body,
            until,
            max_iters,
        } = &step.run
        {
            let mut loop_feedback: Option<String> = None;
            let mut converged = false;
            for _iter in 0..*max_iters {
                match Box::pin(run_steps(
                    module,
                    body,
                    input,
                    store,
                    dispatcher,
                    loop_feedback.take(),
                    None,
                    0,
                    BTreeMap::new(),
                ))
                .await?
                {
                    StepsFlow::Completed => {}
                    StepsFlow::Suspended { .. } => {
                        return Err(RuntimeError::Internal {
                            message: "suspend escaped a nested slice (typecheck should reject)"
                                .to_string(),
                        })
                    }
                }
                let reg =
                    dispatcher
                        .deterministic_registry()
                        .ok_or_else(|| RuntimeError::Internal {
                            message: format!("loop {} needs a deterministic registry", step.id.0),
                        })?;
                let reader = dispatcher.artifact_reader();
                let verdict = crate::interpreter::check::eval_condition(
                    until,
                    store,
                    reader.as_ref().map(|a| a.as_ref()),
                    reg.as_ref(),
                )?;
                if verdict.met {
                    converged = true;
                    break;
                }
                loop_feedback = Some(verdict.rationale);
            }
            if !converged {
                return Err(RuntimeError::LoopExhausted {
                    step: step.id.0.clone(),
                    max_iters: *max_iters,
                });
            }
            i += 1;
            continue;
        }

        // `Suspend` blocks have their own early dispatch, mirroring `Branch`,
        // `Loop`, and `Parallel` above. Only the top-level slice carries a
        // `SuspendCtx` (nested Branch/Loop/Parallel bodies pass `None` —
        // typecheck rejects a `Suspend` nested inside those, so `None` here
        // is an invariant violation, not a normal case). With a ctx: persist
        // the pause (run id, resume signal, step cursor, IR digest, and a
        // snapshot of the store so far) and unwind to the caller as
        // `StepsFlow::Suspended` rather than erroring.
        if let StepRun::Suspend { resume_signal } = &step.run {
            match suspend {
                Some(ctx) => {
                    ctx.store
                        .persist_suspension(&tau_ports::orchestration::PipelineSuspension {
                            run_id: ctx.run_id.to_string(),
                            resume_signal: resume_signal.clone(),
                            step_cursor: i,
                            step_id: step.id.0.clone(),
                            ir_digest: ctx.ir_digest.to_string(),
                            outputs: store.snapshot(),
                            attempts: attempts.clone(),
                        })
                        .map_err(|e| RuntimeError::Internal {
                            message: format!("persist suspension: {e}"),
                        })?;
                    return Ok(StepsFlow::Suspended {
                        step_id: step.id.0.clone(),
                        resume_signal: resume_signal.clone(),
                    });
                }
                None => {
                    return Err(RuntimeError::SuspendUnsupported {
                        step: step.id.0.clone(),
                        resume_signal: resume_signal.clone(),
                    })
                }
            }
        }

        // `Dynamic` regions have their own early dispatch, mirroring
        // `Branch`/`Loop`/`Suspend` above. Real execution (membership,
        // attenuation, bounds counters) lands in EPIC 4.5; until then the
        // interpreter meets a `Dynamic` region with a named error rather
        // than executing or silently skipping it.
        if let StepRun::Dynamic { .. } = &step.run {
            return Err(RuntimeError::DynamicRegionRequiresRuntimeGate {
                step_id: step.id.0.clone(),
            });
        }

        // `Parallel` blocks have their own early dispatch, mirroring `Branch`
        // and `Loop` above: they store no output of their own. Each branch
        // forks a read-only snapshot of `store` (branches are read-isolated
        // from each other — a branch cannot see a sibling's outputs, only
        // the pre-fork state), runs to completion, and returns its produced
        // store. Branches are driven with bounded concurrency
        // (`PARALLEL_CAP` in flight at once, nginx `worker_connections`
        // style) and their outputs are merged back into the shared store in
        // index order once all branches complete, so the merge is
        // deterministic regardless of which branch's future resolves first.
        if let StepRun::Parallel { branches } = &step.run {
            use futures_util::stream::{self, StreamExt};
            let futs = branches.iter().enumerate().map(|(idx, branch)| {
                let module = module.clone();
                let dispatcher = dispatcher.clone();
                let input = input.to_string();
                let mut snap = store.clone();
                async move {
                    match run_steps(
                        &module,
                        branch,
                        &input,
                        &mut snap,
                        &dispatcher,
                        None,
                        None,
                        0,
                        BTreeMap::new(),
                    )
                    .await?
                    {
                        StepsFlow::Completed => {}
                        StepsFlow::Suspended { .. } => {
                            return Err(RuntimeError::Internal {
                                message: "suspend escaped a nested slice (typecheck should reject)"
                                    .to_string(),
                            })
                        }
                    }
                    Ok::<(usize, OutputStore), RuntimeError>((idx, snap))
                }
            });
            let mut results: alloc::vec::Vec<(usize, OutputStore)> = stream::iter(futs)
                .buffered(PARALLEL_CAP)
                .collect::<alloc::vec::Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<_, _>>()?;
            // `buffered` preserves input order, but sort by index defensively
            // so the merge is deterministic regardless of driver scheduling.
            results.sort_by_key(|(idx, _)| *idx);
            for (_, produced) in results {
                store.merge(produced);
            }
            i += 1;
            continue;
        }

        // NOTE: we do NOT call `.entered()` here — `EnteredSpan` mutates a
        // thread-local span stack, and tokio's multi-thread scheduler can
        // move this task to a different worker thread at any `.await`,
        // leaving the guard on the wrong thread and mis-parenting child
        // spans/events. Instead every event uses `parent: &step_span`
        // explicitly, and every awaited future is wrapped with
        // `.instrument(step_span.clone())`. See stream.rs:273-283 for the
        // same idiom applied to the runtime turn span.
        let step_span = info_span!(SPAN_PIPELINE_STEP, id = step.id.0.as_str());
        tracing::info!(parent: &step_span, name = EV_PIPELINE_STEP_STARTED, id = step.id.0.as_str());

        let rendered = tau_ir::template::resolve(&step.input, input, &store.template_map())
            .map_err(|e| RuntimeError::Internal {
                message: format!("pipeline step {}: {e}", step.id.0),
            })?;

        let output: Value = match &step.run {
            StepRun::Agent(agent_id) => {
                let agent = module
                    .workflow
                    .agents
                    .get(agent_id)
                    .ok_or_else(|| RuntimeError::AgentNotFound {
                        agent: agent_id.0.clone(),
                    })?
                    .clone();
                // When a prior check rewound to this gate, inject its
                // rationale as a prior turn so the agent sees *why* it was
                // rejected. `split_history` (agent_loop.rs) treats all-but-last
                // as history and the last as the live turn, so each feedback
                // message becomes a prior turn and `rendered` stays the live
                // prompt.
                let mut initial: alloc::vec::Vec<Message> = alloc::vec::Vec::new();
                // Loop prior-iteration feedback (no check/gate): inject once,
                // at the first agent step of the slice, then consume it.
                if let Some(fb) = pending_initial_feedback.take() {
                    initial.push(user_message(&format!("Previous attempt rejected: {fb}")));
                }
                // Check feedback gating THIS step: one labeled prior turn per
                // outstanding check, so several checks on one gate stay
                // distinct (issue #471). BTreeMap iteration is sorted by check
                // id, so ordering is deterministic.
                if let Some(inner) = feedback.get(&step.id.0) {
                    for (cid, fb) in inner {
                        initial.push(user_message(&format!(
                            "Previous attempt rejected: (check '{cid}') {fb}"
                        )));
                    }
                }
                initial.push(user_message(&rendered));
                let outcome = Box::pin(run_agent(
                    module.clone(),
                    &agent,
                    dispatcher.clone(),
                    initial,
                ))
                .instrument(step_span.clone())
                .await?;
                match outcome {
                    RunOutcome::Failed { status, .. } => {
                        return Err(RuntimeError::Internal {
                            message: format!(
                                "pipeline step {} (agent {}) failed: {status:?}",
                                step.id.0, agent_id.0
                            ),
                        })
                    }
                    _ => Value::String(last_assistant_text(&outcome)),
                }
            }
            StepRun::Tool(tool_id) => {
                let args = rendered_to_args(&rendered);
                let result = dispatcher
                    .invoke(tool_id, &args)
                    .instrument(step_span.clone())
                    .await?;
                match (result.body, result.error) {
                    (Some(body), _) => body,
                    (None, Some(err)) => {
                        return Err(RuntimeError::Internal {
                            message: alloc::format!(
                                "pipeline step {} (tool {}) errored: {err}",
                                step.id.0,
                                tool_id.0
                            ),
                        })
                    }
                    (None, None) => Value::Null,
                }
            }
            StepRun::Deterministic(step_node_id) => {
                let registry =
                    dispatcher
                        .deterministic_registry()
                        .ok_or_else(|| RuntimeError::Internal {
                            message: alloc::format!(
                                "pipeline step {} needs a deterministic registry, none provided",
                                step.id.0
                            ),
                        })?;
                let node = module.workflow.steps.get(step_node_id).ok_or_else(|| {
                    RuntimeError::Internal {
                        message: alloc::format!("unknown deterministic step {}", step_node_id.0),
                    }
                })?;
                let args = rendered_to_args(&rendered);
                crate::interpreter::deterministic::run_step(node, registry.as_ref(), &args)?
            }
            // `Check` steps are dispatched at the top of the loop and never
            // reach this `match` (they store no output of their own).
            StepRun::Check(_) => unreachable!("check steps are handled before this match"),
            // `Branch`, `Parallel`, `Loop`, `Suspend`, and `Dynamic` are all
            // early-dispatched above (each either stores no output of its
            // own, or — for `Suspend`/`Dynamic` — returns
            // `StepsFlow::Suspended`/errors before reaching here), so none of
            // these ever reach this `match`.
            StepRun::Branch { .. }
            | StepRun::Parallel { .. }
            | StepRun::Loop { .. }
            | StepRun::Suspend { .. }
            | StepRun::Dynamic { .. } => {
                unreachable!("control-flow blocks are early-dispatched")
            }
        };

        store.insert(step.id.0.clone(), output);
        tracing::info!(parent: &step_span, name = EV_PIPELINE_STEP_COMPLETED, id = step.id.0.as_str());

        i += 1;
    }

    Ok(StepsFlow::Completed)
}

/// Turn a rendered template string into the `Value` a tool/deterministic
/// step receives: parse it as JSON if it parses, else wrap as a string.
///
/// Footgun: a rendered string that happens to be a bare JSON scalar
/// (`42`, `true`, `null`) parses to that scalar rather than wrapping as a
/// string. Author tool/deterministic `input` templates accordingly.
fn rendered_to_args(rendered: &str) -> Value {
    serde_json::from_str::<Value>(rendered).unwrap_or_else(|_| Value::String(rendered.to_string()))
}

/// Build a user-turn [`Message`] carrying `content` as its text payload.
///
/// Mirrors the initial-message idiom in `tau-cli`'s `run` command: the
/// recipient is a freshly-minted [`AgentInstanceId`] placeholder that the
/// kernel replaces when it assigns the loop's own instance id.
///
/// Exposed as `pub(crate)` so `check.rs` can reuse it without a separate
/// definition (single source of truth).
pub(crate) fn user_message(content: &str) -> Message {
    // Use no_std-safe constructors (std-gated `Message::new` and
    // `AgentInstanceId::new` aren't available in the wasm-interpreter
    // build). The kernel replaces the agent instance id when it builds
    // the inner agent loop, so the placeholder zeros are ephemeral.
    use tau_domain::MessageId;
    Message::new_with(
        MessageId::from_parts(0, [0u8; 10]),
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap(),
        Address::User,
        Address::Agent(AgentInstanceId::from_parts(0, [0u8; 10])),
        MessagePayload::Text {
            content: content.to_string(),
        },
    )
}
