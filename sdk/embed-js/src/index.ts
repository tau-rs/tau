// loadTau / loadTauInWorker (Phase 2 §5.2 "Public surface").
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
