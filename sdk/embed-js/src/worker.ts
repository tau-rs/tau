// Web Worker host driven by loadTauInWorker (./index.ts). Instantiates the
// jco-transpiled component (./generated/component.js, `--instantiation
// async`) off the main thread against all four `tau:host/host` imports
// (wit/tau-host.wit), then forwards normalized events back via postMessage.
//
// Host imports cannot be bridged from the main thread here (functions are
// not structured-cloneable via postMessage), so `complete` always throws a
// clear "not configured" error and `nowMillis`/`nextU64` use worker-local
// defaults — see loadTauInWorker's doc comment in index.ts.

/// <reference lib="webworker" />

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
