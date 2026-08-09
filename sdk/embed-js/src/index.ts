// loadTau / loadTauInWorker (Phase 2 §5.2 "Public surface").
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
