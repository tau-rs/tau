//! v0 partial-interpret driver for `tau_ir::IrModule`.
//!
//! Per the design spec D-5, v0 (β.2) carries the IR as data and runs it
//! through this interpreter. The interpreter is a thin layer over the
//! existing `Runtime` agent loop — for each agent node, it builds a
//! `Runtime` configured with the agent's tools (resolved via the
//! caller's tool registry) and dispatches its budget.
//!
//! The same module is what `tau dev` calls (with callbacks-for-tools)
//! and what the bundle's wasm component calls (with WASI- / tau-host-
//! gated tool dispatch). The interpreter does not distinguish; the
//! difference lives in the `ToolDispatcher` implementation the caller
//! supplies.

pub mod agent_loop;
pub mod deterministic;
pub mod subflow;
pub mod tool_dispatch;

use alloc::sync::Arc;
use alloc::vec::Vec;

use tau_domain::Message;
use tau_ir::{AgentId, IrModule};

use crate::error::RuntimeError;
use crate::outcome::RunOutcome;

/// Drive an `IrModule` from its single entry agent to completion.
///
/// `entry` names which agent in the module to start with. Future v0.x
/// will infer it from a `[workflow]` block; v0.0 requires the caller
/// to supply it.
pub async fn run_ir<D>(
    module: &IrModule,
    entry: &AgentId,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: tool_dispatch::ToolDispatcher + Send + Sync + 'static,
{
    let agent_node = module
        .workflow
        .agents
        .get(entry)
        .ok_or_else(|| RuntimeError::AgentNotFound {
            agent: entry.0.clone(),
        })?;
    agent_loop::run_agent(module, agent_node, dispatcher, initial_messages).await
}
