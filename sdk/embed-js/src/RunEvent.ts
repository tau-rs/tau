// Hand-written; guarded by run_event_ts_coverage test against schemas/run-event/.
//
// Mirrors `RunEvent` (crates/tau-runtime-core/src/stream.rs), normalized
// from serde's externally-tagged wire format (see normalize.ts) into
// camelCase fields and kebab-case `type` discriminants.

export type StopReason =
  | "end-turn"
  | "max-tokens"
  | "tool-use"
  | "stop-sequence"
  | "error";

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
  totalTokens?: number;
}

export type ToolResult = { ok: unknown } | { err: string };

export interface RunOutcomeCompleted {
  kind: "completed";
  finalMessage: unknown;
  allMessages: unknown[];
  totalTurns: number;
  tokenUsage: TokenUsage;
}

export interface RunOutcomeFailed {
  kind: "failed";
  status: { kind: string; detail?: string | null };
  allMessages: unknown[];
  totalTurns: number;
  tokenUsage: TokenUsage;
}

export type RunOutcome = RunOutcomeCompleted | RunOutcomeFailed;

export type RunEvent =
  | { type: "run-started" }
  | { type: "context-step-ran"; step: string; tokensIn: number; tokensOut: number }
  | { type: "inference-call-started" }
  | { type: "inference-call-completed"; stopReason: StopReason; tokensIn: number; tokensOut: number }
  | { type: "text-delta"; delta: string }
  | { type: "tool-call-started"; id: string; name: string; args: unknown }
  | { type: "tool-call-completed"; id: string; name: string; result: ToolResult }
  | { type: "turn-completed"; stopReason: StopReason; usage?: TokenUsage; turn: number }
  | { type: "run-completed"; outcome: RunOutcome }
  | {
      type: "fatal-error";
      kind: string;
      detail: string;
      contextJson?: string;
      toolErrorVariant?: string | null;
    };
