//! Channel normalizer (ADR-0049, supersedes ADR-0048's dual-channel).
//!
//! Maps the single [`tau_runtime_core::stream::RunEvent`] channel into
//! [`ConformanceEvent`]. The β.6 gate observable is now produced entirely
//! by the engine's typed event stream (`run_ir_streaming`), so a no_std
//! wasm guest can emit it across the component boundary with no tracing
//! subscriber.
//!
//! The whitelisted variants map as follows:
//!
//! - `RunStarted`, `ContextStepRan`, `InferenceCallStarted`,
//!   `InferenceCallCompleted` — the β.7.5 typed gate lifecycle variants.
//! - `ToolCallStarted`, `ToolCallCompleted`, `RunCompleted` — tool args /
//!   results / run outcome.
//!
//! Non-whitelisted variants (`TextDelta`, `TurnCompleted`, `FatalError`,
//! plus any future `#[non_exhaustive]` additions) map to `None`.

use std::collections::BTreeMap;

use serde_json::Value;
use tau_ports::tool::{ToolContent, ToolResult};
use tau_runtime_core::outcome::RunOutcome;
use tau_runtime_core::stream::RunEvent;

use crate::event::{ConformanceEvent, ToolOutcome};

/// Per-run normalization state. Canonicalizes provider-specific tool-call
/// ids to first-seen ordinals (`"tc#0"`, `"tc#1"`, …) so the comparison
/// is independent of provider id strings (which are modulo per ADR-0048).
#[derive(Default)]
pub struct NormState {
    tool_ids: BTreeMap<String, String>,
    next_ordinal: usize,
}

impl NormState {
    /// Return the canonical ordinal for a provider tool-call id, assigning
    /// a fresh `"tc#N"` on first sight and reusing it thereafter (so a
    /// `ToolCallStarted`/`ToolCallCompleted` pair shares one ordinal).
    fn ordinal_for(&mut self, id: &str) -> String {
        if let Some(existing) = self.tool_ids.get(id) {
            return existing.clone();
        }
        let ord = format!("tc#{}", self.next_ordinal);
        self.next_ordinal += 1;
        self.tool_ids.insert(id.to_string(), ord.clone());
        ord
    }
}

/// Render a `StopReason` to its Debug variant name (`"ToolUse"`,
/// `"EndTurn"`, …) — the canonical string the frozen [`ConformanceEvent`]
/// and the golden compare against.
fn stop_reason_name(sr: tau_ports::StopReason) -> String {
    format!("{sr:?}")
}

/// Map one [`RunEvent`] into the conformance stream, canonicalizing
/// tool-call ids via [`NormState`]. Returns `None` for non-whitelisted
/// variants (`TextDelta`, `TurnCompleted`, `FatalError`, plus any future
/// `#[non_exhaustive]` additions).
pub fn map_runevent(ev: RunEvent, st: &mut NormState) -> Option<ConformanceEvent> {
    match ev {
        RunEvent::RunStarted => Some(ConformanceEvent::RunStarted),
        RunEvent::ContextStepRan {
            step,
            tokens_in,
            tokens_out,
        } => Some(ConformanceEvent::ContextStepRan {
            step,
            tokens_in,
            tokens_out,
        }),
        RunEvent::InferenceCallStarted => Some(ConformanceEvent::InferenceCallStarted),
        RunEvent::InferenceCallCompleted {
            stop_reason,
            tokens_in,
            tokens_out,
        } => Some(ConformanceEvent::InferenceCallCompleted {
            stop_reason: stop_reason_name(stop_reason),
            tokens_in,
            tokens_out,
        }),
        RunEvent::ToolCallStarted { id, name, args } => Some(ConformanceEvent::ToolCallStarted {
            name,
            args: value_to_json(&args),
            call: st.ordinal_for(&id),
        }),
        RunEvent::ToolCallCompleted { id, name, result } => {
            let outcome = match result {
                Ok(tr) => ToolOutcome::Ok {
                    body: tool_result_to_json(&tr),
                    is_error: tr.is_error,
                },
                Err(_) => ToolOutcome::Err,
            };
            Some(ConformanceEvent::ToolCallCompleted {
                name,
                result: outcome,
                call: st.ordinal_for(&id),
            })
        }
        RunEvent::RunCompleted { outcome } => Some(ConformanceEvent::RunCompleted {
            outcome: run_outcome_discriminant(&outcome),
        }),
        // TextDelta / TurnCompleted / FatalError + future variants.
        _ => None,
    }
}

