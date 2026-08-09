// Web Worker host driven by loadTauInWorker (./index.ts). Instantiates the
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
