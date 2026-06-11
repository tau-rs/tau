//! Wire-contract projection for serve-mode JSON-RPC responses.
//!
//! Single source of truth for how runtime outcome/event types map to the
//! serve protocol's JSON. The conversions destructure the runtime enums by
//! field name, so a field rename upstream is a COMPILE error (finding D2:
//! the shape was previously hand-rebuilt with `json!` scattered across
//! `dispatch_run`, desyncing silently on rename).
//!
//! The serve-protocol *envelopes* (`{"id","kind","data"}` and the
//! `{"final": true, ...}` streaming summary) live in `dispatch_run` — they
//! are serve-layer constructs, not runtime-type projections.

use serde::Serialize;
use serde_json::{json, Value};
use tau_domain::MessagePayload;
use tau_runtime_tokio::{RunEvent, RunOutcome, TokenUsage};

/// Wire projection of a single [`RunEvent`]: the `kind`/`data` payload plus
/// the streaming side-effect state the dispatcher must remember
/// (`stop_reason` from `TurnCompleted`, `token_usage` from `RunCompleted`).
pub(crate) struct WireEvent {
    pub kind: &'static str,
    pub data: Value,
    pub stop_reason: Option<String>,
    pub token_usage: Option<Value>,
}

// ---------------------------------------------------------------------------
// Wire DTOs. These ARE the protocol shape: each `#[serde(rename)]` /
// field name is a deliberate divergence from the runtime type, and the
// conversions below destructure the runtime enums by field name, so a
// rename upstream fails to compile here instead of silently desyncing the
// JSON-RPC wire format (finding D2).
//
// serde_json has no `preserve_order` feature in this workspace, so `Value`
// is a `BTreeMap` and key order is alphabetical regardless of construction
// path — `to_value(dto)` is byte-identical to the equivalent `json!`.
// ---------------------------------------------------------------------------

/// Batch `runtime.run` result body. The `status` tag selects the variant.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum OutcomeDto {
    Completed {
        final_message: String,
        total_turns: u32,
        token_usage: TokenUsageDto,
    },
    Failed {
        agent_status: String,
        total_turns: u32,
        token_usage: TokenUsageDto,
    },
    /// Guard for future `#[non_exhaustive]` `RunOutcome` variants.
    Unknown,
}

/// Runtime-summed token usage (input/output + optional unified total).
#[derive(Serialize)]
struct TokenUsageDto {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: Option<u64>,
}

/// Per-turn token usage (`tau_ports::TokenUsage`: no unified total).
#[derive(Serialize)]
struct PortTokenUsageDto {
    input_tokens: u32,
    output_tokens: u32,
}

/// One content block of a tool result. The `type` tag selects the variant.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ContentDto {
    Text {
        text: String,
    },
    Json {
        data: tau_domain::Value,
    },
    /// Guard for future `#[non_exhaustive]` `ToolContent` variants.
    Unknown,
}

/// `ToolCallCompleted.result`: either a successful result or an error reason.
#[derive(Serialize)]
#[serde(untagged)]
enum ToolResultDto {
    Ok {
        ok: bool, // always true
        content: Vec<ContentDto>,
        is_error: bool,
    },
    Err {
        ok: bool, // always false
        error: String,
    },
}

#[derive(Serialize)]
struct TextDeltaDto {
    text: String,
}

#[derive(Serialize)]
struct ToolCallStartedDto {
    tool: String,
    args: tau_domain::Value,
    call_id: String,
}

#[derive(Serialize)]
struct ToolCallCompletedDto {
    tool: String,
    call_id: String,
    result: ToolResultDto,
}

#[derive(Serialize)]
struct TurnCompletedDto {
    turn: u32,
    stop_reason: String,
    usage: Option<PortTokenUsageDto>,
}

#[derive(Serialize)]
struct FatalErrorDto {
    kind: String,
    detail: String,
    context_json: Option<String>,
    tool_error_variant: Option<String>,
}

