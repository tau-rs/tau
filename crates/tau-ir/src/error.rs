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

    /// An agent's `PromptSource::Asset` reference is not a well-formed content
    /// hash (`"sha256:" + 64 lowercase hex`). Structured — never `Parse`.
    #[error("agent {agent:?}: malformed prompt asset hash {hash:?} (want \"sha256:\" + 64 lowercase hex)")]
    MalformedAssetHash {
        /// The agent whose prompt carries the bad reference.
        agent: AgentId,
        /// The offending hash string.
        hash: String,
    },

    /// Generic parse failure surfacing from the upstream TOML parser.
    #[error("tau.toml parse error: {0}")]
    Parse(String),

    /// SubflowEdge::Compose is not yet implemented (v0 reserves the variant).
    #[error("subflow {subflow:?}: Compose variant is not supported in v0")]
    UnsupportedComposeSubflow {
        /// The offending subflow.
        subflow: SubflowId,
    },

    /// A trigger binding names an entrypoint agent that is not present in
    /// the workflow.
    #[error("trigger {trigger:?} references unknown agent {agent:?}")]
    UnknownTriggerAgent {
        /// The trigger name.
        trigger: String,
        /// The unresolved entrypoint agent id.
        agent: AgentId,
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

    /// A `StepRun::Check` references a check id absent from `workflow.checks`.
    #[error("pipeline step '{step}' runs check '{check}' but no such check is defined")]
    UnknownCheckRef {
        /// The pipeline step id that contains the bad reference.
        step: String,
        /// The check id that was not found in `workflow.checks`.
        check: String,
    },

    /// A context pipeline names a transformer that is neither a known
    /// builtin nor a declared custom node.
    #[error("agent '{agent}': context transformer '{transformer}' is not a known builtin or custom node")]
    UnknownContextTransformer {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The offending transformer name.
        transformer: String,
    },

    /// A context pipeline's last step is not the builtin `fit_budget`.
    #[error("agent '{agent}': the last context step must be `fit_budget` (found '{last}')")]
    ContextFitBudgetNotLast {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The actual last transformer name.
        last: String,
    },

    /// A context pipeline repeats a transformer name.
    #[error("agent '{agent}': duplicate context transformer '{transformer}'")]
    DuplicateContextTransformer {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The repeated transformer name.
        transformer: String,
    },

    /// A context step declares a determinism class string that lowering does
    /// not recognize (D7-B / ADR-0065). Mirror of
    /// `tau_ir_lower::LowerError::UnknownDeterminism` (D11 consolidation debt).
    #[error("agent '{agent}': context transformer '{transformer}' declares unknown determinism '{determinism}' (want one of: pure, llm_backed, stateful)")]
    UnknownDeterminism {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The offending transformer name.
        transformer: String,
        /// The unrecognized determinism string as authored.
        determinism: String,
    },

    /// Lowering encountered a `PromptEntry` variant it does not know how to
    /// lower (D7-B / ADR-0065). Mirror of
    /// `tau_ir_lower::LowerError::UnsupportedPromptKind` (D11 consolidation
    /// debt). A fail-closed guard for a future `#[non_exhaustive]`
    /// `PromptEntry` variant, not a user-authoring error.
    #[error("agent {agent:?}: unsupported prompt kind (this tau cannot lower {detail}); rebuild with a matching tau")]
    UnsupportedPromptKind {
        /// The agent whose prompt could not be lowered.
        agent: AgentId,
        /// Debug rendering of the offending `PromptEntry`.
        detail: String,
    },

    /// A check's `Locus::Output` references an unknown or later pipeline step.
    #[error(
        "check '{check}' evaluates output of '{output}' which is not an earlier pipeline step"
    )]
    UnknownCheckLocus {
        /// The check id whose locus is invalid.
        check: String,
        /// The referenced step id that is missing or comes at/after the check step.
        output: String,
    },

    /// Canonical IR bytes are not valid JSON (structural decode failure).
    /// Carries a stringified `serde_json::Error` (the crate is `no_std`;
    /// the error is not stored by value).
    #[error("canonical IR is not valid JSON: {0}")]
    Decode(String),

    /// The IR-format MAJOR of the loaded bundle does not match the MAJOR
    /// this `tau` emits. A breaking IR-shape change: the bundle must be
    /// rebuilt with a compatible `tau`.
    #[error(
        "IR format major {bundle_major} is incompatible with this tau \
         (emits {current}); rebuild with a matching tau"
    )]
    FormatMajorMismatch {
        /// The bundle's full `ir_format` string, e.g. `v3.0.0`.
        bundle: String,
        /// This tau's `ir_format`, e.g. `v2.4.0`.
        current: String,
        /// The parsed major of `bundle`.
        bundle_major: u64,
    },

    /// The bundle's `ir_format` string is not a valid `vMAJOR.MINOR.PATCH`.
    #[error("IR format version {value:?} is not a valid vMAJOR.MINOR.PATCH string")]
    FormatUnparseable {
        /// The offending `ir_format` value as read from the bundle.
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn format_major_mismatch_renders() {
        let e = IrError::FormatMajorMismatch {
            bundle: "v3.0.0".into(),
            current: "v2.4.0".into(),
            bundle_major: 3,
        };
        let msg = e.to_string();
        assert!(msg.contains("major 3"), "got: {msg}");
        assert!(msg.contains("v2.4.0"), "got: {msg}");
        assert!(msg.contains("rebuild"), "got: {msg}");
    }

    #[test]
    fn format_unparseable_renders() {
        let e = IrError::FormatUnparseable {
            value: "garbage".into(),
        };
        assert!(e.to_string().contains("garbage"));
    }

    #[test]
    fn decode_renders() {
        let e = IrError::Decode("expected value at line 1".into());
        assert!(e.to_string().contains("not valid JSON"));
    }
}
