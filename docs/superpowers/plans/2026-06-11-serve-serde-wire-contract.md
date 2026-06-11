# Serve-Mode Serde Wire Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hand-rolled `json!` serialization of `RunOutcome` / `RunEvent` in serve mode with a single typed `#[serde]` DTO layer, so a runtime field rename becomes a compile error instead of a silent wire desync — while keeping the wire JSON byte-identical.

**Architecture:** Introduce `crates/tau-app/src/serve/wire.rs` owning every wire-shape DTO (`#[derive(Serialize)]` with explicit `#[serde(rename)]` for each divergent field name) plus pure conversion functions `outcome_to_json(&RunOutcome) -> Value` and `event_to_wire(&RunEvent) -> WireEvent`. `dispatch_run.rs` calls these instead of building JSON inline. The conversions destructure the runtime enums by field name, so a rename upstream fails to compile. The serve-protocol *envelopes* (`{"id","kind","data"}`, the `{"final": true, ...}` summary) stay as `json!` in `dispatch_run.rs` — they are serve-layer constructs, not runtime-type projections, and are out of scope for D2.

**Tech Stack:** Rust, serde, serde_json (no `preserve_order` → `Value` is a `BTreeMap`, so byte-identity depends only on matching keys+values, not DTO field order).

**Why DTOs, not upstream serde derives:** The wire shape is a curated projection, not the natural serde shape of the types — `final_message` is the text content only (`all_messages` dropped), the event stream uses a custom `kind`/`data` envelope, fields are renamed (`delta`→`text`, `id`→`call_id`, `name`→`tool`), and two enums (`AgentStatus`, `StopReason`) are intentionally `Debug`-formatted into the wire string. Even with serde derives upstream we could not emit this shape directly, so a projection layer is required regardless. DTOs localize it (the brief's preferred approach when upstream serde would be wide).

**Byte-identity invariants to preserve exactly (do NOT "improve"):**
- `agent_status` and `stop_reason` stay `format!("{:?}", x)` — changing them changes the wire contract.
- `total_tokens: Option<u64>` and `usage: Option<...>` serialize `None` as `null` (NO `skip_serializing_if`).
- `RunCompleted` event emits ONLY `{"token_usage": tu}` (NOT the full outcome).
- The non-`serde` guard arms (`_ => "unknown"` / `Unknown`) are preserved for the `#[non_exhaustive]` runtime enums.

---

## File Structure

- **Create** `crates/tau-app/src/serve/wire.rs` — all DTOs, `outcome_to_json`, `event_to_wire`, `WireEvent`, and golden tests.
- **Modify** `crates/tau-app/src/serve/mod.rs` — add `mod wire;`.
- **Modify** `crates/tau-app/src/serve/dispatch_run.rs` — delete inline `outcome_to_json` / `token_usage_to_json`, gut `emit_event`'s match into `wire::event_to_wire`, keep envelopes.

Cargo target dir per CLAUDE.md: `target/main` (main agent). Always `CARGO_INCREMENTAL=0`, `-p tau-app`, `timeout`.

---

### Task 1: Pure extraction — relocate the hand-rolled logic into `wire.rs` UNCHANGED, then lock it with a golden test

This is the characterization step: move the exact current `json!` logic into testable pure functions WITHOUT changing behavior, then assert the wire shape. Test runs against the hand-rolled JSON → GREEN. The serde swap (Task 2) must keep it GREEN.

**Files:**
- Create: `crates/tau-app/src/serve/wire.rs`
- Modify: `crates/tau-app/src/serve/mod.rs` (add `mod wire;`)
- Modify: `crates/tau-app/src/serve/dispatch_run.rs`

- [ ] **Step 1: Create `wire.rs` with pure functions using the CURRENT `json!` bodies (no DTOs yet).**

```rust
//! Wire-contract projection for serve-mode JSON-RPC responses.
//!
//! Owns the single source of truth for how runtime outcome/event types
//! map to the serve protocol's JSON. Conversions destructure the runtime
//! enums by field name, so a field rename upstream is a COMPILE error
//! (D2: previously the shape was hand-rebuilt with `json!`, desyncing
//! silently on rename).

use serde_json::{json, Value};
use tau_domain::MessagePayload;
use tau_runtime_tokio::{RunEvent, RunOutcome, TokenUsage};

/// Wire projection of a single `RunEvent`, plus the streaming side-effect
/// state the dispatcher must remember (`stop_reason` from `TurnCompleted`,
/// `token_usage` from `RunCompleted`).
pub(crate) struct WireEvent {
    pub kind: &'static str,
    pub data: Value,
    pub stop_reason: Option<String>,
    pub token_usage: Option<Value>,
}

/// Project a finished `RunOutcome` (batch `runtime.run` result body).
pub(crate) fn outcome_to_json(outcome: &RunOutcome) -> Value {
    match outcome {
        RunOutcome::Completed {
            final_message,
            total_turns,
            token_usage,
            ..
        } => {
            let final_text = match &final_message.payload {
                MessagePayload::Text { content } => content.clone(),
                other => format!("{:?}", other),
            };
            json!({
                "status": "completed",
                "final_message": final_text,
                "total_turns": total_turns,
                "token_usage": token_usage_to_json(token_usage),
            })
        }
        RunOutcome::Failed {
            status,
            total_turns,
            token_usage,
            ..
        } => json!({
            "status": "failed",
            "agent_status": format!("{:?}", status),
            "total_turns": total_turns,
            "token_usage": token_usage_to_json(token_usage),
        }),
        _ => json!({ "status": "unknown" }),
    }
}

pub(crate) fn token_usage_to_json(usage: &TokenUsage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens,
    })
}

/// Project one `RunEvent` into its wire `kind` + `data` and any
/// streaming side-effect updates.
pub(crate) fn event_to_wire(event: &RunEvent) -> WireEvent {
    match event {
        RunEvent::TextDelta { delta } => WireEvent {
            kind: "TextDelta",
            data: json!({ "text": delta }),
            stop_reason: None,
            token_usage: None,
        },
        RunEvent::ToolCallStarted {
            id: call_id,
            name,
            args,
        } => WireEvent {
            kind: "ToolCallStarted",
            data: json!({ "tool": name, "args": args, "call_id": call_id }),
            stop_reason: None,
            token_usage: None,
        },
        RunEvent::ToolCallCompleted {
            id: call_id,
            name,
            result,
        } => {
            let result_json = match result {
                Ok(tool_result) => {
                    let content: Vec<Value> = tool_result
                        .content
                        .iter()
                        .map(|c| match c {
                            tau_ports::ToolContent::Text { text } => {
                                json!({ "type": "text", "text": text })
                            }
                            tau_ports::ToolContent::Json { data } => {
                                json!({ "type": "json", "data": data })
                            }
                            _ => json!({ "type": "unknown" }),
                        })
                        .collect();
                    json!({ "ok": true, "content": content, "is_error": tool_result.is_error })
                }
                Err(reason) => json!({ "ok": false, "error": reason }),
            };
            WireEvent {
                kind: "ToolCallCompleted",
                data: json!({ "tool": name, "call_id": call_id, "result": result_json }),
                stop_reason: None,
                token_usage: None,
            }
        }
        RunEvent::TurnCompleted {
            stop_reason: sr,
            usage,
            turn,
        } => {
            let sr_str = format!("{:?}", sr);
            let usage_json = usage
                .as_ref()
                .map(|u| json!({ "input_tokens": u.input_tokens, "output_tokens": u.output_tokens }))
                .unwrap_or(Value::Null);
            WireEvent {
                kind: "TurnCompleted",
                data: json!({ "turn": turn, "stop_reason": sr_str, "usage": usage_json }),
                stop_reason: Some(sr_str),
                token_usage: None,
            }
        }
        RunEvent::RunCompleted { outcome } => {
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
            data: json!({
                "kind": kind,
                "detail": detail,
                "context_json": context_json,
                "tool_error_variant": tool_error_variant,
            }),
            stop_reason: None,
            token_usage: None,
        },
        _ => WireEvent {
            kind: "Unknown",
            data: json!({}),
            stop_reason: None,
            token_usage: None,
        },
    }
}
```

- [ ] **Step 2: Register the module in `mod.rs`.** Add `mod wire;` near the other `mod` lines.

- [ ] **Step 3: Rewrite `dispatch_run.rs` to call `wire::`.**
  - Delete the inline `outcome_to_json` and `token_usage_to_json` fns (dispatch_run lines ~169-216).
  - In `execute_batch`: `let body = wire::outcome_to_json(&outcome);` (was `outcome_to_json(outcome)`).
  - Replace the body of `emit_event` with:

```rust
let w = wire::event_to_wire(event);
if let Some(sr) = w.stop_reason {
    *stop_reason = Some(sr);
}
if let Some(tu) = w.token_usage {
    *last_token_usage = Some(tu);
}
disp.send_notification(
    methods::RUNTIME_EVENT,
    json!({ "id": id, "kind": w.kind, "data": w.data }),
)
.await;
```

  - Add `use super::wire;`. Remove now-unused imports if the compiler flags them (`MessagePayload` may still be needed for building the initial message — keep that one).

- [ ] **Step 4: Add the golden test module at the bottom of `wire.rs`.** Constructs real `RunOutcome`/`RunEvent` values (enum-level `#[non_exhaustive]` permits external construction of existing variants; `Message::new`, runtime-core `TokenUsage` literal, `tau_ports::fixtures::make_token_usage`, `ToolResult::new`, `AgentStatus::Stopped`) and asserts the exact serialized JSON.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tau_domain::{Address, AgentStatus, Message, MessagePayload};
    use tau_ports::fixtures::make_token_usage;
    use tau_ports::tool::{ToolContent, ToolResult};
    use tau_runtime_tokio::{RunEvent, RunOutcome, TokenUsage};

    fn text_message(s: &str) -> Message {
        Message::new(
            Address::User,
            Address::User,
            MessagePayload::Text { content: s.into() },
        )
    }

    #[test]
    fn outcome_completed_wire_shape() {
        let outcome = RunOutcome::Completed {
            final_message: text_message("hi there"),
            all_messages: vec![],
            total_turns: 2,
            token_usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 10,
                total_tokens: Some(15),
            },
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
        let w = event_to_wire(&RunEvent::TextDelta { delta: "frag".into() });
        assert_eq!(w.kind, "TextDelta");
        assert_eq!(w.data, json!({ "text": "frag" }));
        assert!(w.stop_reason.is_none() && w.token_usage.is_none());
    }

    #[test]
    fn event_tool_call_started_wire_shape() {
        let w = event_to_wire(&RunEvent::ToolCallStarted {
            id: "call-1".into(),
            name: "echo".into(),
            args: json!({ "x": 1 }),
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
                    ToolContent::Text { text: "done".into() },
                    ToolContent::Json { data: json!({ "n": 2 }) },
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
            stop_reason: tau_ports::StopReason::EndTurn,
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
            stop_reason: tau_ports::StopReason::MaxTokens,
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
                token_usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: Some(3),
                },
            },
        });
        assert_eq!(w.kind, "RunCompleted");
        let tu = json!({ "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 });
        assert_eq!(w.data, json!({ "token_usage": tu.clone() }));
        assert_eq!(w.token_usage, Some(tu));
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
```

- [ ] **Step 5: Run the golden tests — expect GREEN against the hand-rolled `json!` bodies.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app wire::`
Expected: all `wire::tests::*` PASS. (If any fail, the golden literal was transcribed wrong — fix the literal to match the existing wire shape, NOT the code.)

- [ ] **Step 6: Run the full tau-app suite to confirm no regression in serve tests.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app`
Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
git add crates/tau-app/src/serve/wire.rs crates/tau-app/src/serve/mod.rs crates/tau-app/src/serve/dispatch_run.rs
git commit -m "refactor(tau-app): extract serve wire projection into wire.rs + golden tests"
```

---

### Task 2: Swap the `json!` bodies for typed `#[serde]` DTOs

Now make a runtime field rename a compile error by routing every projection through typed DTOs. The golden tests from Task 1 stay GREEN, proving byte-identity.

**Files:**
- Modify: `crates/tau-app/src/serve/wire.rs`

- [ ] **Step 1: Add the DTO types to `wire.rs` (above the conversion fns).**

```rust
use serde::Serialize;

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
    Unknown,
}

#[derive(Serialize)]
struct TokenUsageDto {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: Option<u64>,
}

#[derive(Serialize)]
struct PortTokenUsageDto {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ContentDto {
    Text { text: String },
    Json { data: Value },
    Unknown,
}

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

// Per-event `data` payloads. Field renames vs the runtime type are the
// whole point: a rename upstream breaks the destructure below and fails
// to compile.
#[derive(Serialize)]
struct TextDeltaDto {
    text: String,
}

#[derive(Serialize)]
struct ToolCallStartedDto {
    tool: String,
    args: Value,
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
struct RunCompletedDto {
    token_usage: Value,
}

#[derive(Serialize)]
struct FatalErrorDto {
    kind: String,
    detail: String,
    context_json: Option<String>,
    tool_error_variant: Option<String>,
}
```

Verify the OutcomeDto tag shape: `#[serde(tag = "status", rename_all = "lowercase")]` emits `{"status":"completed", ...}` / `{"status":"failed", ...}` / `{"status":"unknown"}` — matching the current `json!` output exactly. `ContentDto` with `tag="type", rename_all="lowercase"` emits `{"type":"text",...}` / `{"type":"json",...}` / `{"type":"unknown"}`.

- [ ] **Step 2: Add a `to_value` helper and rewrite `token_usage_to_json`.**

```rust
fn to_value<T: Serialize>(dto: &T) -> Value {
    // Infallible for these fixed-shape DTOs (no maps with non-string keys,
    // no custom serializers that can fail).
    serde_json::to_value(dto).expect("wire DTO serialization is infallible")
}

pub(crate) fn token_usage_to_json(usage: &TokenUsage) -> Value {
    to_value(&TokenUsageDto {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    })
}
```

- [ ] **Step 3: Rewrite `outcome_to_json` to build `OutcomeDto`.**

```rust
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
            agent_status: format!("{:?}", status),
            total_turns: *total_turns,
            token_usage: token_usage_dto(token_usage),
        },
        _ => OutcomeDto::Unknown,
    };
    to_value(&dto)
}

fn token_usage_dto(usage: &TokenUsage) -> TokenUsageDto {
    TokenUsageDto {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    }
}
```

(Refactor `token_usage_to_json` to `to_value(&token_usage_dto(usage))` to avoid duplication.)

- [ ] **Step 4: Rewrite `event_to_wire` to build the per-event DTOs.** Each arm constructs its DTO and calls `to_value`. The `ContentDto`/`ToolResultDto` mapping replaces the inline content loop:

```rust
RunEvent::ToolCallCompleted { id: call_id, name, result } => {
    let result_dto = match result {
        Ok(tr) => ToolResultDto::Ok {
            ok: true,
            content: tr.content.iter().map(content_dto).collect(),
            is_error: tr.is_error,
        },
        Err(reason) => ToolResultDto::Err { ok: false, error: reason.clone() },
    };
    WireEvent {
        kind: "ToolCallCompleted",
        data: to_value(&ToolCallCompletedDto {
            tool: name.clone(),
            call_id: call_id.clone(),
            result: result_dto,
        }),
        stop_reason: None,
        token_usage: None,
    }
}
```

with helper:

```rust
fn content_dto(c: &tau_ports::ToolContent) -> ContentDto {
    match c {
        tau_ports::ToolContent::Text { text } => ContentDto::Text { text: text.clone() },
        tau_ports::ToolContent::Json { data } => ContentDto::Json { data: data.clone() },
        _ => ContentDto::Unknown,
    }
}
```

Apply the same DTO treatment to `TextDelta` (`TextDeltaDto`), `ToolCallStarted` (`ToolCallStartedDto`), `TurnCompleted` (`TurnCompletedDto` with `usage: usage.as_ref().map(|u| PortTokenUsageDto { input_tokens: u.input_tokens, output_tokens: u.output_tokens })`, and `stop_reason: format!("{:?}", sr)` used for both the DTO field and the side-effect), `RunCompleted` (`RunCompletedDto { token_usage: tu }`), and `FatalError` (`FatalErrorDto`). The `_` guard arm stays `kind: "Unknown", data: json!({})` — `json!({})` and `to_value(&EmptyDto)` both yield `{}`, keep `json!({})` to avoid a needless empty struct.

- [ ] **Step 5: Run the golden tests — expect STILL GREEN (byte-identity proof).**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app wire::`
Expected: all `wire::tests::*` PASS, unchanged from Task 1.

- [ ] **Step 6: Run clippy + the full suite.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-app`
Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-app --all-targets -- -D warnings`
Expected: PASS, no warnings. Resolve any unused-import warnings (the `json!` import in `wire.rs` is still needed for the `_` guard arm and tests).

- [ ] **Step 7: Confirm a rename is now a compile error (manual proof, do NOT commit).** Temporarily rename `delta` → `delta_renamed` in the `RunEvent::TextDelta` destructure in `event_to_wire`; `cargo check -p tau-app` must fail. Revert.

- [ ] **Step 8: `cargo fmt`.**

Run: `timeout 30 env CARGO_TARGET_DIR=target/main cargo fmt -p tau-app`

- [ ] **Step 9: Commit.**

```bash
git add crates/tau-app/src/serve/wire.rs
git commit -m "refactor(tau-app): serialize serve wire contract via typed serde DTOs (D2)"
```

---

## Self-Review

- **Spec coverage:** D2 targets `outcome_to_json` / `token_usage_to_json` (Task 1+2 relocate+typify) and `emit_event` manual extraction (Task 1 extracts to `event_to_wire`, Task 2 typifies). ✓ Byte-identity guarded by golden tests written before the serde swap. ✓ Compile-error-on-rename proven in Task 2 Step 7. ✓
- **Out of scope (correctly):** the `{"id","kind","data"}` and `{"final": true, ...}` envelopes stay `json!` — serve-layer, not runtime projections. No protocol changes.
- **Type consistency:** `outcome_to_json(&RunOutcome)`, `event_to_wire(&RunEvent) -> WireEvent`, `token_usage_to_json(&TokenUsage)` used identically in plan + dispatch_run. `WireEvent` fields (`kind`/`data`/`stop_reason`/`token_usage`) consistent.
- **Placeholder scan:** none — every step has full code.
