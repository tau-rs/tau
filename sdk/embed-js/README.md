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

const tau = await loadTau(new URL("./component.wasm", import.meta.url));
for await (const event of tau.run({ message: "hello" })) {
  if (event.type === "text-delta") process.stdout.write(event.delta);
}
```

For a Web Worker host, use `loadTauInWorker` instead — same `TauComponent`
surface, but the wasm component runs off the main thread.

## Package layout

- `src/RunEvent.ts` — hand-written `RunEvent` union. Guarded against schema
  drift by `run_event_ts_coverage` (see `crates/tau-sdk-codegen`).
- `src/normalize.ts` — maps the externally-tagged wire format (e.g.
  `{"TextDelta":{"delta":"..."}}`) to the normalized union.
- `src/index.ts` — `loadTau` / `loadTauInWorker` + `TauComponent`.
- `src/worker.ts` — the Web Worker host driven by `loadTauInWorker`.
- `src/generated/` — jco's build output (gitignored; not committed).
