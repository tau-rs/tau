//! Multi-agent orchestration primitives (executor-agnostic core).
//!
//! Implements the v1 primitive set defined in
//! `docs/superpowers/specs/2026-05-12-multi-agent-orchestration-design.md`
//! and ratified in `docs/decisions/0023-multi-agent-orchestration.md`.
//!
//! - [`task_list`] — `TaskList` state with atomic CAS lock + lease + heartbeat.
//! - [`trace`] — `TraceSubscriber` trait + `TraceStream` fan-out.
//!   The tokio shell's mpsc impl lives in `tau-runtime::orchestration::trace_mpsc`.
//! - [`run_state`] — per-run mutable state (`RunState`).
//! - [`virtual_tools`] — resolver intercepting `task.*`, `run.*`, and
//!   `agent.<kind>.spawn` before plugin dispatch.
//! - [`budget`] — budget breach detection.
//! - [`skill_resolve`] — skill spawn resolution (host-fs gated for fs reads).
//! - [`error`] — typed errors.
//!
//! The kernel entry point is `Runtime::spawn_root_agent` (in `tau-runtime`).
//! `persistence` stays in `tau-runtime` (tokio-shell-specific).

pub mod budget;
pub mod error;
pub mod run_state;
pub mod skill_resolve;
pub mod task_list;
pub mod trace;
pub mod virtual_tools;
// NB: persistence stays in tau-runtime (tokio-shell-specific, uses tokio::fs
// and tokio::sync::mpsc — not portable to embassy/wasm)

pub use budget::BudgetWatchdog;
pub use error::OrchestrationError;
pub use run_state::RunState;
pub use skill_resolve::{
    apply_scope_paths, substitute_skill_dir, SkillSpawnArgs, SkillSpawnRequest,
};
pub use task_list::TaskList;
pub use trace::{NoopTraceSubscriber, TraceStream, TraceSubscriber};
pub use virtual_tools::{
    check_capability_subset, dispatch, is_virtual, required_capability, validate_agent_spawn,
    validate_skill_spawn, AgentSpawnRequest,
};

#[cfg(feature = "host-fs")]
pub use skill_resolve::resolve_skill_for_spawn;
