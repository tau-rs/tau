// Web Worker host driven by loadTauInWorker (./index.ts). Instantiates the
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
