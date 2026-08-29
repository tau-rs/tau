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
    // MINOR v2.5.0: Agent.prompt String -> PromptSource (untagged: Inline | Asset)
    // for the content-addressed asset store (D6-B). `Inline` serializes as a bare
    // string, so pre-2.5 modules parse and re-serialize byte-identically.
    // MINOR v2.6.0: StepRun gains Dynamic (bounded dynamic region; additive; EPIC 4.4).
    // MINOR v2.7.0: Dynamic gains owner + runnable spawn templates (EPIC 4.5). v2.6
    // Dynamic-bearing modules (never executable — interpreter unconditionally errored)
    // fail decode; rebuild.
    pub const CURRENT: &'static str = "v2.7.0";

    /// Major component of `CURRENT`. Kept as a literal (const string
    /// parsing is awkward) and pinned to `CURRENT` by
    /// `current_major_agrees_with_current_string`.
    pub const CURRENT_MAJOR: u64 = 2;

    /// Construct the version this crate emits.
    pub fn current() -> Self {
        Self(Self::CURRENT.into())
    }

    /// Parse the semver MAJOR out of the version string.
    ///
    /// Strips an optional leading `v`, then parses the integer up to the
    /// first `.` (or the end of the string). Returns `Err(())` when that
    /// leading segment is empty or not a `u64`, so `v2.4.0`, `2.4.0`, and a
    /// dotless `v2` all yield the major, while `""`, `v.4`, and `garbage`
    /// are rejected.
    // `Result<u64, ()>` is the pinned Task-2 interface (D8-B brief); the
    // caller only branches on Ok/Err and doesn't need a richer error type
    // here — `FormatUnparseable` carries the diagnostic instead.
    #[allow(clippy::result_unit_err)]
    pub fn major(&self) -> Result<u64, ()> {
        let s = self.0.strip_prefix('v').unwrap_or(&self.0);
        let head = s.split('.').next().ok_or(())?;
        if head.is_empty() {
            return Err(());
        }
        head.parse::<u64>().map_err(|_| ())
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
    /// Version of the `tau-ir-lower` crate that emitted this module (stamped
    /// from its `CARGO_PKG_VERSION` at lowering). Workspace-versioned, so it
    /// equals the `tau` CLI binary version today. Semver-shaped (e.g. `"0.X.Y"`).
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

/// Failure of the Variant B embedding entry-point contract
/// ([`IrModule::entry_agent`]).
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EntryAgentError {
    /// The module contains no agents at all.
    #[error("IR module contains no agents")]
    NoAgents,
    /// More than one agent: the embedder must pick explicitly.
    #[error("IR module contains {} agents ({available:?}); pass an explicit entry AgentId to run_ir", available.len())]
    Ambiguous {
        /// Every agent id in the module, in `BTreeMap` (lexicographic) order.
        available: Vec<AgentId>,
    },
}

impl IrModule {
    /// Variant B embedding entry-point contract (EPIC 7.1): a module is
    /// directly runnable iff it contains exactly one agent — that agent
    /// is the entry point. Multi-agent modules must select explicitly
    /// via `run_ir`'s `entry` parameter. (The wasm guest enforces the
    /// same rule at load.)
    pub fn entry_agent(&self) -> Result<&AgentId, EntryAgentError> {
        let mut keys = self.workflow.agents.keys();
        match (keys.next(), keys.next()) {
            (Some(only), None) => Ok(only),
            (None, _) => Err(EntryAgentError::NoAgents),
            (Some(_), Some(_)) => Err(EntryAgentError::Ambiguous {
                available: self.workflow.agents.keys().cloned().collect(),
            }),
        }
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    #[test]
    fn ir_module_schema_builds_and_is_object() {
        let v = serde_json::to_value(schemars::schema_for!(IrModule)).unwrap();
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
    fn ir_format_version_is_v2_7_0() {
        assert_eq!(IrFormatVersion::CURRENT, "v2.7.0");
        assert_eq!(IrFormatVersion::current().0, "v2.7.0");
    }

    #[test]
    fn major_parses_leading_v_and_bare() {
        assert_eq!(IrFormatVersion("v2.4.0".into()).major(), Ok(2));
        assert_eq!(IrFormatVersion("2.4.0".into()).major(), Ok(2));
        assert_eq!(IrFormatVersion("v10.0.0".into()).major(), Ok(10));
    }

    #[test]
    fn major_rejects_malformed() {
        assert_eq!(IrFormatVersion("".into()).major(), Err(()));
        assert_eq!(IrFormatVersion("garbage".into()).major(), Err(()));
        assert_eq!(IrFormatVersion("v.4".into()).major(), Err(()));
        assert_eq!(IrFormatVersion("v".into()).major(), Err(()));
    }

    #[test]
    fn current_major_agrees_with_current_string() {
        // A future CURRENT bump must not silently desync CURRENT_MAJOR.
        assert_eq!(
            IrFormatVersion::current().major(),
            Ok(IrFormatVersion::CURRENT_MAJOR)
        );
    }

    fn module_with_agents(ids: &[&str]) -> IrModule {
        let target = tau_ports::target::registry::list_available()
            .next()
            .unwrap()
            .triple;
        let mut agents = BTreeMap::new();
        for id in ids {
            agents.insert(
                AgentId((*id).into()),
                Agent {
                    id: AgentId((*id).into()),
                    prompt: crate::prompt::PromptSource::Inline("p".into()),
                    model_ref: crate::model_ref::ModelRef {
                        backend: "anthropic".into(),
                        model_id: "m".into(),
                    },
                    tool_refs: Vec::new(),
                    context: None,
                    budget: crate::budget::AgentBudget {
                        max_turns: None,
                        max_tokens: None,
                    },
                    produces: Vec::new(),
                    output_schema: None,
                    durable: None,
                },
            );
        }
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow {
                agents,
                ..Workflow::default()
            },
            triggers: Vec::new(),
        }
    }

    #[test]
    fn entry_agent_ok_when_exactly_one() {
        let m = module_with_agents(&["main"]);
        assert_eq!(m.entry_agent().unwrap(), &AgentId("main".into()));
    }

    #[test]
    fn entry_agent_err_when_empty() {
        let m = module_with_agents(&[]);
        assert!(matches!(m.entry_agent(), Err(EntryAgentError::NoAgents)));
    }

    #[test]
    fn entry_agent_err_lists_candidates_when_ambiguous() {
        let m = module_with_agents(&["a", "b"]);
        let err = m.entry_agent().unwrap_err();
        let msg = alloc::format!("{err}");
        assert!(msg.contains('a') && msg.contains('b'), "{msg}");
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
