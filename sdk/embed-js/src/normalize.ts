// Maps the wire-level RunEvent (serde externally-tagged JSON emitted by the
// wasm guest's `emit-event` host import) to the normalized union in
// ./RunEvent. Every variant in schemas/run-event/run-event.v1.schema.json
// must have a case here; run_event_ts_coverage guards RunEvent.ts, not this
// file, so keep the two in lockstep by hand.

import type { RunEvent, RunOutcome, StopReason, TokenUsage, ToolResult } from "./RunEvent";

function toStopReason(raw: string): StopReason {
  switch (raw) {
    case "EndTurn":
      return "end-turn";
    case "MaxTokens":
      return "max-tokens";
    case "ToolUse":
      return "tool-use";
    case "StopSequence":
      return "stop-sequence";
    case "Error":
      return "error";
    default:
      throw new Error(`unknown StopReason: ${raw}`);
  }
}

function toTokenUsage(raw: {
  input_tokens: number;
  output_tokens: number;
  total_tokens?: number | null;
}): TokenUsage {
  return {
    inputTokens: raw.input_tokens,
    outputTokens: raw.output_tokens,
    ...(raw.total_tokens != null ? { totalTokens: raw.total_tokens } : {}),
  };
}

function toToolResult(raw: { Ok?: unknown; Err?: string }): ToolResult {
  if ("Ok" in raw) return { ok: raw.Ok };
  return { err: raw.Err as string };
}

function toRunOutcome(raw: {
  Completed?: {
    final_message: unknown;
    all_messages: unknown[];
    total_turns: number;
    token_usage: { input_tokens: number; output_tokens: number; total_tokens?: number | null };
  };
  Failed?: {
    status: { kind: string; detail?: string | null } | string;
    all_messages: unknown[];
    total_turns: number;
    token_usage: { input_tokens: number; output_tokens: number; total_tokens?: number | null };
  };
}): RunOutcome {
  if (raw.Completed) {
    const c = raw.Completed;
    return {
      kind: "completed",
      finalMessage: c.final_message,
      allMessages: c.all_messages,
      totalTurns: c.total_turns,
      tokenUsage: toTokenUsage(c.token_usage),
    };
  }
  const f = raw.Failed!;
  return {
    kind: "failed",
    status: typeof f.status === "string" ? { kind: f.status } : f.status,
    allMessages: f.all_messages,
    totalTurns: f.total_turns,
    tokenUsage: toTokenUsage(f.token_usage),
  };
}

/// Normalize a raw, externally-tagged RunEvent JSON value into the
/// idiomatic kebab-tagged union.
export function normalize(raw: unknown): RunEvent {
  if (typeof raw === "string") {
    switch (raw) {
      case "RunStarted":
        return { type: "run-started" };
      case "InferenceCallStarted":
        return { type: "inference-call-started" };
      default:
        throw new Error(`unknown unit RunEvent variant: ${raw}`);
    }
  }

  const obj = raw as Record<string, unknown>;

  if ("ContextStepRan" in obj) {
    const v = obj.ContextStepRan as { step: string; tokens_in: number; tokens_out: number };
    return { type: "context-step-ran", step: v.step, tokensIn: v.tokens_in, tokensOut: v.tokens_out };
  }
  if ("InferenceCallCompleted" in obj) {
    const v = obj.InferenceCallCompleted as {
      stop_reason: string;
      tokens_in: number;
      tokens_out: number;
    };
    return {
      type: "inference-call-completed",
      stopReason: toStopReason(v.stop_reason),
      tokensIn: v.tokens_in,
      tokensOut: v.tokens_out,
    };
  }
  if ("TextDelta" in obj) {
    const v = obj.TextDelta as { delta: string };
    return { type: "text-delta", delta: v.delta };
  }
  if ("ToolCallStarted" in obj) {
    const v = obj.ToolCallStarted as { id: string; name: string; args: unknown };
    return { type: "tool-call-started", id: v.id, name: v.name, args: v.args };
  }
  if ("ToolCallCompleted" in obj) {
    const v = obj.ToolCallCompleted as {
      id: string;
      name: string;
      result: { Ok?: unknown; Err?: string };
    };
    return { type: "tool-call-completed", id: v.id, name: v.name, result: toToolResult(v.result) };
  }
  if ("TurnCompleted" in obj) {
    const v = obj.TurnCompleted as {
      stop_reason: string;
      turn: number;
      usage?: { input_tokens: number; output_tokens: number; total_tokens?: number | null } | null;
    };
    return {
      type: "turn-completed",
      stopReason: toStopReason(v.stop_reason),
      usage: v.usage ? toTokenUsage(v.usage) : undefined,
      turn: v.turn,
    };
  }
  if ("RunCompleted" in obj) {
    const v = obj.RunCompleted as { outcome: Parameters<typeof toRunOutcome>[0] };
    return { type: "run-completed", outcome: toRunOutcome(v.outcome) };
  }
  if ("FatalError" in obj) {
    const v = obj.FatalError as { kind: string; detail: string; context_json?: string | null };
    return {
      type: "fatal-error",
      kind: v.kind,
      detail: v.detail,
      contextJson: v.context_json ?? undefined,
    };
  }

  throw new Error(`unrecognized RunEvent payload: ${JSON.stringify(raw)}`);
}
