//! Top-level IR container.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use tau_ports::target::TargetTriple;

use crate::capability::CapabilityTable;
use crate::ids::{AgentId, StepId, ToolId};
use crate::node::{Agent, Deterministic, Tool};
use crate::pipeline::Pipeline;
use crate::subflow::SubflowEdge;
use crate::trigger::TriggerBinding;

/// Semver-shaped IR format version (D-6).
///
/// Bumps follow semver rules:
/// - MAJOR for breaking shape changes (removed node type, removed
///   required field, changed lowering contract).
/// - MINOR for additive changes (new optional field, new variant of a
///   `#[non_exhaustive]` enum).
/// - PATCH for spec-only edits with no IR-shape effect.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct IrFormatVersion(pub String);

impl IrFormatVersion {
    /// Current IR format version emitted by this `tau-ir` crate.
    pub const CURRENT: &'static str = "v1.3.0";

    /// Construct the version this crate emits.
    pub fn current() -> Self {
        Self(Self::CURRENT.into())
    }
}

/// The container for one workflow's IR.
///
/// `tau build` emits one `IrModule` per workflow (one per project for
/// v0). `tau verify --bundle` re-builds and asserts byte-equality of
/// the canonical form (D-6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IrModule {
    /// IR language version (D-6 — separate from `tau_version`).
    pub ir_format: IrFormatVersion,
    /// tau compiler binary version that emitted this module.
    /// Semver-shaped (e.g. `"0.X.Y"`).
    pub tau_version: String,
    /// Target triple this module was lowered for.
    pub target: TargetTriple,
    /// The workflow itself.
    pub workflow: Workflow,
    /// Trigger bindings — invocation metadata, a SIBLING of `workflow`
    /// (triggers are about *how* tau is invoked, not the call graph).
    /// `skip_serializing_if` + `default` means a trigger-less module emits
    /// no `triggers` key and hashes identically to a pre-trigger module
    /// (Option B / ADR-0044 §D1); older modules with no key read back as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerBinding>,
}

/// The set of nodes + edges that make up one workflow.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Workflow {
    /// Agent nodes by id.
    pub agents: BTreeMap<AgentId, Agent>,
    /// Tool nodes by id.
    pub tools: BTreeMap<ToolId, Tool>,
    /// Deterministic step nodes by id.
    pub steps: BTreeMap<StepId, Deterministic>,
    /// Subflow edges.
    pub edges: Vec<SubflowEdge>,
    /// Per-tool capability requirements. Derived from `tools` but
    /// stored explicitly for the bundle's `tau.caps` custom section.
    pub capability_table: CapabilityTable,
    /// Optional engine-sequenced pipeline. `None` preserves single-entry
    /// behavior (run the named entry agent). `Some` => `run_pipeline`.
    pub pipeline: Option<Pipeline>,
    /// Postcondition checks, keyed by id. Positioned in the pipeline via
    /// `StepRun::Check`.
    #[serde(
        default,
        skip_serializing_if = "alloc::collections::BTreeMap::is_empty"
    )]
    pub checks: alloc::collections::BTreeMap<crate::ids::CheckId, crate::check::Check>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ir_format_version_is_v1_3_0() {
        assert_eq!(IrFormatVersion::CURRENT, "v1.3.0");
        assert_eq!(IrFormatVersion::current().0, "v1.3.0");
    }

    #[test]
    fn default_workflow_has_empty_checks_and_serializes_without_checks_key() {
        let wf = Workflow::default();
        assert!(
            wf.checks.is_empty(),
            "default Workflow must have empty checks"
        );

        // Serialize and confirm "checks" key is absent (byte-stability).
        let bytes = serde_json::to_vec(&wf).expect("serializes");
        let obj: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert!(
            obj.get("checks").is_none(),
            "empty checks must not appear in serialized Workflow; got: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
}
