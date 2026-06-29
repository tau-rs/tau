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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IrFormatVersion(pub String);

impl IrFormatVersion {
    /// Current IR format version emitted by this `tau-ir` crate.
    // Track 1 (per-agent model resolution) is a breaking change (Agent.model →
    // model_ref, JudgeRef variant rename), so its MAJOR bump supersedes Track 2's
    // MINOR v1.3.0 (output_schema, additive). Reconciled to v2.0.0.
    // MINOR v2.1.0: Agent.durable additive optional field (ADR-0053).
    // MINOR v2.2.0: CheckpointGranularity::PerToolCall variant +
    // TurnCheckpoint.pending_tool_uses additive optional field (ADR-0053
    // follow-up). Byte-stable when durable absent / PerTurn.
    // MINOR v2.3.0: Durability gains the `Intent(survive-restarts)` variant
    // (EPIC 6.1) alongside the explicit form. Optional field, additive shape.
    // MINOR v2.4.0: StepRun gains Branch/Parallel/Loop/Suspend (additive; EPIC 4.1).
    pub const CURRENT: &'static str = "v2.4.0";

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    #[test]
    fn ir_module_schema_builds_and_is_object() {
        let v = serde_json::to_value(&schemars::schema_for!(IrModule)).unwrap();
        assert_eq!(v["type"], "object");
        // ir_format is a required top-level property
        let req = v["required"].as_array().expect("required present");
        assert!(req.iter().any(|x| x == "ir_format"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ir_format_version_is_v2_4_0() {
        assert_eq!(IrFormatVersion::CURRENT, "v2.4.0");
        assert_eq!(IrFormatVersion::current().0, "v2.4.0");
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
