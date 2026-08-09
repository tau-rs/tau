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
//! `src/generated/` (jco's `transpile` output, e.g. `component.js`) is a
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
    out.insert(
        PathBuf::from("sdk/embed-js/src/generated.d.ts"),
        GENERATED_DTS.to_string(),
    );
    out.insert(
        PathBuf::from("sdk/embed-js/vitest.config.ts"),
        VITEST_CONFIG_TS.to_string(),
    );
    out.insert(
        PathBuf::from("sdk/embed-js/src/normalize.test.ts"),
        NORMALIZE_TEST_TS.to_string(),
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
    "build": "jco transpile --instantiation async --name component --out-dir src/generated",
    "test": "vitest run"
  },
  "devDependencies": {
    "@bytecodealliance/jco": "^1.27",
    "typescript": "^5",
    "vitest": "^3"
  }
}
"#;

const GITIGNORE: &str = r#"# npm install output (this is a library scaffold; the lockfile is not pinned).
node_modules/
package-lock.json

# jco transpile build output (produced by `npm run build --wasm=...`).
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
npm run build -- path/to/component.wasm
```

This runs `jco transpile --instantiation async --name component --out-dir
src/generated <wasm>`, producing the JS bindings (`component.js` plus the
sibling `component.core*.wasm` core modules) that `src/index.ts` imports from
`./generated`. (The `--` passes the component path through to `jco`; npm no
longer forwards `--flag=value` into scripts as `npm_config_*`.)

## Usage

```ts
import { loadTau } from "@tau/embed-js";

const tau = await loadTau({
  // Bridges the WIT `complete` host import to an LLM backend. `requestJson`
  // is a serialized `tau_ports::llm::CompletionRequest`; returns a
  // serialized `CompletionResponse` JSON string. SYNCHRONOUS — see below.
  complete: (requestJson) => lookUpCassetteResponse(requestJson),
});
for await (const event of tau.run({ prompt: "hello" })) {
  if (event.type === "text-delta") process.stdout.write(event.delta);
}
```

`complete` is a **synchronous** host import: `tau:host/host`'s `complete` is
a sync WIT function and this package transpiles the component in jco's
default sync mode, so the guest blocks on the return value. A backend that
must await I/O — a live network LLM — cannot be bridged as-is; supply a
`complete` backed by preloaded/cassette responses. Live async inference
needs a `jco --async-mode jspi` build and is a documented follow-up. Omit
`complete` and the guest throws a clear "not configured" error the moment it
calls the LLM. `nowMillis`/`nextU64` default to
`Date.now()`/`crypto.getRandomValues`; override both for deterministic
(e.g. conformance/cassette) runs.

For a Web Worker host, use `loadTauInWorker` instead — same `TauComponent`
surface, but the component runs off the main thread. Host imports can't be
bridged across `postMessage` (functions aren't structured-cloneable), so its
`complete` always throws until a dedicated RPC bridge is added.

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
// The host-import wiring in `instantiate` below targets `jco transpile
// --instantiation async` output (this package's `build` script): the
// generated `./generated/component.js` exposes `instantiate(getCoreModule,
// imports)` keyed by the (unversioned) WIT interface name `tau:host/host`,
// and its `run` export returns an empty sentinel string synchronously while
// streaming RunEvents through `emit-event`. Validated end-to-end against
// real jco output (EPIC 5.4 F1).

/// <reference path="./generated.d.ts" />

import { normalize } from "./normalize";
import type { RunEvent } from "./RunEvent";

// Re-export the public event types. `TauComponent.run` yields `RunEvent`, so
// these are part of this package's public surface; re-exporting them lets
// consumers (e.g. @tau/react, @tau/angular) name what a run emits without a
// deep `@tau/embed-js/src/RunEvent` import.
export type {
  RunEvent,
  RunOutcome,
  RunOutcomeCompleted,
  RunOutcomeFailed,
  StopReason,
  TokenUsage,
  ToolResult,
} from "./RunEvent";

/** Input to a single agent run. The guest world (wit/tau-host.wit `runner`)
 * exports a single-agent, prompt-only `run` — there is no per-call agent
 * selection at this layer. */
export interface RunInput {
  prompt: string;
}

/** Bridges the WIT `complete` host import to an LLM backend. `requestJson`
 * is a serialized `tau_ports::llm::CompletionRequest`; returns a serialized
 * `CompletionResponse` JSON string.
 *
 * SYNCHRONOUS: `tau:host/host`'s `complete` is a sync WIT import and this
 * package transpiles the component in jco's default sync mode, so the guest
 * blocks on the return value. A backend that must await I/O (a live network
 * LLM) therefore cannot be bridged here as-is — supply a `complete` backed
 * by preloaded/cassette responses. Live async inference needs a
 * `jco --async-mode jspi` build and is a documented follow-up. */
export type CompleteFn = (requestJson: string) => string;

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
  return () => {
    throw new Error(
      "@tau/embed-js: no `complete` host import configured — pass " +
        "{ complete } to loadTau() to bridge tau:host/host's `complete` " +
        "import to an LLM backend.",
    );
  };
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

// `jco transpile --instantiation async` output; produced by `npm run build
// --wasm=<path>` into ./generated (gitignored). Not present at emit time, so
// this import is resolved only after the consumer has run the build script.
// `getCoreModule` is optional — omitting it lets jco load the sibling
// `component.core*.wasm` files relative to the generated module (Node fs /
// browser fetch / bundler asset URL). `run` returns the empty sentinel
// string synchronously; the run's RunEvents arrive via `emit-event`.
interface GeneratedRoot {
  run(prompt: string): string;
}

