//! Run-time options for `Runtime::run` (added in Task 10) and the
//! token-usage report carried in `RunOutcome` (added in Task 6).
//!
//! `TokenUsage` is defined in `tau-runtime-core` and re-exported here.
//! `RunOptions` is defined here (host-shell level) until builder.rs
//! and run.rs migrate to core (Tasks 3.4/3.5); it adds the tokio/
//! orchestration fields on top of the core-level fields.

pub use tau_runtime_core::options::TokenUsage;

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
    ///
    /// `Arc<RefCell<RunState>>` (not `Arc<tokio::sync::Mutex<…>>`) is the
    /// honest representation: the kernel futures are non-Send by design
    /// (see `tau_runtime_core::builder` BoxFuture alias), so the agent
    /// loop is single-task already. `RefCell` makes that discipline
    /// explicit and works in `no_std` shells.
    pub orchestration_state:
        Option<std::sync::Arc<core::cell::RefCell<crate::orchestration::run_state::RunState>>>,

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

    /// Clock used by the runtime to stamp wall-clock times on trace events,
    /// run snapshots, and ULID/UUID minting. Host shells inject their impl
    /// (TokioClock on tokio, EmbassyClock on embassy). If `None`, the
    /// kernel uses a zero-valued internal default — meaningful only for
    /// tests; production callers must supply one through their shell's
    /// `drive` entry point.
    pub clock: Option<std::sync::Arc<dyn tau_ports::Clock>>,

    /// Random source used by the runtime to mint session IDs (UUID v4),
    /// run IDs (ULID), trace event IDs (ULID), and other entropy consumers
    /// in the kernel. Host shells inject their impl (OsRandom on std hosts,
    /// HwRandom on MCU). If `None`, the kernel uses a deterministic
    /// fixture — meaningful only for tests.
    pub random: Option<std::sync::Arc<dyn tau_ports::RandomSource>>,

    /// Capability resolver used to apply [`Self::project_override`] (or
    /// any future override system) to a package manifest's declared
    /// capabilities. tau-runtime's [`crate::capability_resolver_impl::TauPkgCapabilityResolver`]
    /// is the production impl.
    ///
    /// When `None`, the kernel's run loop falls back to
    /// [`Self::project_override`] computed via `compute_effective`
    /// (the legacy path), or — if `project_override` is empty too — to
    /// the manifest capabilities unchanged.
    pub capability_resolver: Option<std::sync::Arc<dyn tau_ports::CapabilityResolver>>,
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
            .field("clock", &self.clock.as_ref().map(|_| "<Clock>"))
            .field("random", &self.random.as_ref().map(|_| "<RandomSource>"))
            .field(
                "capability_resolver",
                &self
                    .capability_resolver
                    .as_ref()
                    .map(|_| "<CapabilityResolver>"),
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
            clock: None,
            random: None,
            capability_resolver: None,
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
        // TokenUsage is defined in tau-runtime-core; #[non_exhaustive]
        // blocks struct-literal construction from outside that crate.
        // Use Default to get a value, mutate via field access (which
        // non-exhaustive does NOT block), then verify Copy semantics.
        let mut a = TokenUsage::default();
        a.input_tokens = 1;
        a.output_tokens = 2;
        a.total_tokens = Some(3);
        let b = a; // Copy
        assert_eq!(a.input_tokens, b.input_tokens);
        assert_eq!(a.output_tokens, b.output_tokens);
        assert_eq!(a.total_tokens, b.total_tokens);
    }

    #[test]
    fn run_options_clock_and_random_default_to_none() {
        let opts = RunOptions::default();
        assert!(opts.clock.is_none());
        assert!(opts.random.is_none());
    }
}
