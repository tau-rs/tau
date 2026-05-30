//! Typed errors raised by multi-agent orchestration operations.

use alloc::string::String;
use alloc::vec::Vec;

use tau_ports::{AgentId, TaskId};

/// Errors surfaced by virtual-tool dispatch + TaskList state transitions.
///
/// # Example
///
/// ```
/// use tau_runtime_core::orchestration::error::OrchestrationError;
///
/// let not_found = OrchestrationError::TaskNotFound { task: "99".into() };
/// assert!(not_found.to_string().contains("99"));
/// assert!(not_found.to_string().contains("not found"));
///
/// let budget_err = OrchestrationError::BudgetExceeded {
///     budget: "max_total_tokens".into(),
///     value: 150,
///     limit: 100,
/// };
/// assert!(budget_err.to_string().contains("max_total_tokens"));
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OrchestrationError {
    /// `agent.<kind>.spawn`: the parent agent's grant does not include
    /// the requested child kind in its `Agent::Spawn { allowed_kinds }`.
    #[error("agent {parent:?} not authorized to spawn kind {kind:?}")]
    SpawnNotAuthorized {
        /// Parent agent id.
        parent: AgentId,
        /// Requested child kind.
        kind: String,
    },

    /// Generic capability check failure for a virtual tool call.
    #[error("agent {agent:?} lacks capability {needed}")]
    CapabilityMissing {
        /// The agent.
        agent: AgentId,
        /// Description of the missing capability.
        needed: String,
    },

    /// `task.claim`: task is currently owned by another agent and its
    /// lease has not expired.
    #[error("task {task:?} already locked by {by:?} until {until}")]
    TaskLocked {
        /// Task id.
        task: TaskId,
        /// Current lock holder.
        by: AgentId,
        /// Lease expiry (RFC 3339).
        until: String,
    },

    /// `task.update` / `task.complete` / `task.heartbeat`: the caller is
    /// not the current owner.
    #[error("task {task:?}: agent {agent:?} is not the owner")]
    NotTaskOwner {
        /// Task id.
        task: TaskId,
        /// Calling agent.
        agent: AgentId,
    },

    /// `task.get` / `task.update`: id doesn't exist.
    #[error("task {task:?} not found")]
    TaskNotFound {
        /// Task id.
        task: TaskId,
    },

    /// Invalid state transition (e.g. claim a task that's already Done).
    #[error("task {task:?}: cannot transition to {target}")]
    InvalidTaskTransition {
        /// Task id.
        task: TaskId,
        /// Target status.
        target: String,
    },

    /// Budget exceeded; aborting.
    #[error("budget {budget} exceeded: {value} / {limit}")]
    BudgetExceeded {
        /// Budget name.
        budget: String,
        /// Final value.
        value: u64,
        /// Limit.
        limit: u64,
    },

    /// Capability subset law violated at agent.spawn time: child grant
    /// contains capabilities not present in the parent's grant.
    #[error("child grant exceeds parent grant: extra = {extras:?}")]
    GrantNotSubset {
        /// Capabilities in child but not in parent (JSON-serialized form).
        extras: Vec<String>,
    },

    /// Persistence I/O failed.
    #[error("orchestration persistence error: {0}")]
    PersistenceError(#[from] std::io::Error),

    /// Argument parsing failure (malformed virtual-tool args).
    #[error("virtual tool {tool}: {detail}")]
    ArgError {
        /// Tool name.
        tool: String,
        /// Detail.
        detail: String,
    },

    /// Skills-4: `skill.<name>.spawn` — no installed skill matches `name`.
    #[error("skill not installed: {name:?}")]
    SkillNotInstalled {
        /// The unresolved skill name.
        name: String,
    },

    /// Skills-4: skill's lockfile entry exists but install path is
    /// missing on disk.
    #[error("skill {name:?} install path missing at {expected_path:?}")]
    SkillInstallPathMissing {
        /// Skill name.
        name: String,
        /// The expected install path (the manifest location).
        expected_path: std::path::PathBuf,
    },

    /// Skills-4: SKILL.md couldn't be read or parsed.
    #[error("skill {name:?} content invalid: {detail}")]
    SkillContentInvalid {
        /// Skill name.
        name: String,
        /// Reason (read error, YAML parse failure, missing required field).
        detail: String,
    },

    /// Skills-4: caller's `scope_paths` includes a path not covered
    /// by any declared fs.* path. Typo detection.
    #[error("skill scope_path {path:?} is not covered by any declared fs.* path")]
    SkillScopePathNotCovered {
        /// The offending scope_path entry.
        path: String,
    },

    /// Skills-4: parent's `Capability::Skill(SkillCapability::Spawn)`
    /// doesn't include the requested skill in `allowed_skills`.
    #[error("agent {parent:?} not authorized to spawn skill {name:?}")]
    SkillSpawnNotAuthorized {
        /// Parent agent id.
        parent: AgentId,
        /// The requested skill name.
        name: String,
    },
}