interface GeneratedModule {
  instantiate(
    getCoreModule: undefined,
    imports: { "tau:host/host": HostImports },
  ): GeneratedRoot | Promise<GeneratedRoot>;
}

async function instantiate(hostImports: HostImports): Promise<GeneratedRoot> {
  const mod = (await import("./generated/component.js")) as unknown as GeneratedModule;
  return await mod.instantiate(undefined, { "tau:host/host": hostImports });
}

/** Load this package's bundled tau wasm component (jco-transpiled into
 * ./generated by `npm run build`) and run it on the calling thread. There is
 * no wasm argument: `--instantiation async` bakes the component into the
 * generated module + sibling core wasm, which `instantiate` imports. */
export async function loadTau(
  overrides: HostImportOverrides = {},
): Promise<TauComponent> {
  return {
    run(input: RunInput): AsyncIterable<RunEvent> {
      const queue = makeQueue<RunEvent>();
      const emitEvent = (json: string) => queue.push(normalize(JSON.parse(json)));
      const hostImports = resolveHostImports(emitEvent, overrides);
      instantiate(hostImports)
        .then((root) => root.run(input.prompt))
        .catch((err: unknown) => queue.push(toFatalErrorEvent(err)))
        .finally(() => queue.close());
      return queue;
    },
  };
}

/** Load the bundled component inside a dedicated Web Worker.
 *
 * Note: host imports cannot be bridged from the main thread — functions are
 * not structured-cloneable via `postMessage`. The worker uses its own
 * defaults: `complete` always throws a clear "not configured" error (so this
 * path is for components that never call the LLM, or a future MessageChannel
 * RPC bridge), and `nowMillis`/`nextU64` are non-deterministic.
 * Single-threaded `loadTau` is the only path that supports a real
 * `complete`. */
export async function loadTauInWorker(): Promise<TauComponent> {
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
      worker.postMessage({ input });
      return queue;
    },
  };
}
"#;

const WORKER_TS: &str = r#"// Web Worker host driven by loadTauInWorker (./index.ts). Instantiates the
// jco-transpiled component (./generated/component.js, `--instantiation
// async`) off the main thread against all four `tau:host/host` imports
// (wit/tau-host.wit), then forwards normalized events back via postMessage.
//
// Host imports cannot be bridged from the main thread here (functions are
// not structured-cloneable via postMessage), so `complete` always throws a
// clear "not configured" error and `nowMillis`/`nextU64` use worker-local
// defaults — see loadTauInWorker's doc comment in index.ts.

/// <reference lib="webworker" />
/// <reference path="./generated.d.ts" />

import type { RunInput } from "./index";

type RunMessage = { input: RunInput };
type WorkerMessage = { kind: "event"; json: string } | { kind: "done" };

interface HostImports {
  complete: (requestJson: string) => string;
  nowMillis: () => bigint;
  nextU64: () => bigint;
  emitEvent: (json: string) => void;
}

interface GeneratedRoot {
  run(prompt: string): string;
}

interface GeneratedModule {
  instantiate(
    getCoreModule: undefined,
    imports: { "tau:host/host": HostImports },
  ): GeneratedRoot | Promise<GeneratedRoot>;
}

async function instantiate(hostImports: HostImports): Promise<GeneratedRoot> {
  const mod = (await import("./generated/component.js")) as unknown as GeneratedModule;
  return await mod.instantiate(undefined, { "tau:host/host": hostImports });
}

function defaultNextU64(): () => bigint {
  const buf = new Uint32Array(2);
  return () => {
    crypto.getRandomValues(buf);
    return (BigInt(buf[0]) << 32n) | BigInt(buf[1]);
  };
}

self.addEventListener("message", async (ev: MessageEvent<RunMessage>) => {
  const { input } = ev.data;
  const post = (msg: WorkerMessage) => (self as unknown as Worker).postMessage(msg);
  const hostImports: HostImports = {
    complete: () => {
      throw new Error(
        "@tau/embed-js worker: no `complete` host import configured — " +
          "see loadTauInWorker's doc comment in index.ts.",
      );
    },
    nowMillis: () => BigInt(Date.now()),
    nextU64: defaultNextU64(),
    emitEvent: (json: string) => post({ kind: "event", json }),
  };
  try {
    const root = await instantiate(hostImports);
    root.run(input.prompt);
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

const GENERATED_DTS: &str = r#"// Ambient type for jco's transpile output (src/generated/component.js) — a
// gitignored build artifact that does not exist until `npm run build`.
// Declaring it lets this package's .ts source (index.ts, worker.ts) and any
// downstream consumer of the .ts entry (@tau/react, @tau/angular) typecheck
// without the generated module present. index.ts/worker.ts cast the dynamic
// import to their own `GeneratedModule` shape, so the loose typing here is
// deliberate; when a real build is present, actual module resolution wins over
// this wildcard and the cast still holds.
declare module "*/generated/component.js" {
  export function instantiate(
    getCoreModule: undefined,
    imports: { "tau:host/host": unknown },
  ): unknown;
}
"#;

const VITEST_CONFIG_TS: &str = r#"import { defineConfig } from "vitest/config";

// Runs the hand-authored TS unit tests (currently src/normalize.test.ts) in a
// plain Node environment. `normalize.ts` is pure JSON-shape mapping — no DOM,
// no wasm — so `node` is sufficient and keeps the suite dependency-free beyond
// vitest itself.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
"#;

const NORMALIZE_TEST_TS: &str = r#"// Unit tests for normalize.ts — one case per wire-level RunEvent variant in
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
"#;
