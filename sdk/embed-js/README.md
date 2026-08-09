# @tau/embed-js

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
