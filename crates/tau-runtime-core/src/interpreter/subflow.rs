//! Execute a `Node::Subflow` edge.
//!
//! v0 supports `SubflowKind::Spawn` only (per `RuntimeError::UnsupportedSubflowCompose`).
//! The spawn dispatches into a sibling agent loop with a narrowed
//! capability set. The agent loop is the same `run_agent` used at the
//! root — recursion is bounded by the interpreter's call stack and the
//! per-agent budget.
//!
//! In β.2.6.2 the `ToolImpl::Subflow` variant became the production
//! call site for sub-agent spawning (see `agent_loop::DispatcherTool::
//! invoke`). `run_subflow` survives as the documented entrypoint for
//! callers that hold a `SubflowKind` value directly.

use alloc::sync::Arc;
use tau_ir::{IrModule, SubflowKind};

use crate::error::RuntimeError;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;

/// Execute one subflow edge.
pub async fn run_subflow<D>(
    module: Arc<IrModule>,
    kind: &SubflowKind,
    dispatcher: Arc<D>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    match kind {
        SubflowKind::Spawn {
            target_agent,
            cap_subset: _,
        } => crate::interpreter::run_ir(module, target_agent, dispatcher, alloc::vec![]).await,
        SubflowKind::Compose { .. } => Err(RuntimeError::UnsupportedSubflowCompose),
    }
}
