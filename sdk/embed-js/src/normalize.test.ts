// Unit tests for normalize.ts — one case per wire-level RunEvent variant in
// schemas/run-event/run-event.v1.schema.json, plus the StopReason mapping, the
// optional-field paths, and the three throw-on-unknown guards. Behavioral
// counterpart to the Rust `run_event_ts_coverage` test (which guards the union
// shape); this asserts the runtime mapping from serde's externally-tagged wire
// format to the normalized union.

import { describe, expect, it } from "vitest";
import { normalize } from "./normalize";

describe("normalize", () => {
  it("maps the RunStarted unit variant", () => {
    expect(normalize("RunStarted")).toEqual({ type: "run-started" });
  });

  it("maps the InferenceCallStarted unit variant", () => {
    expect(normalize("InferenceCallStarted")).toEqual({ type: "inference-call-started" });
  });

  it("maps ContextStepRan", () => {
    expect(normalize({ ContextStepRan: { step: "plan", tokens_in: 3, tokens_out: 7 } })).toEqual({
      type: "context-step-ran",
      step: "plan",
      tokensIn: 3,
      tokensOut: 7,
    });
  });

  it("maps InferenceCallCompleted", () => {
    expect(
      normalize({ InferenceCallCompleted: { stop_reason: "EndTurn", tokens_in: 1, tokens_out: 2 } }),
    ).toEqual({ type: "inference-call-completed", stopReason: "end-turn", tokensIn: 1, tokensOut: 2 });
  });

  it("maps every StopReason spelling", () => {
    const cases: Array<[string, string]> = [
      ["EndTurn", "end-turn"],
      ["MaxTokens", "max-tokens"],
      ["ToolUse", "tool-use"],
      ["StopSequence", "stop-sequence"],
      ["Error", "error"],
    ];
    for (const [wire, normalized] of cases) {
      expect(
        normalize({ InferenceCallCompleted: { stop_reason: wire, tokens_in: 0, tokens_out: 0 } }),
      ).toMatchObject({ stopReason: normalized });
    }
  });

  it("maps TextDelta", () => {
    expect(normalize({ TextDelta: { delta: "Hi" } })).toEqual({ type: "text-delta", delta: "Hi" });
  });

  it("maps ToolCallStarted", () => {
    expect(normalize({ ToolCallStarted: { id: "1", name: "read", args: { path: "/x" } } })).toEqual({
      type: "tool-call-started",
      id: "1",
      name: "read",
      args: { path: "/x" },
    });
  });

  it("maps ToolCallCompleted with an Ok result", () => {
    expect(
      normalize({ ToolCallCompleted: { id: "1", name: "read", result: { Ok: "data" } } }),
    ).toEqual({ type: "tool-call-completed", id: "1", name: "read", result: { ok: "data" } });
  });

  it("maps ToolCallCompleted with an Err result", () => {
    expect(
      normalize({ ToolCallCompleted: { id: "1", name: "read", result: { Err: "boom" } } }),
    ).toEqual({ type: "tool-call-completed", id: "1", name: "read", result: { err: "boom" } });
  });

  it("maps TurnCompleted with usage", () => {
    expect(
      normalize({
        TurnCompleted: {
          stop_reason: "EndTurn",
          turn: 2,
          usage: { input_tokens: 5, output_tokens: 9, total_tokens: 14 },
        },
      }),
    ).toEqual({
      type: "turn-completed",
      stopReason: "end-turn",
      turn: 2,
      usage: { inputTokens: 5, outputTokens: 9, totalTokens: 14 },
    });
  });

  it("maps TurnCompleted without usage", () => {
    expect(
      normalize({ TurnCompleted: { stop_reason: "MaxTokens", turn: 1, usage: null } }),
    ).toEqual({ type: "turn-completed", stopReason: "max-tokens", turn: 1, usage: undefined });
  });

  it("maps RunCompleted with a Completed outcome", () => {
    expect(
      normalize({
        RunCompleted: {
          outcome: {
            Completed: {
              final_message: { role: "assistant" },
              all_messages: [1, 2],
              total_turns: 3,
              token_usage: { input_tokens: 10, output_tokens: 20 },
            },
          },
        },
      }),
    ).toEqual({
      type: "run-completed",
      outcome: {
        kind: "completed",
        finalMessage: { role: "assistant" },
        allMessages: [1, 2],
        totalTurns: 3,
        tokenUsage: { inputTokens: 10, outputTokens: 20 },
      },
    });
  });

  it("maps RunCompleted with a Failed outcome (externally-tagged status)", () => {
    expect(
      normalize({
        RunCompleted: {
          outcome: {
            Failed: {
              status: { Failed: { kind: "Budget", detail: "exceeded" } },
              all_messages: [],
              total_turns: 1,
              token_usage: { input_tokens: 1, output_tokens: 0, total_tokens: 1 },
            },
          },
        },
      }),
    ).toEqual({
      type: "run-completed",
      outcome: {
        kind: "failed",
        status: { kind: "Budget", detail: "exceeded" },
        allMessages: [],
        totalTurns: 1,
        tokenUsage: { inputTokens: 1, outputTokens: 0, totalTokens: 1 },
      },
    });
  });

  it("maps RunCompleted with a Failed outcome (bare-string status)", () => {
    expect(
      normalize({
        RunCompleted: {
          outcome: {
            Failed: {
              status: "Cancelled",
              all_messages: [],
              total_turns: 0,
              token_usage: { input_tokens: 0, output_tokens: 0 },
            },
          },
        },
      }),
    ).toMatchObject({
      type: "run-completed",
      outcome: { kind: "failed", status: { kind: "Cancelled" } },
    });
  });

  it("maps FatalError with optional fields", () => {
    expect(
      normalize({
        FatalError: { kind: "Boom", detail: "d", context_json: "{}", tool_error_variant: "X" },
      }),
    ).toEqual({
      type: "fatal-error",
      kind: "Boom",
      detail: "d",
      contextJson: "{}",
      toolErrorVariant: "X",
    });
  });

  it("maps FatalError without optional fields", () => {
    expect(normalize({ FatalError: { kind: "Boom", detail: "d" } })).toEqual({
      type: "fatal-error",
      kind: "Boom",
      detail: "d",
      contextJson: undefined,
      toolErrorVariant: undefined,
    });
  });

  it("throws on an unknown unit variant", () => {
    expect(() => normalize("Nope")).toThrow(/unknown unit RunEvent variant/);
  });

  it("throws on an unknown StopReason", () => {
    expect(() =>
      normalize({ InferenceCallCompleted: { stop_reason: "Weird", tokens_in: 0, tokens_out: 0 } }),
    ).toThrow(/unknown StopReason/);
  });

  it("throws on an unrecognized payload", () => {
    expect(() => normalize({ Bogus: {} })).toThrow(/unrecognized RunEvent payload/);
  });
});
