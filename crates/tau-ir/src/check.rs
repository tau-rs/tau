//! Postcondition checks: `goal` (deterministic predicate) and
//! `deliverable` (produced artifact + LLM-judged content).

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AgentId, CheckId, PipelineStepId};
use crate::tool_impl::NativeFnRef;

/// A postcondition evaluated at a point in the pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// Identifier within the workflow.
    pub id: CheckId,
    /// What is verified and how.
    pub verify: CheckVerify,
    /// Failure handling.
    pub retry: RetryPolicy,
}

/// The two postcondition kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckVerify {
    /// Deterministic predicate over a read locus.
    Goal {
        /// Read locus.
        evaluates: Locus,
        /// Predicate to apply.
        predicate: GoalPredicate,
    },
    /// Produced artifact whose content an LLM judge evaluates.
    Deliverable {
        /// Produced locus.
        locus: Locus,
        /// Natural-language acceptance criterion.
        must_satisfy: String,
        /// Who judges the content.
        judge: JudgeRef,
    },
}

/// A read/produce locus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Locus {
    /// Filesystem path.
    Path(String),
    /// Named pipeline-step output (`steps.<id>.output`).
    Output(PipelineStepId),
}

/// Deterministic goal predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GoalPredicate {
    /// Locus resolves.
    Exists,
    /// Resolves and non-empty.
    NonEmpty,
    /// Equals the literal.
    Equals(String),
    /// Matches the regex.
    Matches(String),
    /// At least N items.
    MinCount(u64),
    /// Validates against the JSON schema.
    SchemaValid(Value),
    /// Registered native fn.
    NativeFn(NativeFnRef),
}

/// Who evaluates a deliverable's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JudgeRef {
    /// The canonical judge, on a build-time-resolved model.
    Default {
        /// Resolved model (alias resolved + producer-default applied at lowering).
        model_ref: crate::model_ref::ModelRef,
    },
    /// A user `[agents.*]` used as judge.
    Agent(AgentId),
}

/// Failure handling for a check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Abort vs rewind-and-retry.
    pub on_fail: OnFail,
    /// Maximum check evaluations (>= 1).
    pub max_attempts: u32,
    /// Rewind point — at or before the producer step.
    pub gate: PipelineStepId,
}

/// `on_fail` discriminant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnFail {
    /// Exit non-zero with the rationale.
    Abort,
    /// Rewind to the gate and re-run forward.
    Retry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PipelineStepId;
    use alloc::string::ToString;

    #[test]
    fn goal_check_round_trips_through_serde() {
        let c = Check {
            id: CheckId("g".to_string()),
            verify: CheckVerify::Goal {
                evaluates: Locus::Path("/x".to_string()),
                predicate: GoalPredicate::Matches("^#".to_string()),
            },
            retry: RetryPolicy {
                on_fail: OnFail::Abort,
                max_attempts: 1,
                gate: PipelineStepId("g".to_string()),
            },
        };
        let bytes = serde_json::to_vec(&c).unwrap();
        let back: Check = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(c, back);
    }
}
