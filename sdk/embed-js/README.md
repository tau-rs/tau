# @tau/embed-js

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
