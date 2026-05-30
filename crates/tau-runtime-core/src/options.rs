//! Run-time options for the kernel and the token-usage report.
//!
//! `TokenUsage` is defined here for both `RunOptions` and `RunOutcome`.
//! The full host-shell `RunOptions` (with tokio/orchestration fields) is
//! defined in `tau-runtime`; this module carries the executor-agnostic
//! subset that no_std kernels and host shells both consume.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

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
}