/// Convert a `tau_domain::Value` to `serde_json::Value` via serde
/// round-trip (both crates share the serde feature).
fn value_to_json(v: &tau_domain::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Project a [`ToolResult`] to a canonical JSON body. Mirrors the MCP
/// bridge's content handling: text blocks join into a string; JSON blocks
/// carry their structured value. A single block collapses to that block's
/// natural JSON; multiple blocks become a JSON array.
fn tool_result_to_json(tr: &ToolResult) -> Value {
    let blocks: Vec<Value> = tr
        .content
        .iter()
        .map(|b| match b {
            ToolContent::Text { text } => Value::String(text.clone()),
            ToolContent::Json { data } => value_to_json(data),
            // ToolContent is #[non_exhaustive]; render unknown blocks as null.
            _ => Value::Null,
        })
        .collect();
    match blocks.len() {
        1 => blocks.into_iter().next().unwrap(),
        _ => Value::Array(blocks),
    }
}

/// Stable discriminant string for a [`RunOutcome`] (the body is modulo;
/// only the success/failure shape is compared). `Completed` → `"Success"`,
/// `Failed` → `"Failure"`.
fn run_outcome_discriminant(o: &RunOutcome) -> String {
    match o {
        RunOutcome::Completed { .. } => "Success".to_string(),
        RunOutcome::Failed { .. } => "Failure".to_string(),
        // #[non_exhaustive]: future outcome variants render as unknown.
        _ => "Unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ConformanceEvent, ToolOutcome};
    use tau_ports::StopReason;

    fn make_tool_started(id: &str, name: &str) -> RunEvent {
        RunEvent::ToolCallStarted {
            id: id.into(),
            name: name.into(),
            args: tau_domain::Value::Null,
        }
    }

    fn make_tool_completed_ok(id: &str, name: &str) -> RunEvent {
        let tr = ToolResult::new(
            vec![ToolContent::Text {
                text: "21.5C".into(),
            }],
            false,
        );
        RunEvent::ToolCallCompleted {
            id: id.into(),
            name: name.into(),
            result: Ok(tr),
        }
    }

    #[test]
    fn run_started_maps() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(RunEvent::RunStarted, &mut st),
            Some(ConformanceEvent::RunStarted)
        );
    }

    #[test]
    fn context_step_ran_maps() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(
                RunEvent::ContextStepRan {
                    step: "trim_old".into(),
                    tokens_in: 40,
                    tokens_out: 30,
                },
                &mut st
            ),
            Some(ConformanceEvent::ContextStepRan {
                step: "trim_old".into(),
                tokens_in: 40,
                tokens_out: 30,
            })
        );
    }

    #[test]
    fn inference_started_maps() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(RunEvent::InferenceCallStarted, &mut st),
            Some(ConformanceEvent::InferenceCallStarted)
        );
    }

    #[test]
    fn inference_completed_maps_stop_reason_to_debug_name() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(
                RunEvent::InferenceCallCompleted {
                    stop_reason: StopReason::ToolUse,
                    tokens_in: 12,
                    tokens_out: 5,
                },
                &mut st
            ),
            Some(ConformanceEvent::InferenceCallCompleted {
                stop_reason: "ToolUse".into(),
                tokens_in: 12,
                tokens_out: 5,
            })
        );
    }

    #[test]
    fn inference_completed_end_turn_maps_to_debug_name() {
        let mut st = NormState::default();
        assert_eq!(
            map_runevent(
                RunEvent::InferenceCallCompleted {
                    stop_reason: StopReason::EndTurn,
                    tokens_in: 0,
                    tokens_out: 0,
                },
                &mut st
            ),
            Some(ConformanceEvent::InferenceCallCompleted {
                stop_reason: "EndTurn".into(),
                tokens_in: 0,
                tokens_out: 0,
            })
        );
    }

    #[test]
    fn tool_call_ids_canonicalize_to_first_seen_ordinals() {
        let mut st = NormState::default();
        let started = map_runevent(make_tool_started("toolu_abc", "read_temp"), &mut st);
        let completed = map_runevent(make_tool_completed_ok("toolu_abc", "read_temp"), &mut st);
        if let (
            Some(ConformanceEvent::ToolCallStarted { call: c1, .. }),
            Some(ConformanceEvent::ToolCallCompleted { call: c2, .. }),
        ) = (&started, &completed)
        {
            assert_eq!(c1, "tc#0");
            assert_eq!(c2, "tc#0");
        } else {
            panic!("expected tool-call pair, got {started:?} {completed:?}");
        }
    }

    #[test]
    fn distinct_tool_ids_get_distinct_ordinals() {
        let mut st = NormState::default();
        let a = map_runevent(make_tool_started("toolu_a", "t"), &mut st);
        let b = map_runevent(make_tool_started("toolu_b", "t"), &mut st);
        let a_again = map_runevent(make_tool_started("toolu_a", "t"), &mut st);
        let call = |e: &Option<ConformanceEvent>| match e {
            Some(ConformanceEvent::ToolCallStarted { call, .. }) => call.clone(),
            _ => panic!("expected ToolCallStarted"),
        };
        assert_eq!(call(&a), "tc#0");
        assert_eq!(call(&b), "tc#1");
        assert_eq!(call(&a_again), "tc#0");
    }

    #[test]
    fn tool_completed_ok_carries_text_body() {
        let mut st = NormState::default();
        let completed = map_runevent(make_tool_completed_ok("id", "read_temp"), &mut st);
        let Some(ConformanceEvent::ToolCallCompleted { result, .. }) = completed else {
            panic!("expected ToolCallCompleted");
        };
        assert_eq!(
            result,
            ToolOutcome::Ok {
                body: serde_json::Value::String("21.5C".into()),
                is_error: false,
            }
        );
    }

    #[test]
    fn tool_completed_err_is_canonical_marker() {
        let mut st = NormState::default();
        let ev = RunEvent::ToolCallCompleted {
            id: "id".into(),
            name: "read_temp".into(),
            result: Err("boom".into()),
        };
        let completed = map_runevent(ev, &mut st);
        let Some(ConformanceEvent::ToolCallCompleted { result, .. }) = completed else {
            panic!("expected ToolCallCompleted");
        };
        assert_eq!(result, ToolOutcome::Err);
    }

    #[test]
    fn non_whitelisted_runevents_are_none() {
        let mut st = NormState::default();
        assert!(map_runevent(RunEvent::TextDelta { delta: "x".into() }, &mut st).is_none());
    }

    #[test]
    fn tool_completed_semantic_error_is_ok_with_is_error_true() {
        let mut st = NormState::default();
        let tr = ToolResult::new(
            vec![ToolContent::Text {
                text: "sensor offline".into(),
            }],
            true,
        );
        let ev = RunEvent::ToolCallCompleted {
            id: "id".into(),
            name: "read_temp".into(),
            result: Ok(tr),
        };
        let completed = map_runevent(ev, &mut st);
        let Some(ConformanceEvent::ToolCallCompleted { result, .. }) = completed else {
            panic!("expected ToolCallCompleted");
        };
        assert!(matches!(result, ToolOutcome::Ok { is_error: true, .. }));
    }
}
