// @tau/angular — typed Angular bindings for streaming a tau wasm component.
//
// Re-exports the embed-js event types so a consumer can type its component
// from a single import (`import { TauRunService, type RunEvent } from
// "@tau/angular"`).
export { TauRunService } from "./tau-run.service";
export type {
  RunEvent,
  RunInput,
  RunOutcome,
  StopReason,
  TauComponent,
  TokenUsage,
  ToolResult,
} from "@tau/embed-js";
