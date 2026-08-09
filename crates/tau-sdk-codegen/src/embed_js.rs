//! Emit the `@tau/embed-js` host-embedding scaffold (Phase 2 §5.2 / EPIC
//! 5.4 foundation). Unlike `emit_ts`/`emit_python` this package is not
//! derived from the IR schema: it is hand-authored glue for driving a `tau
//! build wasm` component from JavaScript/TypeScript (main thread or Web
//! Worker), normalizing the wire-level `RunEvent` (schema-frozen at
//! `schemas/run-event/run-event.v1.schema.json`) into an idiomatic,
//! kebab-tagged TS union.
//!
//! `src/RunEvent.ts` is guarded against schema drift by
//! `tests/run_event_ts_coverage.rs`; the whole scaffold is guarded against
//! emitter drift by `tests/embed_js_drift.rs` (mirrors 5.3's
//! `tests/drift.rs`).
//!
//! `src/generated/` (jco's `transpile` output, e.g. `component.ts`) is a
//! build artifact of the emitted `package.json`'s `build` script, not of
//! this Rust emitter — it is listed in the emitted `.gitignore` and never
//! rendered here.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Render the `@tau/embed-js` scaffold as repo-relative-path -> contents.
pub fn render_embed_js() -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();

    out.insert(
        PathBuf::from("sdk/embed-js/package.json"),
        PACKAGE_JSON.to_string(),
    );
    out.insert(
        PathBuf::from("sdk/embed-js/.gitignore"),
        GITIGNORE.to_string(),
    );
    out.insert(PathBuf::from("sdk/embed-js/README.md"), README.to_string());
    out.insert(
        PathBuf::from("sdk/embed-js/src/RunEvent.ts"),
        RUN_EVENT_TS.to_string(),
    );
    out.insert(
        PathBuf::from("sdk/embed-js/src/normalize.ts"),
        NORMALIZE_TS.to_string(),
    );
    out.insert(
        PathBuf::from("sdk/embed-js/src/index.ts"),
        INDEX_TS.to_string(),
    );
    out.insert(
        PathBuf::from("sdk/embed-js/src/worker.ts"),
        WORKER_TS.to_string(),
    );

    out
}

const PACKAGE_JSON: &str = r#"{
  "name": "@tau/embed-js",
  "version": "0.0.0",
  "description": "Host-embedding glue for running tau wasm components from JavaScript/TypeScript.",
  "type": "module",
  "types": "src/index.ts",
  "main": "src/index.ts",
  "scripts": {
    "build": "jco transpile $npm_config_wasm --out-dir src/generated"
  },
  "devDependencies": {
    "jco": "^1",
    "typescript": "^5"
  }
}
"#;

const GITIGNORE: &str = r#"# jco transpile build output (produced by `npm run build --wasm=...`).
src/generated/
"#;

const README: &str = r#"# @tau/embed-js

Host-embedding glue for running a `tau build wasm` component from
JavaScript/TypeScript (Phase 2 §5.2), in the main thread or a Web Worker.
Normalizes the wire-level `RunEvent` stream into an idiomatic, kebab-tagged
TypeScript union (see `src/RunEvent.ts`).

## Build

```sh
npm install
npm run build --wasm=path/to/component.wasm
```

This runs `jco transpile <wasm> --out-dir src/generated`, producing the JS
bindings that `src/index.ts` imports from `./generated`.

## Usage

```ts
import { loadTau } from "@tau/embed-js";

const tau = await loadTau(new URL("./component.wasm", import.meta.url));
for await (const event of tau.run({ message: "hello" })) {
  if (event.type === "text-delta") process.stdout.write(event.delta);
}
```

For a Web Worker host, use `loadTauInWorker` instead — same `TauComponent`
surface, but the wasm component runs off the main thread.

## Package layout

- `src/RunEvent.ts` — hand-written `RunEvent` union. Guarded against schema
  drift by `run_event_ts_coverage` (see `crates/tau-sdk-codegen`).
- `src/normalize.ts` — maps the externally-tagged wire format (e.g.
  `{"TextDelta":{"delta":"..."}}`) to the normalized union.
- `src/index.ts` — `loadTau` / `loadTauInWorker` + `TauComponent`.
- `src/worker.ts` — the Web Worker host driven by `loadTauInWorker`.
- `src/generated/` — jco's build output (gitignored; not committed).
"#;

/// Hand-written; guarded by `run_event_ts_coverage` test against
/// `schemas/run-event/`.
const RUN_EVENT_TS: &str = r#"// Hand-written; guarded by run_event_ts_coverage test against schemas/run-event/.
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
"#;

const NORMALIZE_TS: &str = r#"// Maps the wire-level RunEvent (serde externally-tagged JSON emitted by the
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

// `AgentStatus` wire shape: either a bare unit-variant string (e.g.
// `"Ready"`) or the externally-tagged `{"Failed":{"kind":...,"detail":...}}`.
// `RunOutcome::Failed.status` is documented as always the latter, but we
// unwrap defensively rather than assume the wrapper is absent.
function toFailedStatus(
  raw: string | { Failed: { kind: string; detail?: string | null } },
): { kind: string; detail?: string | null } {
  if (typeof raw === "string") return { kind: raw };
  return { kind: raw.Failed.kind, detail: raw.Failed.detail };
}

