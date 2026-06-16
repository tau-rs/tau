//! Run-time options for the kernel and the token-usage report.
//!
//! `TokenUsage` is defined here for both `RunOptions` and `RunOutcome`.
//! The full host-shell `RunOptions` (with tokio/orchestration fields) is
//! defined in `tau-runtime`; this module carries the executor-agnostic
//! subset that no_std kernels and host shells both consume.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::RefCell;

/// Token usage reported by the LLM backend, summed across the run.
///
/// Some backends report `total_tokens`; some report only input/output.
/// `Default` returns all zeros (useful when no backend was called yet).
///
/// # Example
///
/// ```
/// use tau_runtime_core::options::TokenUsage;
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

/// Executor-agnostic core options for a kernel run.
///
/// The full host-shell `RunOptions` in `tau-runtime` re-exports this
/// type and extends it with tokio/orchestration fields (not visible
/// here). Constructed via `RunOptions::default()` then mutated.
///
/// # Example
///
/// ```
/// use tau_runtime_core::options::RunOptions;
///
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
    pub clock: Option<Arc<dyn tau_ports::Clock>>,

    /// Random source used by the runtime to mint session IDs (UUID v4),
    /// run IDs (ULID), trace event IDs (ULID), and other entropy consumers
    /// in the kernel. Host shells inject their impl (OsRandom on std hosts,
    /// HwRandom on MCU). If `None`, the kernel uses a deterministic
    /// fixture — meaningful only for tests.
    pub random: Option<Arc<dyn tau_ports::RandomSource>>,

    /// Capability resolver used to apply project-level overrides to a
    /// package manifest's declared capabilities. Host shells supply
    /// their impl — tau-runtime ships `TauPkgCapabilityResolver` over
    /// `tau_pkg::capability_override::compute_effective`; Embassy/wasm
    /// shells either ship a passthrough impl or leave this `None`.
    ///
    /// When `None` AND [`Self::granted_capabilities_override`] is also
    /// `None`, the kernel uses the manifest capabilities unchanged
    /// (no narrowing, no deny entries). This is the right default for
    /// shells with no override system.
    pub capability_resolver: Option<Arc<dyn tau_ports::CapabilityResolver>>,

    /// Skill resolver used to look up installed skills for
    /// `skill.<name>.spawn` virtual-tool dispatch. Host shells supply
    /// their impl — tau-runtime-tokio ships `TauPkgSkillResolver` over
    /// `tau_pkg::find_installed_skill`; guest (wasm/embassy) shells ship
    /// `tau_ports::NoSkillResolver` or leave this `None`.
    ///
    /// When `None`, a `skill.<name>.spawn` call fails gracefully with a
    /// "no skill resolver available" tool error (the kernel never reads
    /// the filesystem itself). Single-agent runs leave this `None`.
    pub skill_resolver: Option<Arc<dyn tau_ports::SkillResolver>>,

    /// Set by `Runtime::spawn_root_agent` when running inside a
    /// multi-agent orchestrated run. When present, virtual tool calls
    /// (`task.*`, `run.*`, `agent.<kind>.spawn`) are intercepted before
    /// plugin dispatch and routed through `crate::orchestration`.
    /// Callers using single-agent `Runtime::run` should leave this `None`.
    ///
    /// `Arc<RefCell<RunState>>` (not async Mutex) is the honest
    /// representation: the kernel futures are non-Send by design
    /// (`builder::BoxFuture` is `Pin<Box<dyn Future>>` without a `Send`
    /// bound), so the agent loop is single-task already.
    pub orchestration_state: Option<Arc<RefCell<crate::orchestration::run_state::RunState>>>,

    /// Set by `Runtime::spawn_root_agent` (v1.1+). Carries the
    /// `Arc<Runtime>` recursion handle so the in-stream
    /// `agent.<kind>.spawn` / `skill.<name>.spawn` intercept can
    /// recursively invoke a child run on the same kernel via
    /// `run_with_history`. `None` for single-agent runs.
    pub orchestration_runtime: Option<Arc<crate::builder::Runtime>>,

    /// Project-scope root on disk — used for skill resolution and JSONL
    /// run-log paths. Set by the host shell's wrapper around
    /// `spawn_root_agent` from its scope discovery.
    ///
    /// `String` (not `PathBuf`) so this field stays usable in `no_std +
    /// alloc` shells with no `std::path`. Tokio host code wraps/unwraps
    /// via `PathBuf::from` / `Path::to_str`.
    pub scope_root: Option<String>,

    /// β.4 per-turn context pipeline. Empty = no context management
    /// (full history every turn — pre-β.4 behavior).
    pub context_pipeline: Vec<Arc<dyn crate::context::ContextTransformer>>,

    /// Token estimator used by `fit_budget`. Defaults to the heuristic.
    pub token_estimator: Arc<dyn crate::context::TokenEstimator>,
}

impl core::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RunOptions")
            .field("max_turns", &self.max_turns)
            .field("trace_label", &self.trace_label)
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
            .field(
                "skill_resolver",
                &self.skill_resolver.as_ref().map(|_| "<SkillResolver>"),
            )
            .field(
                "orchestration_state",
                &self.orchestration_state.as_ref().map(|_| "<RunState>"),
            )
            .field(
                "orchestration_runtime",
                &self.orchestration_runtime.as_ref().map(|_| "<Runtime>"),
            )
            .field("scope_root", &self.scope_root)
            .field(
                "context_pipeline",
                &alloc::format!("<{} transformers>", self.context_pipeline.len()),
            )
            .field("token_estimator", &"<TokenEstimator>")
            .finish()
    }
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_turns: 16,
            trace_label: None,
            granted_capabilities_override: None,
            clock: None,
            random: None,
            capability_resolver: None,
            skill_resolver: None,
            orchestration_state: None,
            orchestration_runtime: None,
            scope_root: None,
            context_pipeline: Vec::new(),
            token_estimator: Arc::new(crate::context::HeuristicEstimator),
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
        let mut opts = RunOptions::default();
        opts.max_turns = 100;
        opts.trace_label = Some("session-abc".into());
        assert_eq!(opts.max_turns, 100);
        assert_eq!(opts.trace_label.as_deref(), Some("session-abc"));
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

    #[test]
    fn run_options_clock_and_random_default_to_none() {
        let opts = RunOptions::default();
        assert!(opts.clock.is_none());
        assert!(opts.random.is_none());
    }

    #[test]
    fn run_options_skill_resolver_defaults_to_none() {
        let opts = RunOptions::default();
        assert!(opts.skill_resolver.is_none());
    }
}
