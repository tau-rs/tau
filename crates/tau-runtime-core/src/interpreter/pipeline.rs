//! Engine-sequenced pipeline executor.
//!
//! Runs `IrModule.workflow.pipeline` steps in order, threading each
//! step's output through an [`OutputStore`] so `${steps.<id>.output}`
//! resolves. For now only **agent** steps execute; tool and
//! deterministic steps (Task 9) hit an explicit not-yet-supported error
//! arm.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;

use serde_json::Value;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_ir::pipeline::StepRun;
use tau_ir::IrModule;

use crate::error::RuntimeError;
use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::output_store::OutputStore;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;

/// Drive an `IrModule`'s pipeline to completion, returning all step
/// outputs.
///
/// The module's `workflow.pipeline` must be `Some` — callers branch on
/// that before dispatching here (see `tau run`). Each step renders its
/// `input` template against the run `input` and prior outputs, runs the
/// step, and records the step's output keyed by its pipeline-step id so
/// later steps can reference it via `${steps.<id>.output}`.
///
/// Only [`StepRun::Agent`] steps execute today; [`StepRun::Tool`] and
/// [`StepRun::Deterministic`] return a not-yet-supported
/// [`RuntimeError::Internal`] (Task 9).
pub async fn run_pipeline<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
) -> Result<OutputStore, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let pipeline = module.workflow.pipeline.clone().ok_or_else(|| {
        RuntimeError::Internal {
            message: "run_pipeline called on a module without a pipeline".to_string(),
        }
    })?;

    let mut store = OutputStore::new();

    for step in &pipeline.steps {
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
                let outcome =
                    Box::pin(run_agent(module.clone(), &agent, dispatcher.clone(), initial))
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
            other => {
                return Err(RuntimeError::Internal {
                    message: format!(
                        "pipeline run target not yet supported (Task 9): {other:?}"
                    ),
                })
            }
        };

        store.insert(step.id.0.clone(), output);
    }

    Ok(store)
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
