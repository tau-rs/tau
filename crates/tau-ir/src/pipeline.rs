//! Sequential pipeline: an ordered list of steps the engine executes
//! top-to-bottom, threading each step's output to later steps via
//! `${steps.<id>.output}` templating.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::check::Condition;
use crate::ids::{AgentId, PipelineStepId, StepId, ToolId};

/// An ordered, engine-sequenced pipeline of steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Pipeline {
    /// Steps, executed top-to-bottom in this order.
    pub steps: Vec<PipelineStep>,
}

/// One step in a [`Pipeline`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PipelineStep {
    /// Handle for this step; its output is addressable as
    /// `steps.<id>.output` by later steps.
    pub id: PipelineStepId,
    /// What this step runs.
    pub run: StepRun,
    /// Input template (`${input}`, `${steps.<id>.output}`).
    pub input: String,
}

/// What a [`PipelineStep`] executes — a reference to an existing node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub enum StepRun {
    /// Run an agent node by id.
    Agent(AgentId),
    /// Invoke a tool node by id.
    Tool(ToolId),
    /// Run a deterministic step node by id.
    Deterministic(StepId),
    /// Evaluate a postcondition check by id.
    ///
    /// The referenced [`CheckId`](crate::ids::CheckId) must exist in
    /// `workflow.checks`; this is enforced by the typecheck stage.
    /// Runtime evaluation is implemented in `tau-runtime-core`'s
    /// pipeline executor (Task 19).
    Check(crate::ids::CheckId),
    /// EPIC 4.1: run `then` if `on` holds, else `otherwise` (may be empty).
    /// Executed by the interpreter in 4.2.
    Branch {
        /// The branch condition.
        on: Condition,
        /// Steps run when `on` holds.
        then: Vec<PipelineStep>,
        /// Steps run when `on` does not hold (may be empty).
        otherwise: Vec<PipelineStep>,
    },
    /// EPIC 4.1: fork `branches`, run concurrently, join. Bounded fork-join
    /// (concurrency capped by the interpreter, 4.2).
    Parallel {
        /// Independent step sequences run in parallel.
        branches: Vec<Vec<PipelineStep>>,
    },
    /// EPIC 4.1: run `body` until `until` holds or `max_iters` is hit
    /// (mandatory bound — no unbounded loops). Reuses `OnFail::Retry` rewind
    /// in the interpreter (4.2).
    Loop {
        /// The loop body.
        body: Vec<PipelineStep>,
        /// Exit condition, checked each iteration.
        until: Condition,
        /// Hard iteration cap (`> 0`, enforced by typecheck).
        max_iters: u64,
    },
    /// EPIC 4.1: human-in-the-loop pause — checkpoint and wait for
    /// `resume_signal`, then seed-and-skip resume (ADR-0053 `per_tool_call`
    /// checkpoint; round-trip in 4.3).
    Suspend {
        /// Signal name that resumes the run.
        resume_signal: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentId, CheckId};

    #[test]
    fn pipeline_serde_round_trips() {
        let p = Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("writer".into()),
                run: StepRun::Agent(AgentId("writer".into())),
                input: "${steps.gather.output}".into(),
            }],
        };
        let bytes = serde_json::to_vec(&p).expect("serializes");
        let back: Pipeline = serde_json::from_slice(&bytes).expect("deserializes");
        assert_eq!(p, back);
    }

    #[test]
    fn check_step_serde_round_trips() {
        let p = Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("check-report".into()),
                run: StepRun::Check(CheckId("c".into())),
                input: "${input}".into(),
            }],
        };
        let bytes = serde_json::to_vec(&p).expect("serializes");
        let back: Pipeline = serde_json::from_slice(&bytes).expect("deserializes");
        assert_eq!(p, back);
    }
}