/// Serialize a fixed-shape wire DTO to a `Value`. Infallible for these
/// types except for the `@bytes:`-prefix reservation in `tau_domain::Value`
/// (`args` / `ContentDto::Json.data`), which serializes that reserved string
/// form to an error. This mirrors the pre-refactor `json!({"args": args})`
/// path exactly — that macro also unwraps the same serialization, so the
/// panic behavior is preserved, not introduced.
fn to_value<T: Serialize>(dto: &T) -> Value {
    serde_json::to_value(dto).expect("wire DTO serialization is infallible")
}

fn token_usage_dto(usage: &TokenUsage) -> TokenUsageDto {
    TokenUsageDto {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    }
}

fn content_dto(content: &tau_ports::ToolContent) -> ContentDto {
    match content {
        tau_ports::ToolContent::Text { text } => ContentDto::Text { text: text.clone() },
        tau_ports::ToolContent::Json { data } => ContentDto::Json { data: data.clone() },
        // ToolContent is #[non_exhaustive].
        _ => ContentDto::Unknown,
    }
}

/// Project a finished [`RunOutcome`] into the batch `runtime.run` result body.
pub(crate) fn outcome_to_json(outcome: &RunOutcome) -> Value {
    let dto = match outcome {
        RunOutcome::Completed {
            final_message,
            total_turns,
            token_usage,
            ..
        } => {
            let final_message = match &final_message.payload {
                MessagePayload::Text { content } => content.clone(),
                other => format!("{:?}", other),
            };
            OutcomeDto::Completed {
                final_message,
                total_turns: *total_turns,
                token_usage: token_usage_dto(token_usage),
            }
        }
        RunOutcome::Failed {
            status,
            total_turns,
            token_usage,
            ..
        } => OutcomeDto::Failed {
            // `agent_status` is the `AgentStatus` Debug string, preserved
            // byte-for-byte from the pre-refactor code. A *field* rename is a
            // compile error (the destructure above); a *variant* rename of
            // `AgentStatus` would silently change this string — knowingly out
            // of scope for this pure refactor (the golden test pins the
            // current string so a rename at least breaks a test).
            agent_status: format!("{:?}", status),
            total_turns: *total_turns,
            token_usage: token_usage_dto(token_usage),
        },
        // RunOutcome is #[non_exhaustive]; guard for future variants.
        _ => OutcomeDto::Unknown,
    };
    to_value(&dto)
}

/// Project the runtime's summed [`TokenUsage`] (input/output + optional total).
pub(crate) fn token_usage_to_json(usage: &TokenUsage) -> Value {
    to_value(&token_usage_dto(usage))
}

