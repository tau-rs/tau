//! Postcondition check IR (D2). A `Check` is defined in
//! `workflow.checks` and positioned in the pipeline by
//! [`StepRun::Check`](crate::pipeline::StepRun::Check). Two kinds:
//! `goal` (deterministic predicate) and `deliverable` (existence floor +
//! LLM judge of content).

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AgentId, CheckId, PipelineStepId};

/// A postcondition check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// This check's id.
    pub id: CheckId,
    /// What is verified and how.
    pub verify: CheckVerify,
    /// Failure handling. `None` => abort on failure (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
}

/// What a check asserts and how it is verified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckVerify {
    /// A measurable condition, verified deterministically (no LLM).
    Goal {
        /// The read locus the predicate inspects.
        evaluates: Locus,
        /// The predicate.
        predicate: Predicate,
    },
    /// A produced artifact whose content an LLM judges.
    Deliverable {
        /// Where the artifact lives.
        locus: Locus,
        /// Natural-language acceptance criterion fed to the judge.
        must_satisfy: String,
        /// Who judges.
        judge: JudgeRef,
    },
}

/// What a check inspects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locus {
    /// A filesystem path (read via the engine's trusted `read_artifact`).
    Path(String),
    /// A named pipeline-step output (`steps.<id>.output`).
    Output(PipelineStepId),
}

/// Deterministic predicate menu + native-fn escape hatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    /// The locus exists.
    Exists,
    /// The locus exists and is non-empty.
    NonEmpty,
    /// Equals a literal string.
    Equals(String),
    /// Matches a regular expression.
    Matches(String),
    /// At least `min` matches of the regex.
    MinCount {
        /// The regex whose matches are counted.
        pattern: String,
        /// Required minimum.
        min: u64,
    },
    /// Validates against a JSON schema.
    SchemaValid(Value),
    /// `<crate>::<path>` registered in the `DeterministicRegistry`.
    NativeFn(String),
}

/// Who judges a deliverable's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JudgeRef {
    /// tau's built-in minimalist judge; `model` overrides the default.
    Builtin {
        /// Optional model override (the `judge_model` author field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A user `[agents.*]` judge.
    Agent(AgentId),
}

/// Failure handling for a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retry {
    /// Abort or rewind-and-retry.
    pub on_fail: OnFail,
    /// Maximum attempts (inclusive of the first).
    pub max_attempts: u32,
    /// Rewind point (resolved by lowering; defaults to the producer).
    pub gate: PipelineStepId,
    /// The resolved producer step (the step that writes the locus).
    pub producer: PipelineStepId,
}

/// Failure policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnFail {
    /// Exit non-zero with the rationale (default).
    Abort,
    /// Rewind to `gate` and re-run forward, feeding back the rationale.
    Retry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn check_serde_round_trips() {
        let c = Check {
            id: CheckId("report".to_string()),
            verify: CheckVerify::Deliverable {
                locus: Locus::Path("/workspace/report.md".to_string()),
                must_satisfy: "coherent".to_string(),
                judge: JudgeRef::Builtin { model: None },
            },
            retry: Some(Retry {
                on_fail: OnFail::Retry,
                max_attempts: 3,
                gate: PipelineStepId("writer".to_string()),
                producer: PipelineStepId("writer".to_string()),
            }),
        };
        let b = serde_json::to_vec(&c).expect("serializes");
        assert_eq!(c, serde_json::from_slice::<Check>(&b).expect("deserializes"));
    }
}
