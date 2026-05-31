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

    /// Generic parse failure surfacing from the upstream TOML parser.
    #[error("tau.toml parse error: {0}")]
    Parse(String),

    /// SubflowEdge::Compose is not yet implemented (v0 reserves the variant).
    #[error("subflow {subflow:?}: Compose variant is not supported in v0")]
    UnsupportedComposeSubflow {
        /// The offending subflow.
        subflow: SubflowId,
    },
}
