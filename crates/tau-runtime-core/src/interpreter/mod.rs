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
pub mod check;
pub mod deterministic;
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

/// The IR features this interpreter implements. EPIC 4.2 (#399) adds
/// Branch/Parallel/Loop/Suspend. Kept in sync with
/// `tau_ir::feature::backend_features` by a drift-guard test.
pub const SUPPORTED_FEATURES: &[tau_ir::feature::IrFeature] = &[
    tau_ir::feature::IrFeature::Pipeline,
    tau_ir::feature::IrFeature::Checks,
    tau_ir::feature::IrFeature::Subflow,
    tau_ir::feature::IrFeature::McpTools,
    tau_ir::feature::IrFeature::NativeTools,
    tau_ir::feature::IrFeature::DeterministicSteps,
    tau_ir::feature::IrFeature::Triggers,
];

/// Reject, at load (before any stepping), a module that walks an IR
/// feature this interpreter does not implement. Called as the first
/// statement of both [`run_ir`] and [`run_ir_streaming`] so the same gate
/// covers the native CLI and the wasm guest (both funnel through this
/// module).
fn ensure_supported(module: &IrModule) -> Result<(), RuntimeError> {
    let supported: alloc::collections::BTreeSet<_> = SUPPORTED_FEATURES.iter().copied().collect();
    let required = tau_ir::feature::required_features(module);
    let missing: alloc::vec::Vec<_> = required
        .difference(&supported)
        .map(|f| alloc::format!("{f:?}"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::UnsupportedFeature { features: missing })
    }
}

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
    ensure_supported(&module)?;
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
    ensure_supported(&module)?;
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

#[cfg(test)]
mod feature_gate_tests {
    use super::*;
    use tau_ports::target::adapter_family::AdapterFamily;

    /// Drift guard: the interpreter's `SUPPORTED_FEATURES` const must equal
    /// the shared build-time profile for its adapter family. If a future
    /// EPIC adds interpreter support for another `IrFeature` without
    /// updating both tables, this test catches the gap.
    #[test]
    fn supported_features_matches_shared_table() {
        let shared = tau_ir::feature::backend_features(AdapterFamily::Native);
        let ours: alloc::collections::BTreeSet<_> = SUPPORTED_FEATURES.iter().copied().collect();
        assert_eq!(ours, shared);
    }
}
