//! Per-agent context-management configuration.
//!
//! v0 surface: the field exists on [`crate::Agent`] but is `None` for
//! every workflow — β.4 owns the actual pipeline shape. v0 reserves
//! the slot so adding β.4's struct later is a `MINOR` `ir_format`
//! bump (additive optional field), not a `MAJOR` one.

// schemars 0.8 derive generates code using bare `Box`/`String`/`vec!`
// from the std prelude — import it when the feature is active.
#[cfg(feature = "schema")]
#[allow(unused_imports)]
use std::prelude::rust_2021::*;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// β.4 context-manager configuration attached to an [`crate::node::Agent`].
///
/// `None` on the agent means "no context management" (full history every
/// turn — pre-β.4 behavior). An empty `pipeline` serializes to `{}` so a
/// `Some(ContextConfig::default())` is byte-identical to the legacy empty
/// placeholder.
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContextConfig {
    /// Ordered transformers, applied top-to-bottom each turn. The last
    /// step must be the builtin `fit_budget` (typecheck-enforced).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<ContextStep>,
}

/// One node in a context pipeline.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContextStep {
    /// Transformer name. For builtins: `trim_old`, `compact_tool_outputs`,
    /// `fit_budget`. For custom nodes: the user-chosen step name.
    pub transformer: String,
    /// Author-declared determinism class. Gates β.6 conformance and what
    /// `TransformCx` exposes at runtime.
    pub determinism: DeterminismClass,
    /// Whether this is a builtin or a user-supplied custom node.
    #[serde(default)]
    pub kind: ContextNodeKind,
    /// Per-node config (e.g. `keep_last_turns`, `max_bytes`, `max_tokens`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::collections::BTreeMap<alloc::string::String, serde_json::Value>")
    )]
    pub config: BTreeMap<String, Value>,
}

/// Determinism class shared by the IR (this enum) and the runtime trait
/// (`tau_runtime_core::context::ContextTransformer::determinism`).
/// Defined here so both crates use one definition (no drift).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DeterminismClass {
    /// Pure function of (messages, config); v1's three transformers.
    Pure,
    /// Calls an `LlmBackend` (β.4.3); conformance-gated via cassette replay.
    LlmBacked,
    /// Reads/writes a memory store (β.4.4); excluded from the conformance gate.
    Stateful,
}

/// Delivery vehicle for a context node.
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ContextNodeKind {
    /// A tau-provided builtin transformer.
    #[default]
    Builtin,
    /// A user-supplied node resolved at runtime. `source` selects the lane.
    ///
    /// See: [escape-hatches.md#contextnodekind-custom](../../docs/explanation/escape-hatches.md#contextnodekind-custom).
    Custom {
        /// `native` (v1) | `wasm` (later) | `mcp` (later).
        source: String,
        /// Package reference providing the node (e.g. `my-nodes@^0.1`).
        package: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn empty_context_config_serializes_to_empty_object() {
        // Backward-compat: a ContextConfig with no steps must serialize
        // identically to the pre-β.4 empty placeholder ({}), so existing
        // bundles hash unchanged.
        let cfg = ContextConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn pipeline_roundtrips() {
        let cfg = ContextConfig {
            pipeline: alloc::vec![ContextStep {
                transformer: "fit_budget".to_string(),
                determinism: DeterminismClass::Pure,
                kind: ContextNodeKind::Builtin,
                config: Default::default(),
            }],
        };
        let json = serde_json::to_vec(&cfg).unwrap();
        let back: ContextConfig = serde_json::from_slice(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
