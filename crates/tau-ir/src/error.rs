//! IR-level errors raised during parsing, lowering, capability-fit
//! checking, canonicalization, and hashing.

use alloc::string::String;
use alloc::vec::Vec;
use tau_domain::CapabilityShape;
use thiserror::Error;

use crate::ids::{AgentId, StepId, SubflowId, ToolId};

/// IR-level error type.
#[derive(Debug, Error)]
pub enum IrError {
    /// Workflow-shape error: an Agent references a Tool that doesn't
    /// exist in the workflow.
    #[error("agent {agent:?} references unknown tool {tool:?}")]
    UnknownToolRef {
        /// Agent that contains the bad reference.
        agent: AgentId,
        /// The unknown tool id.
        tool: ToolId,
    },

    /// Workflow-shape error: a SubflowEdge::Spawn targets an Agent that
    /// doesn't exist.
    #[error("subflow {subflow:?} targets unknown agent {agent:?}")]
    UnknownSubflowTarget {
        /// The subflow.
        subflow: SubflowId,
        /// The unknown target.
        agent: AgentId,
    },

    /// Workflow-shape error: a SubflowEdge::Spawn's `cap_subset` is
    /// not a subset of the parent agent's grant.
    #[error("subflow {subflow:?}: cap_subset is not a subset of parent agent grant")]
    SubflowCapNotSubset {
        /// The offending subflow.
        subflow: SubflowId,
    },

    /// Capability-fit failure (D-3b). One or more required capability
    /// shapes are not supported by the build target.
    #[error("workflow needs unsupported capability shape(s) on target: {missing:?}")]
    CapabilityFitFailed {
        /// The shapes that the target does not support.
        missing: Vec<CapabilityShape>,
        /// Diagnostic: which tools required them.
        tools: Vec<ToolId>,
    },

    /// A Deterministic step references a function name that the lowering
    /// registry doesn't know.
    #[error("deterministic step {step:?} references unknown fn `{fn_name}`")]
    UnknownDeterministicFn {
        /// The step id.
        step: StepId,
        /// The unresolved name.
        fn_name: String,
    },

    /// A `ToolImpl::Native` reference's content hash could not be resolved
    /// (the native tool registry did not know the symbolic name).
    #[error("native tool {tool:?} references unknown fn `{fn_name}`")]
    UnknownNativeTool {
        /// The tool id that contains the unresolved native ref.
        tool: ToolId,
        /// The native fn name that was not resolved.
        fn_name: String,
    },

    /// A `ToolImpl::Subflow` tool targets an agent that is not present in
    /// the workflow.
    #[error("subflow tool {tool:?} targets unknown agent {agent:?}")]
    UnknownSubflowToolTarget {
        /// The tool id whose `Subflow` variant points at a missing agent.
        tool: ToolId,
        /// The unresolved target agent id.
        agent: AgentId,
    },

    /// A `ToolImpl::Step` tool references a step id that is not present in
    /// the workflow's `steps` table.
    #[error("step tool {tool:?} references unknown step {step:?}")]
    UnknownStepToolTarget {
        /// The tool id whose `Step` variant points at a missing step.
        tool: ToolId,
        /// The unresolved step id.
        step: StepId,
    },

    /// Generic parse failure surfacing from the upstream TOML parser.
    #[error("tau.toml parse error: {0}")]
    Parse(String),

    /// MCP-specific build error (per β.3 design doc §5).
    #[error("MCP build: {0}")]
    McpBuild(#[from] crate::lower::McpBuildError),

    /// SubflowEdge::Compose is not yet implemented (v0 reserves the variant).
    #[error("subflow {subflow:?}: Compose variant is not supported in v0")]
    UnsupportedComposeSubflow {
        /// The offending subflow.
        subflow: SubflowId,
    },

    /// A pipeline step's `run` target does not exist in the workflow.
    #[error("pipeline step {step:?}: run target {target} not found")]
    UnknownPipelineRun {
        /// The pipeline step id.
        step: String,
        /// The unresolved `kind:id` target, e.g. `agent:writer`.
        target: String,
    },

    /// Two pipeline steps share an id.
    #[error("pipeline step id {id:?} is declared more than once")]
    DuplicatePipelineStepId {
        /// The duplicated id.
        id: String,
    },

    /// `${steps.x.output}` references a step that runs at or after this one.
    #[error(
        "pipeline step {step:?} references output of {referenced:?}, which is not an earlier step"
    )]
    ForwardOutputRef {
        /// The referencing step.
        step: String,
        /// The referenced (later/self) step id.
        referenced: String,
    },

    /// `${steps.x.output}` references a step id not in the pipeline.
    #[error("pipeline step {step:?} references unknown step output {referenced:?}")]
    UnknownOutputRef {
        /// The referencing step.
        step: String,
        /// The unknown referenced id.
        referenced: String,
    },

    /// A pipeline input template was malformed (unterminated/unrecognized).
    #[error("pipeline step {step:?}: bad input template: {detail}")]
    BadPipelineTemplate {
        /// The step id.
        step: String,
        /// Human-readable template error.
        detail: String,
    },

    /// A `StepRun::Check` references an id absent from `workflow.checks`.
    #[error("pipeline step {step:?} runs check {check:?} but no such check is defined")]
    UnknownCheckRef {
        /// The pipeline step id.
        step: String,
        /// The missing check id.
        check: String,
    },

    /// A check's `Output` locus names a non-earlier pipeline step.
    #[error("check {check:?} evaluates output {output:?}, which is not an earlier pipeline step")]
    UnknownCheckLocus {
        /// The check id.
        check: String,
        /// The referenced (missing or non-earlier) step output.
        output: String,
    },

    /// A check needing a producer has no pipeline step writing its locus.
    #[error("check {check:?} has no producer: no step writes/declares {locus:?}")]
    DeliverableNoProducer {
        /// The check id.
        check: String,
        /// The unresolvable locus (path or step output).
        locus: String,
    },

    /// `retry_from` names a step that is not in the pipeline.
    #[error("check {check:?} retry_from {retry_from:?} is not a pipeline step")]
    UnknownRetryFrom {
        /// The check id.
        check: String,
        /// The unresolved `retry_from` step id.
        retry_from: String,
    },

    /// `retry_from` runs after the producer (Guarantee 1).
    #[error("check {check:?} retry_from {gate:?} runs after producer {producer:?} — the gate must be at or before the producer")]
    GateAfterProducer {
        /// The check id.
        check: String,
        /// The gate step id.
        gate: String,
        /// The producer step id.
        producer: String,
    },

    /// The retry span has no non-deterministic (agent) step (Guarantee 2).
    #[error("check {check:?} on_fail=retry but the retry span ({span}) contains no agent step; retrying cannot change the result")]
    RetrySpanDeterministic {
        /// The check id.
        check: String,
        /// Human-readable span description (e.g. `"gather -> writer"`).
        span: String,
    },

    /// Two retry spans overlap (D7).
    #[error("retry spans of checks {a:?} and {b:?} overlap; v1 requires disjoint retry spans")]
    OverlappingRetrySpans {
        /// The first check id.
        a: String,
        /// The second check id.
        b: String,
    },

    /// A custom judge agent is not defined.
    #[error("deliverable {check:?} sets judge {judge:?} but no [agents.{judge}] is defined")]
    UnknownJudgeAgent {
        /// The deliverable check id.
        check: String,
        /// The unresolved judge agent id.
        judge: String,
    },
}
