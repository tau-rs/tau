//! Outcome of a kernel run.
//!
//! Distinguishes successful completion from agent-level failures
//! (which are NOT errors — see [`crate::error::RuntimeError`] docs
//! for the ADR-0006 dichotomy).

use alloc::vec::Vec;

use tau_domain::{AgentStatus, Message};

use crate::options::TokenUsage;

/// Outcome of a `Runtime::run` call (added in Task 10).
///
/// - [`RunOutcome::Completed`]: agent finished successfully and produced
///   a final response message.
/// - [`RunOutcome::Failed`]: agent ran but failed via a typed
///   [`AgentStatus::Failed { kind, detail }`]. Partial conversation
///   preserved in `all_messages` for inspection.
///
/// Kernel-level operational failures (plugin errors, dispatch errors)
/// are reported via `Err(RuntimeError)`, NOT this enum.
///
/// # Example
///
/// ```
/// use tau_runtime_core::outcome::RunOutcome;
///
/// // `RunOutcome` is `#[non_exhaustive]` and is constructed by
/// // `Runtime::run`; from outside the crate the type appears only
/// // as a function-parameter shape. The doctest below verifies the
/// // pattern-matching surface compiles, including the `_ => {}`
/// // catch-all that `#[non_exhaustive]` requires.
/// fn handle(outcome: RunOutcome) {
///     match outcome {
///         RunOutcome::Completed { final_message: _, total_turns, .. } => {
///             let _ = total_turns;
///         }
///         RunOutcome::Failed { status, .. } => {
///             let _ = status;
///         }
///         _ => {}
///     }
/// }
/// # let _ = handle;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// Agent completed and produced a final response.
    Completed {
        /// The final assistant message (LLM response with no tool_uses).
        final_message: Message,
        /// Full conversation history: initial message + every LLM
        /// response + every tool_use / tool_result.
        all_messages: Vec<Message>,
        /// Number of agent loop iterations performed.
        total_turns: u32,
        /// Token usage summed across the run.
        token_usage: TokenUsage,
    },
    /// Agent ran but failed via a typed `FailureKind`. Partial
    /// conversation preserved.
    Failed {
        /// Always `AgentStatus::Failed { kind, detail }`. The `kind`
        /// distinguishes `PolicyDenied` (capability denial),
        /// `OutOfResources` (max turns reached), `BackendError`,
        /// `Crashed`, `InternalError`.
        status: AgentStatus,
        /// Full conversation history up to the failure point.
        all_messages: Vec<Message>,
        /// Number of agent loop iterations performed.
        total_turns: u32,
        /// Token usage summed across the run.
        token_usage: TokenUsage,
    },
}

/// Outcome of a `run_pipeline` call (D1).
///
/// Richer than the bare `OutputStore` Plan 1 returned: it carries the
/// step outputs, aggregated token usage across every step *and retry
/// attempt*, and a terminal status that distinguishes an agent-level
/// failure (ADR-0006) and a check abort from a clean completion. Kernel
/// / dispatch errors remain `Err(RuntimeError)` and never appear here.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// Every pipeline step's output, keyed by pipeline-step id.
    pub outputs: crate::interpreter::output_store::OutputStore,
    /// Token usage summed across all steps and all retry attempts.
    pub token_usage: TokenUsage,
    /// How the pipeline terminated.
    pub status: PipelineStatus,
}

/// Terminal status of a pipeline run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineStatus {
    /// Every step (and any checks) passed.
    Completed,
    /// An agent step failed via a typed `AgentStatus::Failed` (ADR-0006).
    /// This is NOT a kernel error.
    AgentFailed {
        /// The pipeline-step id that failed.
        step: alloc::string::String,
        /// The agent's typed failure status.
        status: tau_domain::AgentStatus,
    },
    /// A check failed and its `on_fail` policy (or exhausted retries)
    /// aborted the run.
    CheckAborted {
        /// The check id that aborted the run.
        check: alloc::string::String,
        /// The final verdict rationale / diagnostic.
        rationale: alloc::string::String,
    },
}

/// Test-fixture constructors for [`PipelineOutcome`] and
/// [`PipelineStatus`]. Gated on `test-fixtures` (and `tool-validation`,
/// which gates the `OutputStore` dependency) so production callers
/// never see these.
#[cfg(all(feature = "tool-validation", any(test, feature = "test-fixtures")))]
pub mod fixtures {
    use alloc::string::ToString;
    use tau_domain::AgentStatus;

