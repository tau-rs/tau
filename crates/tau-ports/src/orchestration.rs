//! Entity types shared across the multi-agent orchestration layer.
//!
//! Lives in `tau-ports` so consumers (`tau-cli`, future serve-mode) can
//! import the types without depending on the runtime kernel. Behavior
//! (state transitions, locking, dispatch) lives in
//! `tau-runtime::orchestration`.
//!
//! See `docs/superpowers/specs/2026-05-12-multi-agent-orchestration-design.md`
//! for the design and `docs/decisions/0023-multi-agent-orchestration.md`
//! for the ADR.

use alloc::string::String;
use alloc::vec::Vec;

use chrono::{DateTime, Utc};

/// Hierarchical task id. Examples: `"01"`, `"01.02"`, `"01.02.01"`.
pub type TaskId = String;

/// Agent id (typically a ULID).
pub type AgentId = String;

/// Run id (typically a ULID).
pub type RunId = String;

/// Task lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum TaskStatus {
    /// Created; no agent has claimed ownership yet.
    Pending,
    /// Claimed by an owner; lease is active; no work yet started.
    Claimed,
    /// Owner is actively executing.
    InProgress,
    /// Completed successfully.
    Done,
    /// Failed; owner reported an error.
    Failed,
    /// Explicitly accepted as orphan by the orchestrator (won't fail the run).
    Discarded,
}

/// One audit entry on a task's life.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TaskEvent {
    /// Time the mutation happened.
    pub ts: DateTime<Utc>,
    /// Agent that performed the mutation; `None` for host-initiated
    /// (lease expiry, run termination).
    pub by: Option<AgentId>,
    /// Short kind: `"created"`, `"claimed"`, `"updated"`, `"completed"`,
    /// `"failed"`, `"released"`, `"discarded"`, `"lease_expired"`, `"heartbeat"`.
    pub kind: String,
    /// Optional human-readable detail (status before/after, notes).
    pub detail: Option<String>,
}

/// A unit of intended work.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Task {
    /// Hierarchical id.
    pub id: TaskId,
    /// Human-readable description.
    pub description: String,
    /// Parent task id; `None` for top-level.
    pub parent_task_id: Option<TaskId>,
    /// Agent that created this task.
    pub created_by: AgentId,
    /// Lock holder; `None` = unclaimed.
    pub owner: Option<AgentId>,
    /// Lease expiry; `None` when unclaimed.
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Current status.
    pub status: TaskStatus,
    /// Result text (set on `Done`).
    pub result_summary: Option<String>,
    /// Error text (set on `Failed`).
    pub error: Option<String>,
    /// Append-only audit trail.
    pub events: Vec<TaskEvent>,
}

/// Filter passed to `task.list`.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TaskListFilter {
    /// Filter by status (e.g. `Pending`).
    pub status: Option<TaskStatus>,
    /// Filter by owner agent id.
    pub owner: Option<AgentId>,
    /// Filter by `parent_task_id`.
    pub parent: Option<TaskId>,
    /// If true, include only tasks with `owner == None`.
    pub unclaimed_only: bool,
}

/// One trace event observable by host subscribers.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TraceEvent {
    /// Per-run unique id (typically a ULID).
    pub id: String,
    /// Wall-clock timestamp.
    pub ts: DateTime<Utc>,
    /// Run this event belongs to.
    pub run_id: RunId,
    /// Agent that emitted; `None` for host-emitted events.
    pub agent_id: Option<AgentId>,
    /// Event kind discriminant + payload.
    pub kind: TraceEventKind,
}

