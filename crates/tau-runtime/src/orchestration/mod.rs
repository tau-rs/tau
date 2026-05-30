//! Multi-agent orchestration primitives.
//!
//! Core types live in `tau-runtime-core::orchestration` and are re-exported
//! here. Tokio-shell-specific pieces (persistence, mpsc trace subscriber)
//! remain local.

// ── Re-exports from core ─────────────────────────────────────────────────────
pub use tau_runtime_core::orchestration::budget;
pub use tau_runtime_core::orchestration::error;
pub use tau_runtime_core::orchestration::run_state;
pub use tau_runtime_core::orchestration::skill_resolve;
pub use tau_runtime_core::orchestration::task_list;
pub use tau_runtime_core::orchestration::trace;
pub use tau_runtime_core::orchestration::virtual_tools;

pub use tau_runtime_core::orchestration::resolve_skill_for_spawn;
pub use tau_runtime_core::orchestration::BudgetWatchdog;
pub use tau_runtime_core::orchestration::OrchestrationError;
pub use tau_runtime_core::orchestration::RunState;
pub use tau_runtime_core::orchestration::TaskList;
pub use tau_runtime_core::orchestration::{
    apply_scope_paths, substitute_skill_dir, SkillSpawnArgs, SkillSpawnRequest,
};
pub use tau_runtime_core::orchestration::{
    check_capability_subset, dispatch, is_virtual, required_capability, validate_agent_spawn,
    validate_skill_spawn, AgentSpawnRequest,
};
pub use tau_runtime_core::orchestration::{NoopTraceSubscriber, TraceStream, TraceSubscriber};

// ── Tokio-shell-specific ─────────────────────────────────────────────────────

/// JSONL run-log writer (tokio::fs + tokio::spawn).
/// Stays in tau-runtime — uses tokio::sync::mpsc::UnboundedReceiver, tokio::fs,
/// and tokio::spawn which are not available in embassy/wasm shells.
pub mod persistence;

/// Tokio mpsc implementation of TraceSubscriber.
/// Wraps tokio::sync::mpsc::UnboundedSender<TraceEvent> to implement the
/// core TraceSubscriber trait.
pub mod trace_mpsc;
