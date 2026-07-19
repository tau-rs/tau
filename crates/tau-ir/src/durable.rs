//! Per-agent durable-execution configuration (durable execution
//! A-minimal — ADR-0053).
//!
//! `None` on the agent means "not durable" — no checkpoints are written
//! and the run is whole-bundle-reentrant only (Phase B). `Some` opts the
//! agent into turn-level checkpoint/resume: after each completed turn the
//! runtime persists a `TurnCheckpoint` (see
//! `tau_ports::orchestration::TurnCheckpoint`) via the injected
//! `CheckpointStore`, and `tau run --resume <run_id>` re-enters at the
//! next turn.
//!
//! Both enums are `#[non_exhaustive]`: A-minimal ships `PerTurn`, `File`,
//! and `PerToolCall`. `DurableStore::Kv` and the A-full `EventSourced`
//! granularity are additive `MINOR` `ir_format` bumps for later (the
//! same discipline that added `output_schema` as v1.3.0).

use serde::{Deserialize, Serialize};

/// Durable-execution config attached to an [`crate::node::Agent`].
///
/// Either a high-level **intent** (the host picks granularity + store per
/// target — EPIC 6.1) or the **explicit** A-minimal form (ADR-0053).
/// Absent (`None` on the agent) is byte-stable with pre-A-minimal modules.
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Durability {
    /// High-level intent. The host resolves it to a concrete granularity +
    /// store for the run/build target (see `tau_runtime_core::durable_resolve`).
    Intent(DurabilityIntent),
    /// Explicit escape hatch: the author names the mechanism directly.
    Explicit {
        /// How often a checkpoint is committed.
        checkpoint: CheckpointGranularity,
        /// Where checkpoints are written.
        store: DurableStore,
    },
}

impl Durability {
    /// Construct the explicit form from parts. Required because the enum is
    /// `#[non_exhaustive]` — crates outside `tau-ir` cannot use the variant
    /// struct-literal directly.
    pub fn new(checkpoint: CheckpointGranularity, store: DurableStore) -> Self {
        Self::Explicit { checkpoint, store }
    }

    /// The A-minimal default: explicit per-turn checkpoints to the filesystem.
    pub fn per_turn_file() -> Self {
        Self::new(CheckpointGranularity::PerTurn, DurableStore::File)
    }
}

/// High-level durability intent. The host sizes it per target.
///
/// `#[non_exhaustive]`: more intents are additive `MINOR` `ir_format` bumps.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DurabilityIntent {
    /// "This run must survive a process restart." Resolves per target to the
    /// coarsest checkpoint + store the target can durably provide.
    #[serde(rename = "survive-restarts")]
    SurviveRestarts,
}

/// Checkpoint granularity.
///
/// A-minimal ships `PerTurn` (commit after each completed turn,
/// at-least-once for the crashed turn) and `PerToolCall` (narrower
/// at-least-once window within a turn). Finer granularities are additive.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CheckpointGranularity {
    /// Commit a checkpoint after each completed turn.
    #[serde(rename = "per_turn")]
    PerTurn,
    /// Commit a checkpoint after each completed tool call within a turn.
    /// Narrows (does not close) the at-least-once window — exactly-once
    /// stays A-full's job. Resume re-dispatches only the tools that had
    /// not completed before the crash (ADR-0053 follow-up).
    #[serde(rename = "per_tool_call")]
    PerToolCall,
}

/// Where durable state is written.
///
/// A-minimal ships only `File` (`./.tau/runs/<run_id>/turn-<n>.json`).
/// `Kv` (an MCP-contracted journal — never a built-in DB, per NG6) is an
/// additive follow-up belonging to A-full.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DurableStore {
    /// Per-turn snapshot files under `.tau/runs/<run_id>/`.
    #[serde(rename = "file")]
    File,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_round_trips() {
        let d = Durability::per_turn_file();
        let json = serde_json::to_string(&d).expect("serialize");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }

    #[test]
    fn explicit_serializes_tagged_snake_case() {
        let d = Durability::per_turn_file();
        let json = serde_json::to_string(&d).expect("serialize");
        // externally tagged: {"explicit":{"checkpoint":"per_turn","store":"file"}}
        assert!(json.contains("\"explicit\""), "got: {json}");
        assert!(json.contains("per_turn"), "got: {json}");
        assert!(json.contains("\"file\""), "got: {json}");
    }

    #[test]
    fn explicit_per_tool_call_round_trips() {
        let d = Durability::new(CheckpointGranularity::PerToolCall, DurableStore::File);
        let json = serde_json::to_string(&d).expect("serialize");
        assert!(json.contains("per_tool_call"), "got: {json}");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }

    #[test]
    fn intent_round_trips_and_serializes_kebab() {
        let d = Durability::Intent(DurabilityIntent::SurviveRestarts);
        let json = serde_json::to_string(&d).expect("serialize");
        // {"intent":"survive-restarts"}
        assert!(json.contains("\"intent\""), "got: {json}");
        assert!(json.contains("survive-restarts"), "got: {json}");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }
}
