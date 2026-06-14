//! Channel normalizer (ADR-0046).
//!
//! Maps the two raw runtime channels into [`ConformanceEvent`]:
//!
//! 1. **tracing** events captured by [`tau_observe::CapturedEvent`]. The
//!    VOCABULARY event name lives in `fields["name"]` (the runtime emits
//!    `info!(name = EV_..., field = val, ...)`), NOT in the struct's
//!    `.name` (which the captor sets to the literal message or callsite).
//! 2. [`tau_runtime_core::stream::RunEvent`] — the whitelisted tool-call
//!    and run-completion variants.
//!
//! ## Patch-last token folding
//!
//! `llm.token_usage` is emitted AFTER `llm.response_received` (stream.rs
//! emits response_received then, conditionally, token_usage). So tokens
//! cannot be pre-stashed for a preceding response_received. Instead,
//! [`map_tracing`] pushes an [`InferenceCallCompleted`] with zero tokens
//! on `llm.response_received`, then `llm.token_usage` patches the
//! most-recently-pushed `InferenceCallCompleted` in place.
//!
//! [`InferenceCallCompleted`]: ConformanceEvent::InferenceCallCompleted

use std::collections::BTreeMap;

use serde_json::Value;
use tau_observe::capture::CapturedEvent;
use tau_ports::tool::{ToolContent, ToolResult};
use tau_runtime_core::outcome::RunOutcome;
use tau_runtime_core::stream::RunEvent;

use crate::event::{ConformanceEvent, ToolOutcome};

/// Per-run normalization state. Canonicalizes provider-specific tool-call
/// ids to first-seen ordinals (`"tc#0"`, `"tc#1"`, …) so the comparison
/// is independent of provider id strings (which are modulo per ADR-0046).
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

/// Read a vocabulary field by name (e.g. the `name` field carrying the
/// event vocabulary string, or `step`, `stop_reason`, …).
fn field<'a>(c: &'a CapturedEvent, k: &str) -> Option<&'a str> {
    c.fields.get(k).map(String::as_str)
}

