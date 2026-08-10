// @tau/react — typed React bindings for streaming a tau wasm component.
//
// Re-exports the embed-js event types so a consumer can type its UI from a
// single import (`import { useTauRun, type RunEvent } from "@tau/react"`).
export { useTauRun } from "./useTauRun";
export type { TauRunStatus, TauRunError, UseTauRun } from "./useTauRun";
export type {
  RunEvent,
  RunInput,
  RunOutcome,
  StopReason,
  TauComponent,
  TokenUsage,
  ToolResult,
} from "@tau/embed-js";