    use crate::interpreter::output_store::OutputStore;
    use crate::options::TokenUsage;
    use crate::outcome::{PipelineOutcome, PipelineStatus};

    /// A `PipelineOutcome` with `PipelineStatus::Completed`, one output
    /// entry `(step_id, Value::String(text))`, and the given token usage.
    pub fn completed_outcome(
        step_id: &str,
        text: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> PipelineOutcome {
        let mut outputs = OutputStore::new();
        outputs.insert(step_id, serde_json::Value::String(text.to_string()));
        PipelineOutcome {
            outputs,
            token_usage: TokenUsage { input_tokens, output_tokens, total_tokens: None },
            status: PipelineStatus::Completed,
        }
    }

    /// A `PipelineOutcome` with `PipelineStatus::CheckAborted`.
    pub fn check_aborted_outcome(check: &str, rationale: &str) -> PipelineOutcome {
        PipelineOutcome {
            outputs: OutputStore::new(),
            token_usage: TokenUsage::default(),
            status: PipelineStatus::CheckAborted {
                check: check.to_string(),
                rationale: rationale.to_string(),
            },
        }
    }

    /// A `PipelineOutcome` with `PipelineStatus::AgentFailed`.
    pub fn agent_failed_outcome(step: &str, status: AgentStatus) -> PipelineOutcome {
        PipelineOutcome {
            outputs: OutputStore::new(),
            token_usage: TokenUsage::default(),
            status: PipelineStatus::AgentFailed {
                step: step.to_string(),
                status,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::TokenUsage;
    use tau_domain::fixtures;

    // AgentStatus::Failed is variant-level #[non_exhaustive] (E0639), so
    // struct-literal construction of that variant is blocked from outside
    // tau-domain. We use AgentStatus::Stopped (a unit variant) to test
    // RunOutcome::Failed structure, and note that AgentStatus::Failed
    // itself will be constructed only by tau-runtime internals (Task 10).
    #[test]
    fn run_outcome_failed_carries_status_and_messages() {
        let outcome = RunOutcome::Failed {
            status: AgentStatus::Stopped,
            all_messages: alloc::vec![],
            total_turns: 3,
            token_usage: TokenUsage::default(),
        };
        let RunOutcome::Failed {
            status,
            total_turns,
            ..
        } = outcome.clone()
        else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert_eq!(status, AgentStatus::Stopped);
        assert_eq!(total_turns, 3);
    }

    #[test]
    fn run_outcome_failed_preserves_token_usage() {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: Some(15),
        };
        let outcome = RunOutcome::Failed {
            status: AgentStatus::Stopped,
            all_messages: alloc::vec![],
            total_turns: 16,
            token_usage: usage,
        };
        let RunOutcome::Failed {
            total_turns,
            token_usage,
            ..
        } = outcome.clone()
        else {
            panic!("expected Failed, got {outcome:?}");
        };
        assert_eq!(total_turns, 16);
        assert_eq!(token_usage.input_tokens, 10);
        assert_eq!(token_usage.output_tokens, 5);
        assert_eq!(token_usage.total_tokens, Some(15));
    }

    #[test]
    fn run_outcome_completed_construction() {
        // Uses tau_domain::fixtures::any_message() (behind the
        // "test-fixtures" feature) because Message is #[non_exhaustive]
        // and has no public constructor outside tau-domain.
        let msg = fixtures::any_message();
        let outcome = RunOutcome::Completed {
            final_message: msg.clone(),
            all_messages: alloc::vec![msg.clone()],
            total_turns: 2,
            token_usage: TokenUsage::default(),
        };
        let RunOutcome::Completed {
            total_turns,
            all_messages,
            ..
        } = outcome.clone()
        else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(total_turns, 2);
        assert_eq!(all_messages.len(), 1);
    }

    #[test]
    fn token_usage_add_saturates_and_sums() {
        let mut a = TokenUsage { input_tokens: 10, output_tokens: 5, total_tokens: Some(15) };
        a.add(&TokenUsage { input_tokens: 1, output_tokens: 2, total_tokens: Some(3) });
        assert_eq!(a.input_tokens, 11);
        assert_eq!(a.output_tokens, 7);
        assert_eq!(a.total_tokens, Some(18));
    }
}
