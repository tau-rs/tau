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
use crate::outcome::RunOutcome;
use crate::vocabulary::{EV_PIPELINE_STEP_COMPLETED, EV_PIPELINE_STEP_STARTED, SPAN_PIPELINE_STEP};

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
) -> Result<OutputStore, RuntimeError>
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
            // TODO(Task 19): real check evaluation (evaluate_goal / evaluate_deliverable +
            // rewind-to-gate retry loop). This placeholder keeps the workspace compiling
            // while the check evaluation machinery is being built.
            StepRun::Check(_) => {
                return Err(crate::error::RuntimeError::Internal {
                    message: alloc::string::String::from("StepRun::Check not yet wired (Task 19)"),
                });
            }
        };

        store.insert(step.id.0.clone(), output);
        tracing::info!(parent: &step_span, name = EV_PIPELINE_STEP_COMPLETED, id = step.id.0.as_str());
    }

    Ok(store)
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
    Message::new(
        Address::User,
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text {
            content: content.to_string(),
        },
    )
}
