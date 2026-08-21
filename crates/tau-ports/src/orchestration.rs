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

use alloc::collections::BTreeMap;
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

/// Governance decision recorded for a capability-gated tool call.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "verdict", rename_all = "snake_case"))]
pub enum CapabilityVerdict {
    /// Call allowed as requested.
    Allow,
    /// Call allowed after meet-clamping to a narrower authority.
    Clamp {
        /// Human-readable clamped target (e.g. the allowed host).
        to: String,
    },
    /// Call denied fail-closed.
    Drop {
        /// Why it was dropped.
        reason: String,
    },
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
        /// Total tokens (input + output) consumed by this turn.
        tokens: u64,
    },
    /// An agent called a tool.
    ToolCall {
        /// Tool name.
        tool_name: String,
        /// Duration in ms.
        duration_ms: u64,
        /// Status (`"ok"`, `"error"`).
        status: String,
        /// Capability decision, if this tool was capability-gated.
        /// `None` for un-gated tools or traces predating this field.
        #[cfg_attr(feature = "serde", serde(default))]
        capability: Option<CapabilityVerdict>,
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

// ---------------------------------------------------------------------------
// Turn-level checkpoint/resume (ADR-0053 — durable execution A-minimal)
// ---------------------------------------------------------------------------

/// A resumable snapshot of an agent run taken at a completed turn boundary.
///
/// The full message history is carried so resume is "feed the history back
/// in" — β.4's context pipeline is stateless and re-derives deterministically
/// from the history, so no separate context-manager state is needed (ADR-0053
/// D4). Token counts are plain `u64` (not [`crate::llm::TokenUsage`], which is
/// `u32`-based) so the runtime's `u64` accounting round-trips without
/// narrowing; the runtime reconstructs its own token type on resume.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TurnCheckpoint {
    /// Run this checkpoint belongs to (the `--resume` key).
    pub run_id: RunId,
    /// The turn number that just completed (1-based). Resume re-enters at
    /// `turn + 1`.
    pub turn: u32,
    /// Full message history as of the end of `turn`.
    pub history: Vec<tau_domain::Message>,
    /// Cumulative input (prompt) tokens through `turn`.
    pub input_tokens: u64,
    /// Cumulative output (completion) tokens through `turn`.
    pub output_tokens: u64,
    /// Tools the model requested in `turn` that had **not** completed when
    /// this snapshot was taken (`PerToolCall` mid-turn checkpoints only).
    /// Empty for a `PerTurn` / turn-boundary checkpoint — serde-skipped so
    /// those snapshots stay byte-identical to the A-minimal wire form.
    /// On resume the runtime re-dispatches exactly these before the next
    /// LLM call; carried explicitly because they are not derivable from
    /// `history` (see ADR-0053 follow-up).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub pending_tool_uses: Vec<crate::llm::ToolUse>,
}

/// Errors surfaced by a [`CheckpointStore`].
///
/// Messages are `String` (not `std::io::Error`) so the type stays
/// no_std-clean; the host `FileCheckpointStore` maps I/O failures into
/// [`CheckpointError::Io`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    /// Reading or writing the durable store failed.
    #[error("checkpoint store I/O failed: {0}")]
    Io(String),
    /// Encoding or decoding a checkpoint failed.
    #[error("checkpoint (de)serialization failed: {0}")]
    Serialization(String),
}

/// Port: persists and loads [`TurnCheckpoint`]s for durable agents (ADR-0053).
///
/// Injected into the runtime via `RunOptions`; the agent loop calls
/// [`CheckpointStore::persist`] after each `TurnCompleted` when the agent is
/// durable, and `tau run --resume` calls [`CheckpointStore::load_latest`]
/// before the loop. Keeping this behind a port (rather than feature-gating
/// file I/O inside the no_std kernel) lets the kill-and-resume test run
/// in-core against an in-memory mock — see `crate::fixtures` under the
/// `test-fixtures` feature.
pub trait CheckpointStore: Send + Sync {
    /// Durably commit a turn boundary. At-least-once: a side effect in the
    /// turn may have already happened, so callers must be idempotent.
    fn persist(&self, ckpt: &TurnCheckpoint) -> Result<(), CheckpointError>;

    /// Load the latest (highest-`turn`) checkpoint for `run_id`, or `None`
    /// if the run has no committed checkpoint.
    fn load_latest(&self, run_id: &RunId) -> Result<Option<TurnCheckpoint>, CheckpointError>;
}

