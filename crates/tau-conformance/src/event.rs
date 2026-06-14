//! The versioned, crate-owned conformance event model. The authoritative
//! comparison contract (ADR-0046). Each variant is sourced from exactly
//! one runtime channel during normalization (see `normalize.rs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bump when the whitelist or any field projection changes; re-bless
/// goldens in the same change. Recorded in `expected_events.json`.
pub const CONFORMANCE_EVENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ConformanceEvent {
    /// tracing `runtime.run_started`. run_id is modulo (not present here).
    RunStarted,
    /// tracing `runtime.context_step_ran`. Token counts compared.
    ContextStepRan {
        step: String,
        tokens_in: u64,
        tokens_out: u64,
    },
    /// tracing `llm.request_built`.
    InferenceCallStarted,
    /// tracing `llm.response_received` (+ folded `llm.token_usage`).
    InferenceCallCompleted {
        stop_reason: String,
        tokens_in: u64,
        tokens_out: u64,
    },
    /// RunEvent::ToolCallStarted. `call` is the canonical first-seen
    /// ordinal (e.g. "tc#0"); the provider id is modulo.
    ToolCallStarted {
        name: String,
        args: Value,
        call: String,
    },
    /// RunEvent::ToolCallCompleted. `result` is the Ok body or a canonical
    /// error marker; `call` matches the paired Started ordinal.
    ToolCallCompleted {
        name: String,
        result: ToolOutcome,
        call: String,
    },
    /// RunEvent::RunCompleted. Outcome discriminant only.
    RunCompleted { outcome: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolOutcome {
    Ok { body: Value },
    Err, // canonical marker; error text is modulo (provider-specific)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serde_round_trips_and_carries_version() {
        let ev = ConformanceEvent::ToolCallStarted {
            name: "read_temp".into(),
            args: serde_json::json!({}),
            call: "tc#0".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: ConformanceEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
        assert!(CONFORMANCE_EVENT_VERSION >= 1);
    }
}
