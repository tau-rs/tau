//! Sequential pipeline: an ordered list of steps the engine executes
//! top-to-bottom, threading each step's output to later steps via
//! `${steps.<id>.output}` templating.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, PipelineStepId, StepId, ToolId};

/// An ordered, engine-sequenced pipeline of steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    /// Steps, executed top-to-bottom in this order.
    pub steps: Vec<PipelineStep>,
}

/// One step in a [`Pipeline`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepRun {
    /// Run an agent node by id.
    Agent(AgentId),
    /// Invoke a tool node by id.
    Tool(ToolId),
    /// Run a deterministic step node by id.
    Deterministic(StepId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentId;

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
}