/// Project one [`RunEvent`] into its wire `kind` + `data` and any streaming
/// side-effect updates the dispatcher accumulates for the final summary.
pub(crate) fn event_to_wire(event: &RunEvent) -> WireEvent {
    match event {
        RunEvent::TextDelta { delta } => WireEvent {
            kind: "TextDelta",
            data: to_value(&TextDeltaDto {
                text: delta.clone(),
            }),
            stop_reason: None,
            token_usage: None,
        },
        RunEvent::ToolCallStarted {
            id: call_id,
            name,
            args,
        } => WireEvent {
            kind: "ToolCallStarted",
            data: to_value(&ToolCallStartedDto {
                tool: name.clone(),
                args: args.clone(),
                call_id: call_id.clone(),
            }),
            stop_reason: None,
            token_usage: None,
        },
        RunEvent::ToolCallCompleted {
            id: call_id,
            name,
            result,
        } => {
            let result = match result {
                Ok(tool_result) => ToolResultDto::Ok {
                    ok: true,
                    content: tool_result.content.iter().map(content_dto).collect(),
                    is_error: tool_result.is_error,
                },
                Err(reason) => ToolResultDto::Err {
                    ok: false,
                    error: reason.clone(),
                },
            };
            WireEvent {
                kind: "ToolCallCompleted",
                data: to_value(&ToolCallCompletedDto {
                    tool: name.clone(),
                    call_id: call_id.clone(),
                    result,
                }),
                stop_reason: None,
                token_usage: None,
            }
        }
        RunEvent::TurnCompleted {
            stop_reason: sr,
            usage,
            turn,
        } => {
            // `stop_reason` is the `StopReason` Debug string, preserved
            // byte-for-byte. As with `agent_status`, a *variant* rename of
            // `StopReason` would silently change this string — knowingly out
            // of scope for this pure refactor; the golden test pins it.
            let sr_str = format!("{:?}", sr);
            // TurnCompleted.usage is Option<tau_ports::TokenUsage> which has
            // only input_tokens and output_tokens (no total_tokens field).
            let usage = usage.as_ref().map(|u| PortTokenUsageDto {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
            });
            WireEvent {
                kind: "TurnCompleted",
                data: to_value(&TurnCompletedDto {
                    turn: *turn,
                    stop_reason: sr_str.clone(),
                    usage,
                }),
                stop_reason: Some(sr_str),
                token_usage: None,
            }
        }
        RunEvent::RunCompleted { outcome } => {
            // Extract token_usage from the outcome for the final summary.
            let tu = match outcome {
                RunOutcome::Completed { token_usage, .. } => token_usage_to_json(token_usage),
                RunOutcome::Failed { token_usage, .. } => token_usage_to_json(token_usage),
                _ => Value::Null,
            };
            WireEvent {
                kind: "RunCompleted",
                data: json!({ "token_usage": tu.clone() }),
                stop_reason: None,
                token_usage: Some(tu),
            }
        }
        RunEvent::FatalError {
            kind,
            detail,
            context_json,
            tool_error_variant,
        } => WireEvent {
            kind: "FatalError",
            data: to_value(&FatalErrorDto {
                kind: kind.clone(),
                detail: detail.clone(),
                context_json: context_json.clone(),
                tool_error_variant: tool_error_variant.clone(),
            }),
            stop_reason: None,
            token_usage: None,
        },
        // RunEvent is #[non_exhaustive]; guard for future variants.
        _ => WireEvent {
            kind: "Unknown",
            data: json!({}),
            stop_reason: None,
            token_usage: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::{Address, AgentStatus, Message, MessagePayload};
    use tau_ports::fixtures::make_token_usage;
    use tau_ports::{StopReason, ToolContent, ToolResult};
    use tau_runtime_tokio::{RunEvent, RunOutcome, TokenUsage};

    fn text_message(s: &str) -> Message {
        Message::new(
            Address::User,
            Address::User,
            MessagePayload::Text { content: s.into() },
        )
    }

    /// Build a runtime-core `TokenUsage`. The struct is `#[non_exhaustive]`
    /// with no constructor, so we mutate the `pub` fields after `default()`.
    fn usage(input: u64, output: u64, total: Option<u64>) -> TokenUsage {
        let mut u = TokenUsage::default();
        u.input_tokens = input;
        u.output_tokens = output;
        u.total_tokens = total;
        u
    }

    /// Build a `tau_domain::Value` (the type carried by `args`/`data`)
    /// from a JSON literal via its serde round-trip.
    fn dval(v: Value) -> tau_domain::Value {
        serde_json::from_value(v).expect("valid tau_domain::Value")
    }

    #[test]
    fn outcome_completed_wire_shape() {
        let outcome = RunOutcome::Completed {
            final_message: text_message("hi there"),
            all_messages: vec![],
            total_turns: 2,
            token_usage: usage(5, 10, Some(15)),
        };
        assert_eq!(
            outcome_to_json(&outcome),
            json!({
                "status": "completed",
                "final_message": "hi there",
                "total_turns": 2,
                "token_usage": { "input_tokens": 5, "output_tokens": 10, "total_tokens": 15 },
            })
        );
    }

    #[test]
    fn outcome_failed_wire_shape() {
        let outcome = RunOutcome::Failed {
            status: AgentStatus::Stopped,
            all_messages: vec![],
            total_turns: 1,
            token_usage: TokenUsage::default(),
        };
        assert_eq!(
            outcome_to_json(&outcome),
            json!({
                "status": "failed",
                "agent_status": "Stopped",
                "total_turns": 1,
                "token_usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": null },
            })
        );
    }

    #[test]
    fn event_text_delta_wire_shape() {
        let w = event_to_wire(&RunEvent::TextDelta {
            delta: "frag".into(),
        });
        assert_eq!(w.kind, "TextDelta");
        assert_eq!(w.data, json!({ "text": "frag" }));
        assert!(w.stop_reason.is_none() && w.token_usage.is_none());
    }

    #[test]
    fn event_tool_call_started_wire_shape() {
        let w = event_to_wire(&RunEvent::ToolCallStarted {
            id: "call-1".into(),
            name: "echo".into(),
            args: dval(json!({ "x": 1 })),
        });
        assert_eq!(w.kind, "ToolCallStarted");
        assert_eq!(
            w.data,
            json!({ "tool": "echo", "args": { "x": 1 }, "call_id": "call-1" })
        );
    }

    #[test]
    fn event_tool_call_completed_ok_wire_shape() {
        let w = event_to_wire(&RunEvent::ToolCallCompleted {
            id: "call-1".into(),
            name: "echo".into(),
            result: Ok(ToolResult::new(
                vec![
                    ToolContent::Text {
                        text: "done".into(),
                    },
                    ToolContent::Json {
                        data: dval(json!({ "n": 2 })),
                    },
                ],
                false,
            )),
        });
        assert_eq!(w.kind, "ToolCallCompleted");
        assert_eq!(
            w.data,
            json!({
                "tool": "echo",
                "call_id": "call-1",
                "result": {
                    "ok": true,
                    "content": [
                        { "type": "text", "text": "done" },
                        { "type": "json", "data": { "n": 2 } }
                    ],
                    "is_error": false
                }
            })
        );
    }

    #[test]
    fn event_tool_call_completed_err_wire_shape() {
        let w = event_to_wire(&RunEvent::ToolCallCompleted {
            id: "call-1".into(),
            name: "echo".into(),
            result: Err("bad args".into()),
        });
        assert_eq!(
            w.data,
            json!({
                "tool": "echo",
                "call_id": "call-1",
                "result": { "ok": false, "error": "bad args" }
            })
        );
    }

    #[test]
    fn event_turn_completed_wire_shape() {
        let w = event_to_wire(&RunEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: Some(make_token_usage(3, 7)),
            turn: 1,
        });
        assert_eq!(w.kind, "TurnCompleted");
        assert_eq!(
            w.data,
            json!({ "turn": 1, "stop_reason": "EndTurn", "usage": { "input_tokens": 3, "output_tokens": 7 } })
        );
        assert_eq!(w.stop_reason.as_deref(), Some("EndTurn"));
    }

    #[test]
    fn event_turn_completed_no_usage_wire_shape() {
        let w = event_to_wire(&RunEvent::TurnCompleted {
            stop_reason: StopReason::MaxTokens,
            usage: None,
            turn: 4,
        });
        assert_eq!(
            w.data,
            json!({ "turn": 4, "stop_reason": "MaxTokens", "usage": null })
        );
    }

    #[test]
    fn event_run_completed_wire_shape() {
        let w = event_to_wire(&RunEvent::RunCompleted {
            outcome: RunOutcome::Completed {
                final_message: text_message("x"),
                all_messages: vec![],
                total_turns: 1,
                token_usage: usage(1, 2, Some(3)),
            },
        });
        assert_eq!(w.kind, "RunCompleted");
        let tu = json!({ "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 });
        assert_eq!(w.data, json!({ "token_usage": tu.clone() }));
        assert_eq!(w.token_usage, Some(tu));
    }

    // Guard-arm shapes for future `#[non_exhaustive]` upstream variants.
    // Asserted at the DTO level because a `RunOutcome`/`ToolContent` variant
    // we don't yet know about can't be constructed here.
    #[test]
    fn unknown_guard_arm_wire_shapes() {
        assert_eq!(
            to_value(&OutcomeDto::Unknown),
            json!({ "status": "unknown" })
        );
        assert_eq!(to_value(&ContentDto::Unknown), json!({ "type": "unknown" }));
    }

    #[test]
    fn event_fatal_error_wire_shape() {
        let w = event_to_wire(&RunEvent::FatalError {
            kind: "Tool".into(),
            detail: "boom".into(),
            context_json: Some("{\"tool\":\"echo\"}".into()),
            tool_error_variant: Some("BadArgs".into()),
        });
        assert_eq!(w.kind, "FatalError");
        assert_eq!(
            w.data,
            json!({
                "kind": "Tool",
                "detail": "boom",
                "context_json": "{\"tool\":\"echo\"}",
                "tool_error_variant": "BadArgs"
            })
        );
    }
}
