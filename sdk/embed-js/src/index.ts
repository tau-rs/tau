// loadTau / loadTauInWorker (Phase 2 §5.2 "Public surface").
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