function toRunOutcome(raw: {
  Completed?: {
    final_message: unknown;
    all_messages: unknown[];
    total_turns: number;
    token_usage: { input_tokens: number; output_tokens: number; total_tokens?: number | null };
  };
  Failed?: {
    status: string | { Failed: { kind: string; detail?: string | null } };
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
    status: toFailedStatus(f.status),
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
    const v = obj.FatalError as {
      kind: string;
      detail: string;
      context_json?: string | null;
      tool_error_variant?: string | null;
    };
    return {
      type: "fatal-error",
      kind: v.kind,
      detail: v.detail,
      contextJson: v.context_json ?? undefined,
      toolErrorVariant: v.tool_error_variant ?? undefined,
    };
  }

  throw new Error(`unrecognized RunEvent payload: ${JSON.stringify(raw)}`);
}
"#;

const INDEX_TS: &str = r#"// loadTau / loadTauInWorker (Phase 2 §5.2 "Public surface").
//
// Both entry points expose the same TauComponent surface: `run(input)`
// returns an AsyncIterable<RunEvent> fed by the wasm guest's `emit-event`
// host import, normalized via ./normalize.

import { normalize } from "./normalize";
import type { RunEvent } from "./RunEvent";

/** Input to a single agent run. */
export interface RunInput {
  agent?: string;
  message: string;
}

/** A loaded tau wasm component, ready to drive runs. */
export interface TauComponent {
  run(input: RunInput): AsyncIterable<RunEvent>;
}

type EmitEvent = (json: string) => void;

interface GeneratedExports {
  run(inputJson: string, hostImports: { emitEvent: EmitEvent }): Promise<void> | void;
}

interface AsyncQueue<T> extends AsyncIterable<T> {
  push(value: T): void;
  close(): void;
}

function makeQueue<T>(): AsyncQueue<T> {
  const items: T[] = [];
  const waiters: Array<(result: IteratorResult<T>) => void> = [];
  let closed = false;

  return {
    push(value: T) {
      const waiter = waiters.shift();
      if (waiter) waiter({ value, done: false });
      else items.push(value);
    },
    close() {
      closed = true;
      while (waiters.length > 0) {
        waiters.shift()!({ value: undefined as unknown as T, done: true });
      }
    },
    [Symbol.asyncIterator](): AsyncIterator<T> {
      return {
        next(): Promise<IteratorResult<T>> {
          if (items.length > 0) {
            return Promise.resolve({ value: items.shift() as T, done: false });
          }
          if (closed) {
            return Promise.resolve({ value: undefined as unknown as T, done: true });
          }
          return new Promise((resolve) => waiters.push(resolve));
        },
      };
    },
  };
}

// jco transpile output; produced by `npm run build --wasm=<path>` into
// ./generated (gitignored). Not present at emit time, so this import is
// resolved only after the consumer has run the build script.
async function instantiate(wasm: BufferSource | URL): Promise<GeneratedExports> {
  const mod = (await import("./generated/component.js")) as {
    instantiate?: (wasm: BufferSource | URL) => Promise<GeneratedExports>;
  } & GeneratedExports;
  return mod.instantiate ? await mod.instantiate(wasm) : mod;
}

/** Load a tau wasm component and run it on the calling thread. */
export async function loadTau(wasm: BufferSource | URL): Promise<TauComponent> {
  const exports = await instantiate(wasm);
  return {
    run(input: RunInput): AsyncIterable<RunEvent> {
      const queue = makeQueue<RunEvent>();
      const emitEvent: EmitEvent = (json: string) => {
        queue.push(normalize(JSON.parse(json)));
      };
      Promise.resolve(exports.run(JSON.stringify(input), { emitEvent })).finally(() => queue.close());
      return queue;
    },
  };
}

/** Load a tau wasm component and run it inside a dedicated Web Worker. */
export async function loadTauInWorker(wasm: URL): Promise<TauComponent> {
  return {
    run(input: RunInput): AsyncIterable<RunEvent> {
      const queue = makeQueue<RunEvent>();
      const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
      const onMessage = (ev: MessageEvent) => {
        const msg = ev.data as { kind: "event"; json: string } | { kind: "done" };
        if (msg.kind === "event") {
          queue.push(normalize(JSON.parse(msg.json)));
        } else {
          queue.close();
          worker.removeEventListener("message", onMessage);
          worker.terminate();
        }
      };
      worker.addEventListener("message", onMessage);
      worker.postMessage({ wasm: wasm.href, input });
      return queue;
    },
  };
}
"#;

const WORKER_TS: &str = r#"// Web Worker host driven by loadTauInWorker (./index.ts). Instantiates the
// jco-transpiled component off the main thread and forwards normalized
// events back via postMessage.

/// <reference lib="webworker" />

type RunMessage = { wasm: string; input: unknown };

interface GeneratedExports {
  run(inputJson: string, hostImports: { emitEvent: (json: string) => void }): Promise<void> | void;
}

async function instantiate(): Promise<GeneratedExports> {
  const mod = (await import("./generated/component.js")) as {
    instantiate?: () => Promise<GeneratedExports>;
  } & GeneratedExports;
  return mod.instantiate ? await mod.instantiate() : mod;
}

self.addEventListener("message", async (ev: MessageEvent<RunMessage>) => {
  const { input } = ev.data;
  const exports = await instantiate();
  const emitEvent = (json: string) => {
    (self as unknown as Worker).postMessage({ kind: "event", json });
  };
  await exports.run(JSON.stringify(input), { emitEvent });
  (self as unknown as Worker).postMessage({ kind: "done" });
});
"#;
