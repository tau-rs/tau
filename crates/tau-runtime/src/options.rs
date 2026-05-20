//! Run-time options for `Runtime::run` (added in Task 10) and the
//! token-usage report carried in `RunOutcome` (added in Task 6).

/// Token usage reported by the LLM backend, summed across the run.
///
/// Some backends report `total_tokens`; some report only input/output.
/// `Default` returns all zeros (useful when no backend was called yet).
///
/// # Example
///
/// ```
/// use tau_runtime::TokenUsage;
///
/// let usage = TokenUsage::default();
/// assert_eq!(usage.input_tokens, 0);
/// assert_eq!(usage.output_tokens, 0);
/// assert_eq!(usage.total_tokens, None);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Total input (prompt) tokens used across the run.
    pub input_tokens: u64,
    /// Total output (completion) tokens emitted across the run.
    pub output_tokens: u64,
    /// `Some(n)` when the backend reports a unified count; `None` otherwise.
    pub total_tokens: Option<u64>,
}

/// Options for `Runtime::run`.
///
/// Constructed via `RunOptions::default()` (then mutated via public
/// fields if needed). `#[non_exhaustive]` to allow additive options
/// later — Phase-1+ may add `soft_fail_tool_errors`, `llm_retry_policy`,
/// `overall_timeout`, etc.
///
/// # Example
///
/// ```
/// use tau_runtime::RunOptions;
///
/// // `RunOptions` is `#[non_exhaustive]`; default + field mutation
/// // is the only construction pattern.
/// let mut opts = RunOptions::default();
/// opts.max_turns = 8;
/// opts.trace_label = Some("session-abc".into());
/// assert_eq!(opts.max_turns, 8);
/// assert_eq!(opts.trace_label.as_deref(), Some("session-abc"));
/// ```
#[non_exhaustive]
#[derive(Clone)]
pub struct RunOptions {
    /// Hard cap on agent loop iterations. Hitting this returns
    /// `Ok(RunOutcome::Failed { kind: OutOfResources, .. })`.
    /// Default: 16.
    pub max_turns: u32,

    /// Optional caller-supplied label included in tracing spans for
    /// log correlation (e.g. session UUID from a TUI).
    pub trace_label: Option<String>,

    /// Project tau.toml capability override; default empty. Validated
    /// at runtime via `compute_effective` (defense-in-depth — tau-cli
    /// also validates at parse time). When non-empty, narrows the
    /// agent's effective grant from its package manifest.
    pub project_override: Vec<crate::capability_override::CapabilityOverride>,

    /// Set by `Runtime::spawn_root_agent` when running inside a
    /// multi-agent orchestrated run. When present, virtual tool calls
    /// (`task.*`, `run.*`, `agent.<kind>.spawn`) are intercepted before
    /// plugin dispatch and routed through `crate::orchestration`.
    /// Callers using single-agent `Runtime::run` should leave this `None`.
    pub orchestration_state:
        Option<std::sync::Arc<tokio::sync::Mutex<crate::orchestration::run_state::RunState>>>,

    /// Set by `Runtime::spawn_root_agent` (v1.1+). Carries the `Arc<Runtime>`
    /// so the in-stream `agent.<kind>.spawn` intercept can recursively
    /// invoke a child run via `run_with_history` without re-resolving
    /// the kernel. None for single-agent runs.
    pub orchestration_runtime: Option<std::sync::Arc<crate::builder::Runtime>>,

    /// Set by the orchestration recursion path (v1.1+). When `Some`,
    /// short-circuits the `compute_effective(manifest + project_override)`
    /// calculation and uses this list directly as the agent's effective
    /// grant. The capability subset law (`check_capability_subset`) is the
    /// authoritative gate before this is set, so the kernel trusts it as a
    /// pre-validated narrowing of the parent's grant.
    pub granted_capabilities_override: Option<Vec<tau_domain::Capability>>,
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("max_turns", &self.max_turns)
            .field("trace_label", &self.trace_label)
            .field("project_override", &self.project_override)
            .field(
                "orchestration_state",
                &self.orchestration_state.as_ref().map(|_| "<RunState>"),
            )
            .field(
                "orchestration_runtime",
                &self.orchestration_runtime.as_ref().map(|_| "<Runtime>"),
            )
            .field(
                "granted_capabilities_override",
                &self.granted_capabilities_override,
            )
            .finish()
    }
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_turns: 16,
            trace_label: None,
            project_override: Vec::new(),
            orchestration_state: None,
            orchestration_runtime: None,
            granted_capabilities_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_options_default_max_turns_is_16() {
        let opts = RunOptions::default();
        assert_eq!(opts.max_turns, 16);
        assert_eq!(opts.trace_label, None);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn run_options_can_override_max_turns_and_trace_label() {
        // RunOptions is #[non_exhaustive]: from outside the crate,
        // struct-literal construction is blocked.  This test intentionally
        // exercises the default() + field-mutation pattern callers must use.
        let mut opts = RunOptions::default();
        opts.max_turns = 100;
        opts.trace_label = Some("session-abc".into());
        assert_eq!(opts.max_turns, 100);
        assert_eq!(opts.trace_label.as_deref(), Some("session-abc"));
    }

    #[test]
    fn run_options_default_project_override_is_empty() {
        let opts = RunOptions::default();
        assert!(opts.project_override.is_empty());
    }

    #[test]
    fn token_usage_default_is_all_zero() {
        let usage = TokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn token_usage_is_copy() {
        let a = TokenUsage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: Some(3),
        };
        let b = a; // requires Copy
        assert_eq!(a, b);
    }
}