/// Read a numeric field, defaulting to 0 when absent or unparseable.
/// (The captor renders all integers through `to_string`, so these parse
/// cleanly when present.)
fn u64f(c: &CapturedEvent, k: &str) -> u64 {
    field(c, k).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Strip a single pair of surrounding double-quotes, if present. The
/// `stop_reason` field is Debug-formatted; some shapes wrap the value in
/// quotes. We keep the cleaned value deterministic for golden capture.
fn strip_quotes(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
}

/// Map one captured tracing event into the conformance stream, pushing or
/// patching [`out`] in place. Non-whitelisted events are no-ops.
///
/// The vocabulary event name is read from `fields["name"]` (see module
/// docs). Token folding uses the patch-last approach: see module docs.
pub fn map_tracing(c: &CapturedEvent, _st: &mut NormState, out: &mut Vec<ConformanceEvent>) {
    let Some(name) = field(c, "name") else {
        return;
    };
    match name {
        "runtime.run_started" => out.push(ConformanceEvent::RunStarted),
        "runtime.context_step_ran" => out.push(ConformanceEvent::ContextStepRan {
            step: field(c, "step").unwrap_or_default().to_string(),
            tokens_in: u64f(c, "tokens_in"),
            tokens_out: u64f(c, "tokens_out"),
        }),
        "llm.request_built" => out.push(ConformanceEvent::InferenceCallStarted),
        "llm.response_received" => out.push(ConformanceEvent::InferenceCallCompleted {
            stop_reason: strip_quotes(field(c, "stop_reason").unwrap_or_default()).to_string(),
            tokens_in: 0,
            tokens_out: 0,
        }),
        "llm.token_usage" => {
            // Patch the most-recently-pushed InferenceCallCompleted.
            if let Some(ConformanceEvent::InferenceCallCompleted {
                tokens_in,
                tokens_out,
                ..
            }) = out
                .iter_mut()
                .rev()
                .find(|e| matches!(e, ConformanceEvent::InferenceCallCompleted { .. }))
            {
                *tokens_in = u64f(c, "input_tokens");
                *tokens_out = u64f(c, "output_tokens");
            }
        }
        // capability.*, message.added, dispatch.*, runtime.completed,
        // runtime.failed, llm.stop_reason, etc. — not whitelisted.
        _ => {}
    }
}

/// Map one [`RunEvent`] into the conformance stream, canonicalizing
/// tool-call ids via [`NormState`]. Returns `None` for non-whitelisted
/// variants (`TextDelta`, `TurnCompleted`, `FatalError`, plus any future
/// `#[non_exhaustive]` additions).
pub fn map_runevent(ev: RunEvent, st: &mut NormState) -> Option<ConformanceEvent> {
    match ev {
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
    use std::collections::BTreeMap;
    use tau_observe::capture::CapturedEvent;

    fn captured(name: &str, kv: &[(&str, &str)]) -> CapturedEvent {
        let mut fields = BTreeMap::new();
        fields.insert("name".to_string(), name.to_string());
        for (k, v) in kv {
            fields.insert(k.to_string(), v.to_string());
        }
        CapturedEvent {
            target: "tau_runtime_core::stream".into(),
            level: "debug".into(),
            name: "event".into(),
            fields,
        }
    }

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
    fn whitelisted_tracing_events_map() {
        let mut out = Vec::new();
        let mut st = NormState::default();
        map_tracing(&captured("runtime.run_started", &[]), &mut st, &mut out);
        map_tracing(
            &captured(
                "runtime.context_step_ran",
                &[("step", "trim_old"), ("tokens_in", "40"), ("tokens_out", "30")],
            ),
            &mut st,
            &mut out,
        );
        assert_eq!(out[0], ConformanceEvent::RunStarted);
        assert_eq!(
            out[1],
            ConformanceEvent::ContextStepRan {
                step: "trim_old".into(),
                tokens_in: 40,
                tokens_out: 30
            }
        );
    }

    #[test]
    fn token_usage_patches_last_inference_completed() {
        let mut out = Vec::new();
        let mut st = NormState::default();
        map_tracing(
            &captured("llm.response_received", &[("stop_reason", "tool_use")]),
            &mut st,
            &mut out,
        );
        map_tracing(
            &captured(
                "llm.token_usage",
                &[("input_tokens", "12"), ("output_tokens", "5")],
            ),
            &mut st,
            &mut out,
        );
        assert_eq!(
            out.last().unwrap(),
            &ConformanceEvent::InferenceCallCompleted {
                stop_reason: "tool_use".into(),
                tokens_in: 12,
                tokens_out: 5
            }
        );
    }

    #[test]
    fn stop_reason_quotes_are_stripped() {
        let mut out = Vec::new();
        let mut st = NormState::default();
        map_tracing(
            &captured("llm.response_received", &[("stop_reason", "\"EndTurn\"")]),
            &mut st,
            &mut out,
        );
        assert_eq!(
            out.last().unwrap(),
            &ConformanceEvent::InferenceCallCompleted {
                stop_reason: "EndTurn".into(),
                tokens_in: 0,
                tokens_out: 0
            }
        );
    }

    #[test]
    fn non_whitelisted_tracing_events_dropped() {
        let mut out = Vec::new();
        let mut st = NormState::default();
        map_tracing(&captured("capability.allow", &[]), &mut st, &mut out);
        map_tracing(
            &captured("message.added", &[("role", "User")]),
            &mut st,
            &mut out,
        );
        assert!(out.is_empty());
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
    fn token_usage_without_prior_inference_is_noop() {
        let mut out = Vec::new();
        let mut st = NormState::default();
        map_tracing(
            &captured(
                "llm.token_usage",
                &[("input_tokens", "9"), ("output_tokens", "3")],
            ),
            &mut st,
            &mut out,
        );
        assert!(out.is_empty());
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
        assert!(matches!(
            result,
            ToolOutcome::Ok { is_error: true, .. }
        ));
    }
}
