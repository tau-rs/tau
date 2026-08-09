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

const tau = await loadTau(new URL("./component.wasm", import.meta.url), {
  // Bridges the WIT `complete` host import to a real LLM backend.
  // `requestJson` is a serialized `tau_ports::llm::CompletionRequest`;
  // must resolve to a serialized `CompletionResponse` JSON string.
  complete: async (requestJson) => callYourBackend(requestJson),
});
for await (const event of tau.run({ prompt: "hello" })) {
  if (event.type === "text-delta") process.stdout.write(event.delta);
}
```

`complete` has no safe default — omit it and the guest fails with a clear
"not configured" error the moment it calls the LLM. `nowMillis`/`nextU64`
default to `Date.now()`/`crypto.getRandomValues`; override both for
deterministic (e.g. conformance/cassette) runs.

For a Web Worker host, use `loadTauInWorker` instead — same `TauComponent`
surface, but the wasm component runs off the main thread. Its `complete`
cannot be bridged from the main thread (functions aren't
structured-cloneable via `postMessage`) and always rejects until a
dedicated RPC bridge is added.

> Host-import wiring (the `instantiate` helper in `src/index.ts` and
> `src/worker.ts`) is finalized and validated against real `jco transpile`
> output in EPIC 5.4-c (the streaming demo). The shapes here follow jco's
> documented async-instantiation convention but are not yet exercised
> end-to-end.

## Package layout

- `src/RunEvent.ts` — hand-written `RunEvent` union. Guarded against schema
  drift by `run_event_ts_coverage` (see `crates/tau-sdk-codegen`).
- `src/normalize.ts` — maps the externally-tagged wire format (e.g.
  `{"TextDelta":{"delta":"..."}}`) to the normalized union.
- `src/index.ts` — `loadTau` / `loadTauInWorker` + `TauComponent`, and the
  `tau:host/host` (wit/tau-host.wit) host-import wiring (`complete`,
  `nowMillis`, `nextU64`, `emitEvent`).
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
// Both entry points wire the wasm component's four `tau:host/host` imports
// (wit/tau-host.wit: `complete`, `now-millis`, `next-u64`, `emit-event`) at
// component instantiation time, then drive the world's single `run(prompt)`
// export. `run(input)` returns an AsyncIterable<RunEvent> fed by the
// `emit-event` host import, normalized via ./normalize.
//
// NOTE: the host-import wiring in `instantiate` below follows jco's
// documented async-instantiation convention (an imports object keyed by WIT
// interface name, e.g. `{ "tau:host/host": {...} }`). It has not been run
// against real `jco transpile` output yet — that validation is EPIC
// 5.4-c's job (the streaming demo). If the actual generated shape differs,
// only `instantiate` needs to change; the public loadTau/loadTauInWorker/
// RunInput/HostImports surface is stable regardless.

import { normalize } from "./normalize";
import type { RunEvent } from "./RunEvent";

/** Input to a single agent run. The guest world (wit/tau-host.wit `runner`)
 * exports a single-agent, prompt-only `run` — there is no per-call agent
 * selection at this layer. */
export interface RunInput {
  prompt: string;
}

/** Bridges the WIT `complete` host import to a real LLM backend.
 * `requestJson` is a serialized `tau_ports::llm::CompletionRequest`; must
 * resolve to a serialized `CompletionResponse` JSON string. */
export type CompleteFn = (requestJson: string) => Promise<string>;

/** The four imports `tau:host/host` requires (wit/tau-host.wit). Wired once
 * per component instantiation, not per-call — `run` itself only takes the
 * prompt. */
export interface HostImports {
  complete: CompleteFn;
  nowMillis: () => bigint;
  nextU64: () => bigint;
  emitEvent: (json: string) => void;
}

/** Caller-supplied overrides for `loadTau`/`loadTauInWorker`. `complete` has
 * no safe default: omit it and the guest fails with a clear error the
 * moment it calls the LLM. `nowMillis`/`nextU64` default to `Date.now()`
 * and a `crypto.getRandomValues`-backed u64 — override both for
 * deterministic (e.g. conformance/cassette) runs. */
export type HostImportOverrides = Partial<HostImports>;

/** A loaded tau wasm component, ready to drive runs. */
export interface TauComponent {
  run(input: RunInput): AsyncIterable<RunEvent>;
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

function unconfiguredComplete(): CompleteFn {
  return () =>
    Promise.reject(
      new Error(
        "@tau/embed-js: no `complete` host import configured — pass " +
          "{ complete } to loadTau()/loadTauInWorker() to bridge " +
          "tau:host/host's `complete` import to an LLM backend.",
      ),
    );
}

/** `crypto.getRandomValues`-backed u64 source for the `next-u64` host
 * import. Non-deterministic — override via `HostImportOverrides` for
 * conformance/cassette runs. */
function defaultNextU64(): () => bigint {
  const buf = new Uint32Array(2);
  return () => {
    crypto.getRandomValues(buf);
    return (BigInt(buf[0]) << 32n) | BigInt(buf[1]);
  };
}

function resolveHostImports(
  emitEvent: (json: string) => void,
  overrides: HostImportOverrides,
): HostImports {
  return {
    complete: overrides.complete ?? unconfiguredComplete(),
    nowMillis: overrides.nowMillis ?? (() => BigInt(Date.now())),
    nextU64: overrides.nextU64 ?? defaultNextU64(),
    emitEvent: overrides.emitEvent ?? emitEvent,
  };
}

function toFatalErrorEvent(err: unknown): RunEvent {
  return {
    type: "fatal-error",
    kind: "EmbedJsHostError",
    detail: err instanceof Error ? err.message : String(err),
  };
}

// jco transpile output; produced by `npm run build --wasm=<path>` into
// ./generated (gitignored). Not present at emit time, so this import is
// resolved only after the consumer has run the build script.
interface GeneratedExports {
  run(prompt: string): Promise<string> | string;
}

async function instantiate(
  wasm: BufferSource | URL,
  hostImports: HostImports,
): Promise<GeneratedExports> {
  const mod = (await import("./generated/component.js")) as {
    instantiate?: (
      wasm: BufferSource | URL,
      imports: { "tau:host/host": HostImports },
    ) => Promise<GeneratedExports>;
  } & GeneratedExports;
  return mod.instantiate
    ? await mod.instantiate(wasm, { "tau:host/host": hostImports })
    : mod;
}

/** Load a tau wasm component and run it on the calling thread. */
export async function loadTau(
  wasm: BufferSource | URL,
  overrides: HostImportOverrides = {},
): Promise<TauComponent> {
  return {
    run(input: RunInput): AsyncIterable<RunEvent> {
      const queue = makeQueue<RunEvent>();
      const emitEvent = (json: string) => queue.push(normalize(JSON.parse(json)));
      const hostImports = resolveHostImports(emitEvent, overrides);
      instantiate(wasm, hostImports)
        .then((exports) => exports.run(input.prompt))
        .catch((err: unknown) => queue.push(toFatalErrorEvent(err)))
        .finally(() => queue.close());
      return queue;
    },
  };
}

/** Load a tau wasm component and run it inside a dedicated Web Worker.
 *
 * Note: `complete` cannot be bridged from the main thread today —
 * functions are not structured-cloneable via `postMessage`. The worker's
 * `complete` always rejects with a clear "not configured" error until a
 * MessageChannel (or similar) RPC bridge is added; single-threaded
 * `loadTau` is the only path that supports a real `complete` today. */
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
// jco-transpiled component off the main thread against all four
// `tau:host/host` imports (wit/tau-host.wit), loading the wasm URL posted
// from the main thread, then forwards normalized events back via
// postMessage.
//
// See index.ts's `instantiate` for the jco-shape caveat: this glue mirrors
// it, and both get validated against real jco output together in EPIC
// 5.4-c.
//
// `complete` cannot be bridged from the main thread here (functions are not
// structured-cloneable via postMessage), so it always rejects with a clear
// "not configured" error — see loadTauInWorker's doc comment in index.ts.

/// <reference lib="webworker" />

import type { RunInput } from "./index";

type RunMessage = { wasm: string; input: RunInput };
type WorkerMessage = { kind: "event"; json: string } | { kind: "done" };

interface HostImports {
  complete: (requestJson: string) => Promise<string>;
  nowMillis: () => bigint;
  nextU64: () => bigint;
  emitEvent: (json: string) => void;
}

interface GeneratedExports {
  run(prompt: string): Promise<string> | string;
}

async function instantiate(wasm: string, hostImports: HostImports): Promise<GeneratedExports> {
  const mod = (await import("./generated/component.js")) as {
    instantiate?: (
      wasm: string,
      imports: { "tau:host/host": HostImports },
    ) => Promise<GeneratedExports>;
  } & GeneratedExports;
  return mod.instantiate
    ? await mod.instantiate(wasm, { "tau:host/host": hostImports })
    : mod;
}

function defaultNextU64(): () => bigint {
  const buf = new Uint32Array(2);
  return () => {
    crypto.getRandomValues(buf);
    return (BigInt(buf[0]) << 32n) | BigInt(buf[1]);
  };
}

self.addEventListener("message", async (ev: MessageEvent<RunMessage>) => {
  const { input, wasm } = ev.data;
  const post = (msg: WorkerMessage) => (self as unknown as Worker).postMessage(msg);
  const hostImports: HostImports = {
    complete: () =>
      Promise.reject(
        new Error(
          "@tau/embed-js worker: no `complete` host import configured — " +
            "see loadTauInWorker's doc comment in index.ts.",
        ),
      ),
    nowMillis: () => BigInt(Date.now()),
    nextU64: defaultNextU64(),
    emitEvent: (json: string) => post({ kind: "event", json }),
  };
  try {
    const exports = await instantiate(wasm, hostImports);
    await exports.run(input.prompt);
  } catch (err) {
    post({
      kind: "event",
      json: JSON.stringify({
        FatalError: {
          kind: "EmbedJsHostError",
          detail: err instanceof Error ? err.message : String(err),
        },
      }),
    });
  } finally {
    post({ kind: "done" });
  }
});
"#;