/// A resumable snapshot of a pipeline paused at a top-level `Suspend` step.
///
/// Distinct from [`TurnCheckpoint`] (agent-turn durability): this carries the
/// pipeline `OutputStore` snapshot + the step cursor, not message history. Both
/// share the `run_id` handle and the `.tau/runs/<run_id>/` directory. Resume is
/// restore-and-continue: rehydrate `outputs`, jump to `step_cursor + 1`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PipelineSuspension {
    /// Run this suspension belongs to (the `--resume` key).
    pub run_id: RunId,
    /// Signal the resumer must match to continue (`--signal`).
    pub resume_signal: String,
    /// Index of the `Suspend` step in the top-level pipeline slice. Resume
    /// re-enters at `step_cursor + 1`.
    pub step_cursor: usize,
    /// The `Suspend` step's id (for the "paused at <id>" human message).
    pub step_id: String,
    /// Canonical-IR SHA-256 of the module at pause time (`"sha256:" + hex`).
    /// Resume rejects a project that changed since the pause.
    pub ir_digest: String,
    /// The `OutputStore` snapshot as of the pause (step id -> output value).
    pub outputs: BTreeMap<String, serde_json::Value>,
    /// Per-check retry-attempt counts accumulated up to the pause (check id ->
    /// count). Carried across resume so a `Check` whose `retry.gate` sits
    /// *before* the `Suspend` step cannot reset its attempt budget on every
    /// resume (which would let it rewind→re-suspend forever without
    /// `max_attempts` ever tripping). Absent in pre-followup snapshots, so it
    /// defaults to empty on deserialize.
    #[cfg_attr(feature = "serde", serde(default))]
    pub attempts: BTreeMap<String, u32>,
}

/// Port: persists and loads a pipeline [`PipelineSuspension`] for HITL resume.
///
/// One live suspension per run (a second `Suspend` on resume overwrites it; a
/// completed run removes it). Keyed by the same `RunId` as [`CheckpointStore`]
/// and stored in the same run directory, so one `--resume <run_id>` handle
/// covers both agent-turn and pipeline-step resume.
pub trait SuspensionStore: Send + Sync {
    /// Durably record the pause point. Overwrites any prior suspension for the
    /// same `run_id`.
    fn persist_suspension(&self, s: &PipelineSuspension) -> Result<(), CheckpointError>;

    /// Load the current suspension for `run_id`, or `None` if the run is not
    /// paused.
    fn load_suspension(
        &self,
        run_id: &RunId,
    ) -> Result<Option<PipelineSuspension>, CheckpointError>;

    /// Remove the suspension for `run_id`, if any. Idempotent: absent = `Ok(())`.
    /// Called after a resumed run completes so a stale snapshot cannot be
    /// re-resumed (which would re-run the post-suspend steps). Default no-op for
    /// stores that do not need cleanup.
    fn delete_suspension(&self, run_id: &RunId) -> Result<(), CheckpointError> {
        let _ = run_id;
        Ok(())
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn per_turn_checkpoint_omits_pending_field() {
        let c = TurnCheckpoint {
            run_id: "r".into(),
            turn: 1,
            history: alloc::vec![],
            input_tokens: 0,
            output_tokens: 0,
            pending_tool_uses: alloc::vec![],
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            !json.contains("pending_tool_uses"),
            "empty pending must be skipped for byte-stability; got {json}"
        );
    }

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

    #[cfg(feature = "serde")]
    #[test]
    fn tool_call_serdes_capability_verdict() {
        let evt = TraceEventKind::ToolCall {
            tool_name: "net.http".into(),
            duration_ms: 380,
            status: "ok".into(),
            capability: Some(CapabilityVerdict::Clamp {
                to: "api.example.com".into(),
            }),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"tool_call""#));
        assert!(json.contains(r#""capability""#));
        let back: TraceEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, evt);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn tool_call_capability_absent_deserializes_none() {
        // Forward-compat: an older run without the field parses as None.
        let json = r#"{"kind":"tool_call","tool_name":"fs.read","duration_ms":2,"status":"ok"}"#;
        let back: TraceEventKind = serde_json::from_str(json).unwrap();
        match back {
            TraceEventKind::ToolCall { capability, .. } => assert!(capability.is_none()),
            _ => panic!("wrong variant"),
        }
    }
}
