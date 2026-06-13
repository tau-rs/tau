//! Engine-sequenced pipeline executor.
//!
//! Runs `IrModule.workflow.pipeline` steps in order, threading each
//! step's output through an [`OutputStore`] so `${steps.<id>.output}`
//! resolves. Agent, tool, and deterministic steps are all supported.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;

use serde_json::Value;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_ir::pipeline::StepRun;
use tau_ir::IrModule;

use tracing::{info_span, Instrument};

use crate::error::RuntimeError;
use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::output_store::OutputStore;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::options::TokenUsage;
use crate::outcome::{PipelineOutcome, PipelineStatus, RunOutcome};
use crate::vocabulary::{
    EV_CHECK_EVALUATED, EV_PIPELINE_STEP_COMPLETED, EV_PIPELINE_STEP_STARTED, SPAN_PIPELINE_STEP,
};

/// Drive an `IrModule`'s pipeline to completion, returning all step
/// outputs.
///
/// The module's `workflow.pipeline` must be `Some` — callers branch on
/// that before dispatching here (see `tau run`). Each step renders its
/// `input` template against the run `input` and prior outputs, runs the
/// step, and records the step's output keyed by its pipeline-step id so
/// later steps can reference it via `${steps.<id>.output}`.
///
/// [`StepRun::Agent`], [`StepRun::Tool`], and
/// [`StepRun::Deterministic`] steps are all supported.
pub async fn run_pipeline<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
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

    let mut store = OutputStore::new();
    let mut total_usage = TokenUsage::default();

    for step in &pipeline.steps {
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

        let rendered = tau_ir::template::resolve(&step.input, &input, &store.template_map())
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
                let initial = vec![user_message(&rendered)];
                let outcome = Box::pin(run_agent(
                    module.clone(),
                    &agent,
                    dispatcher.clone(),
                    initial,
                ))
                .instrument(step_span.clone())
                .await?;
                match &outcome {
                    RunOutcome::Failed { status, token_usage, .. } => {
                        total_usage.add(token_usage);
                        return Ok(PipelineOutcome {
                            outputs: store,
                            token_usage: total_usage,
                            status: PipelineStatus::AgentFailed {
                                step: step.id.0.clone(),
                                status: status.clone(),
                            },
                        });
                    }
                    RunOutcome::Completed { token_usage, .. } => {
                        total_usage.add(token_usage);
                        Value::String(last_assistant_text(&outcome))
                    }
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
            StepRun::Check(check_id) => {
                let check =
                    module.workflow.checks.get(check_id).ok_or_else(|| RuntimeError::Internal {
                        message: alloc::format!(
                            "pipeline step {} runs unknown check {}",
                            step.id.0,
                            check_id.0
                        ),
                    })?;

                // Clone the parts we need so the borrow on `module` doesn't
                // span the mutable `store` operations below.
                let evaluates = check.verify.clone();

                match evaluates {
                    tau_ir::check::CheckVerify::Goal { evaluates, predicate } => {
                        // Materialize the locus to bytes.
                        let bytes: Option<alloc::vec::Vec<u8>> = match &evaluates {
                            tau_ir::check::Locus::Output(step_id) => {
                                store.get(&step_id.0).map(|v| match v {
                                    serde_json::Value::String(s) => {
                                        s.clone().into_bytes()
                                    }
                                    other => other.to_string().into_bytes(),
                                })
                            }
                            tau_ir::check::Locus::Path(p) => {
                                match dispatcher.read_artifact(p) {
                                    Some(Ok(opt)) => opt,
                                    Some(Err(e)) => return Err(e),
                                    None => {
                                        return Err(RuntimeError::Internal {
                                            message: "check evaluates a filesystem path but \
                                                      the dispatcher provides no host filesystem"
                                                .into(),
                                        })
                                    }
                                }
                            }
                        };

                        // Evaluate the predicate. NativeFn routes through the
                        // deterministic registry; all other predicates use
                        // eval_predicate directly.
                        let (passed, rationale) = match &predicate {
                            tau_ir::check::Predicate::NativeFn(name) => {
                                let registry =
                                    dispatcher.deterministic_registry().ok_or_else(|| {
                                        RuntimeError::Internal {
                                            message: alloc::format!(
                                                "check {} uses native fn {name} but no \
                                                 deterministic registry is available",
                                                check_id.0
                                            ),
                                        }
                                    })?;
                                // Pass the locus value as args (JSON). Absent locus -> Null.
                                let args = match &bytes {
                                    Some(b) => serde_json::from_slice::<serde_json::Value>(b)
                                        .unwrap_or_else(|_| {
                                            serde_json::Value::String(
                                                String::from_utf8_lossy(b).into_owned(),
                                            )
                                        }),
                                    None => serde_json::Value::Null,
                                };
                                let result =
                                    registry.invoke(name, &args).map_err(|e| {
                                        RuntimeError::Internal {
                                            message: alloc::format!(
                                                "check {} native fn {name} failed: {e}",
                                                check_id.0
                                            ),
                                        }
                                    })?;
                                let ok = result.as_bool().unwrap_or(false);
                                let why = if ok {
                                    "native predicate passed".into()
                                } else {
                                    alloc::format!(
                                        "native predicate {name} returned {result}"
                                    )
                                };
                                (ok, why)
                            }
                            other => {
                                crate::interpreter::check_eval::eval_predicate(
                                    other,
                                    bytes.as_deref(),
                                )
                            }
                        };

                        // Emit the verdict event.
                        let verdict = if passed { "pass" } else { "fail" };
                        tracing::info!(
                            parent: &step_span,
                            name = EV_CHECK_EVALUATED,
                            id = check_id.0.as_str(),
                            kind = "goal",
                            verdict = verdict,
                            attempt = 1u32,
                            rationale = rationale.as_str(),
                        );

                        if passed {
                            serde_json::Value::Bool(true)
                        } else {
                            // C1: abort on failure (retry lands in C2).
                            return Ok(PipelineOutcome {
                                outputs: store,
                                token_usage: total_usage,
                                status: PipelineStatus::CheckAborted {
                                    check: check_id.0.clone(),
                                    rationale,
                                },
                            });
                        }
                    }
                    tau_ir::check::CheckVerify::Deliverable { .. } => {
                        // Deliverable judge + retry land in Task C2.
                        return Err(RuntimeError::Internal {
                            message: alloc::format!(
                                "deliverable check {} evaluation lands in C2",
                                check_id.0
                            ),
                        });
                    }
                }
            }
        };

        store.insert(step.id.0.clone(), output);
        tracing::info!(parent: &step_span, name = EV_PIPELINE_STEP_COMPLETED, id = step.id.0.as_str());
    }

    Ok(PipelineOutcome { outputs: store, token_usage: total_usage, status: PipelineStatus::Completed })
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
fn user_message(content: &str) -> Message {
    Message::new(
        Address::User,
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text {
            content: content.to_string(),
        },
    )
}
