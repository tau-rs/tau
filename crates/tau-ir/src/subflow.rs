//! Subflow edges connecting agents and (eventually) sub-workflows.

use alloc::boxed::Box;
use serde::{Deserialize, Serialize};

use crate::capability::CapabilityRequirements;
use crate::ids::{AgentId, SubflowId};

/// The kind of subflow connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub enum SubflowKind {
    /// Spawn a sibling agent within the same module with a narrowed
    /// capability set. Per the subset-of-parent rule, `cap_subset`
    /// MUST be a subset of the parent agent's grant; the lowering pass
    /// checks this.
    Spawn {
        /// Target agent within this module.
        target_agent: AgentId,
        /// Capability subset granted to the child.
        cap_subset: CapabilityRequirements,
    },
    /// Compose another full workflow as a subroutine. Used for
    /// pipeline composition; v0 reserves the variant but the lowering
    /// pass currently rejects it pending the multi-workflow framing
    /// in a future spec.
    Compose {
        /// The sub-workflow's IR module.
        target_workflow: Box<crate::IrModule>,
    },
}

/// A subflow edge in a workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SubflowEdge {
    /// Identifier of this subflow within the workflow.
    pub id: SubflowId,
    /// What kind of connection.
    pub kind: SubflowKind,
}
