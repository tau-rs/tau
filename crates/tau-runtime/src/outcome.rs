//! Outcome of a `Runtime::run` call.
//!
//! Distinguishes successful completion from agent-level failures
//! (which are NOT errors — see [`crate::RuntimeError`] docs for the
//! ADR-0006 dichotomy).

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
/// ```ignore
/// // `RunOutcome` is `#[non_exhaustive]`; constructed by `Runtime::run`.
/// use tau_runtime::RunOutcome;
///
/// fn handle(outcome: RunOutcome) {
///     match outcome {
///         RunOutcome::Completed { final_message, total_turns, .. } => {
///             println!("done in {total_turns} turns");
///         }
///         RunOutcome::Failed { status, .. } => {
///             eprintln!("agent failed: {status:?}");
///         }
///         _ => {}
///     }
/// }
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

#[cfg(test)]
mod tests {
    use super::*;
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
            all_messages: vec![],
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
            all_messages: vec![],
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
            all_messages: vec![msg.clone()],
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
}
