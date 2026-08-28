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
pub mod artifact;
pub(crate) mod attenuate;
pub mod check;
pub mod deterministic;
pub(crate) mod dynamic;
pub mod output_store;
pub mod pipeline;
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
///
/// `module` is taken as `Arc<IrModule>` so the dispatcher's
/// `DispatcherTool` can share it across recursive subflow invocations
/// without copying the IR.
pub async fn run_ir<D>(
    module: alloc::sync::Arc<IrModule>,
    entry: &AgentId,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: tool_dispatch::ToolDispatcher + Send + Sync + 'static,
{
    let agent_node =
        module
            .workflow
            .agents
            .get(entry)
            .ok_or_else(|| RuntimeError::AgentNotFound {
                agent: entry.0.clone(),
            })?;
    // Clone the Agent node out of the Arc so the borrow doesn't escape;
    // it's small (id + prompt + a few Vec<...>) and avoids a self-borrow
    // through run_agent's signature.
    let agent_node = agent_node.clone();
    agent_loop::run_agent(module, &agent_node, dispatcher, initial_messages).await
}

/// Drive an `IrModule` from its single entry agent, returning the
/// uncollapsed [`crate::stream::RunEvent`] stream instead of a collapsed
/// [`RunOutcome`].
///
/// Streaming counterpart of [`run_ir`]: identical entry-agent lookup, but
/// delegates to [`agent_loop::run_agent_streaming`] so the conformance dev
/// profile can observe the per-event run (the stream terminates with exactly
/// one `RunEvent::RunCompleted`). The returned stream is `'static` (see
/// [`agent_loop::run_agent_streaming`]).
pub async fn run_ir_streaming<D>(
    module: alloc::sync::Arc<IrModule>,
    entry: &AgentId,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static, RuntimeError>
where
    D: tool_dispatch::ToolDispatcher + Send + Sync + 'static,
{
    let agent_node =
        module
            .workflow
            .agents
            .get(entry)
            .ok_or_else(|| RuntimeError::AgentNotFound {
                agent: entry.0.clone(),
            })?;
    // Clone the Agent node out of the Arc so the borrow doesn't escape
    // (same rationale as `run_ir`).
    let agent_node = agent_node.clone();
    agent_loop::run_agent_streaming(module, &agent_node, dispatcher, initial_messages).await
}
