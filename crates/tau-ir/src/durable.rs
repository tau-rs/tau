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
//! Both enums are `#[non_exhaustive]`: A-minimal ships exactly
//! `PerTurn` + `File`. `CheckpointGranularity::PerToolCall`,
//! `DurableStore::Kv`, and the A-full `EventSourced` granularity are
//! additive `MINOR` `ir_format` bumps for later (the same discipline that
//! added `output_schema` as v1.3.0).

use serde::{Deserialize, Serialize};

/// Durable-execution config attached to an [`crate::node::Agent`].
///
/// Carried verbatim in the IR; the runtime reads it to decide whether to
/// emit a checkpoint after each `TurnCompleted`. Absent (`None` on the
/// agent) is byte-stable with pre-A-minimal modules.
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Durability {
    /// How often a checkpoint is committed.
    pub checkpoint: CheckpointGranularity,
    /// Where checkpoints are written.
    pub store: DurableStore,
}

impl Durability {
    /// Construct from explicit parts. Required because the struct is
    /// `#[non_exhaustive]` — crates outside `tau-ir` (e.g. `tau-ir-lower`)
    /// cannot use struct-literal syntax.
    pub fn new(checkpoint: CheckpointGranularity, store: DurableStore) -> Self {
        Self { checkpoint, store }
    }

    /// The A-minimal default: per-turn checkpoints to the filesystem.
    pub fn per_turn_file() -> Self {
        Self::new(CheckpointGranularity::PerTurn, DurableStore::File)
    }
}

/// Checkpoint granularity.
///
/// A-minimal ships only `PerTurn` (commit after each completed turn,
/// at-least-once for the crashed turn). Finer granularities are additive.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckpointGranularity {
    /// Commit a checkpoint after each completed turn.
    #[serde(rename = "per_turn")]
    PerTurn,
}

/// Where durable state is written.
///
/// A-minimal ships only `File` (`./.tau/runs/<run_id>/turn-<n>.json`).
/// `Kv` (an MCP-contracted journal — never a built-in DB, per NG6) is an
/// additive follow-up belonging to A-full.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum DurableStore {
    /// Per-turn snapshot files under `.tau/runs/<run_id>/`.
    #[serde(rename = "file")]
    File,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durability_round_trips() {
        let d = Durability::per_turn_file();
        let json = serde_json::to_string(&d).expect("serialize");
        let back: Durability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(d, back);
    }

    #[test]
    fn granularity_and_store_serialize_snake_case() {
        let d = Durability::per_turn_file();
        let json = serde_json::to_string(&d).expect("serialize");
        assert!(json.contains("per_turn"), "got: {json}");
        assert!(json.contains("file"), "got: {json}");
    }
}
