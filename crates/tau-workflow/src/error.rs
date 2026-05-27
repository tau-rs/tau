//! Typed errors for tau-workflow.

use std::path::PathBuf;

/// Errors raised by parsing, validating, running, or persisting a workflow.
///
/// # Examples
///
/// Construct a `ParseFailed` variant and check its `Display` output:
///
/// ```
/// use tau_workflow::WorkflowError;
/// use std::path::PathBuf;
///
/// let err = WorkflowError::ParseFailed {
///     path: PathBuf::from("workflows/bad.toml"),
///     message: "toml parse: unexpected EOF".into(),
/// };
/// let msg = err.to_string();
/// assert!(msg.contains("bad.toml"), "path should appear in display: {msg}");
/// assert!(msg.contains("unexpected EOF"), "message should appear: {msg}");
/// ```
///
/// Construct a `DriftDetected` variant and check that `Display` names both
/// sets of step ids:
///
/// ```
/// use tau_workflow::WorkflowError;
///
/// let err = WorkflowError::DriftDetected {
///     logged: vec!["step-a".into(), "step-b".into()],
///     current: vec!["step-a".into(), "step-c".into()],
/// };
/// let msg = err.to_string();
/// assert!(msg.contains("step-b"), "logged ids should appear: {msg}");
/// assert!(msg.contains("step-c"), "current ids should appear: {msg}");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkflowError {
    /// Failed to read or parse a workflow TOML file.
    #[error("workflow {path:?}: {message}")]
    ParseFailed {
        /// The workflow file path.
        path: PathBuf,
        /// Human-readable parse detail.
        message: String,
    },

    /// A `${steps.<id>.output}` template referenced a step that does not exist
    /// (or is later in the workflow, which is rejected at parse time).
    #[error("workflow {workflow:?}: step {step_id:?} references unknown step {missing:?}")]
    TemplateUnresolved {
        /// The workflow name.
        workflow: String,
        /// The step that contained the bad template.
        step_id: String,
        /// The missing step identifier.
        missing: String,
    },

    /// A workflow step `agent.run` referenced an agent not declared in tau.toml.
    #[error("workflow {workflow:?}: step {step_id:?} references unknown agent {agent:?}")]
    AgentNotFound {
        /// The workflow name.
        workflow: String,
        /// The step id with the bad reference.
        step_id: String,
        /// The missing agent id.
        agent: String,
    },

    /// A `tool.call` step referenced a tool not declared / not granted by the
    /// workflow's default agent.
    #[error("workflow {workflow:?}: step {step_id:?} references unknown tool {tool:?}")]
    ToolNotFound {
        /// The workflow name.
        workflow: String,
        /// The step id with the bad reference.
        step_id: String,
        /// The missing tool id.
        tool: String,
    },

    /// A step terminated abnormally. The wrapped source is preserved for
    /// `Debug` output; the run aborts and subsequent steps are not executed.
    #[error("workflow step {step_id:?} failed: {source}")]
    StepFailed {
        /// The failing step's id.
        step_id: String,
        /// Underlying runtime error from `tau_runtime`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Persistence I/O failure (disk full, permission denied, etc.).
    /// The partial JSONL is NOT cleaned up — the user can inspect it.
    #[error("workflow persistence failed at {path:?}: {source}")]
    PersistenceError {
        /// The JSONL file path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Resume requested but the JSONL file's recorded steps no longer match
    /// the workflow's current step ids. Use `--force` to override.
    #[error(
        "workflow drift: log step ids {logged:?} differ from current workflow step ids {current:?}"
    )]
    DriftDetected {
        /// Step ids found in the JSONL log.
        logged: Vec<String>,
        /// Step ids present in the current workflow file.
        current: Vec<String>,
    },
}