/// Discriminated union of trace event kinds.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", rename_all = "snake_case"))]
pub enum TraceEventKind {
    /// A new agent was spawned.
    Spawn {
        /// The new agent's id.
        child_id: AgentId,
        /// The new agent's kind.
        agent_kind: String,
        /// Number of capabilities granted to the child.
        grant_size: usize,
    },
    /// An agent completed one turn.
    Turn {
        /// The agent.
        agent_id: AgentId,
        /// Zero-based turn index within that agent's run.
        turn_index: u32,
        /// Duration of the turn in milliseconds.
        duration_ms: u64,
    },
    /// An agent called a tool.
    ToolCall {
        /// Tool name.
        tool_name: String,
        /// Duration in ms.
        duration_ms: u64,
        /// Status (`"ok"`, `"error"`).
        status: String,
    },
    /// A task was mutated.
    TaskMutation {
        /// The task id.
        task_id: TaskId,
        /// Mutation kind (`"created"`, `"claimed"`, `"completed"`, etc.).
        mutation: String,
    },
    /// An agent appended to the plan/notes.
    PlanNote {
        /// Truncated snippet (≤ 200 chars).
        snippet: String,
    },
    /// Budget approaching threshold (within 10%).
    BudgetWarn {
        /// Which budget.
        budget: String,
        /// Current value.
        current: u64,
        /// Limit value.
        limit: u64,
    },
    /// Budget exceeded; run aborting.
    BudgetExceeded {
        /// Which budget.
        budget: String,
        /// Final value.
        final_value: u64,
        /// Limit value.
        limit: u64,
    },
    /// An agent completed normally.
    Completion {
        /// The agent.
        agent_id: AgentId,
        /// `"completed"` or `"failed"`.
        status: String,
    },
    /// Run aborted by host (budget, watchdog, SIGINT).
    Abort {
        /// Human-readable reason.
        reason: String,
    },
    /// Orphan tasks present at root completion.
    OrphanedTasksAtTermination {
        /// Their ids.
        task_ids: Vec<TaskId>,
    },
}

/// Optional limits per run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RunBudget {
    /// Maximum cumulative tokens across all agents in the run.
    pub max_total_tokens: Option<u64>,
    /// Maximum wall-clock duration of the run, in seconds.
    pub max_total_duration_secs: Option<u64>,
    /// Maximum number of agents that may be spawned across the run.
    pub max_total_agents: Option<u32>,
}

/// Top-level run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum RunStatus {
    /// Currently executing.
    Running,
    /// Root agent completed AND all tasks ∈ {done, failed, discarded}.
    Completed,
    /// Root agent failed OR orphan tasks present at termination.
    Failed,
    /// Aborted by host (budget exceeded, SIGINT, watchdog).
    Aborted,
}

/// Lightweight snapshot of run state. Useful for inspection / persistence.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RunSnapshot {
    /// Run id.
    pub run_id: RunId,
    /// Root agent id.
    pub root_agent_id: AgentId,
    /// All tasks at snapshot time.
    pub task_list: Vec<Task>,
    /// Free-form plan/notes.
    pub plan: String,
    /// Budget (immutable across the run).
    pub budget: RunBudget,
    /// Aggregated token usage so far.
    pub tokens_used: u64,
    /// Wall-clock seconds since run start.
    pub elapsed_secs: u64,
    /// Number of agents spawned so far.
    pub agents_spawned: u32,
    /// Current status.
    pub status: RunStatus,
    /// Started at.
    pub started_at: DateTime<Utc>,
    /// Ended at (if not still running).
    pub ended_at: Option<DateTime<Utc>>,
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn task_status_roundtrips_snake_case() {
        let s = serde_json::to_string(&TaskStatus::InProgress).unwrap();
        assert_eq!(s, "\"in_progress\"");
        let back: TaskStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, TaskStatus::InProgress);
    }

    #[test]
    fn trace_event_kind_tagged_serde() {
        let evt = TraceEventKind::Spawn {
            child_id: "agent_01".into(),
            agent_kind: "researcher".into(),
            grant_size: 3,
        };
        let s = serde_json::to_value(&evt).unwrap();
        assert_eq!(s["kind"], "spawn");
        assert_eq!(s["child_id"], "agent_01");
        let back: TraceEventKind = serde_json::from_value(s).unwrap();
        assert_eq!(back, evt);
    }

    #[test]
    fn run_budget_defaults_all_none() {
        let b = RunBudget::default();
        assert!(b.max_total_tokens.is_none());
        assert!(b.max_total_duration_secs.is_none());
        assert!(b.max_total_agents.is_none());
    }

    #[test]
    fn task_list_filter_default_unclaimed_false() {
        let f = TaskListFilter::default();
        assert!(!f.unclaimed_only);
        assert!(f.status.is_none());
    }

    #[test]
    fn task_serializes_as_object() {
        let t = Task {
            id: "01".into(),
            description: "do thing".into(),
            parent_task_id: None,
            created_by: "agent_a".into(),
            owner: None,
            lease_expires_at: None,
            status: TaskStatus::Pending,
            result_summary: None,
            error: None,
            events: vec![],
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["id"], "01");
        assert_eq!(v["status"], "pending");
        let back: Task = serde_json::from_value(v).unwrap();
        assert_eq!(back, t);
    }
}
