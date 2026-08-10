# streaming-demo

EPIC 5.4-c acceptance: a Vite + React app that renders a `tau build wasm`
component's streaming text **live**, driven by [`@tau/react`](../../sdk/react)'s
`useTauRun` over [`@tau/embed-js`](../../sdk/embed-js).

It proves the full typed-consumer path end-to-end:

```
component.wasm ──jco transpile──▶ @tau/embed-js (loadTau) ──▶ @tau/react (useTauRun) ──▶ live <pre>{text}</pre>
```

## What it shows

- `loadTau({ complete })` instantiates the jco-transpiled component in the
  browser. jco's `--instantiation async` output loads its core wasm via
  `new URL('./component.coreN.wasm', import.meta.url)` — this demo confirms
  Vite bundles and serves that correctly (dev and `vite build`).
- A **synchronous cassette `complete`** (`src/cassette.ts`) bridges the sync
  `tau:host/host` `complete` import with a canned response — no network, no
  API key. (Live async inference needs a `jco --async-mode jspi` build; see
  `@tau/embed-js`.)
- `useTauRun(tau).text` renders the assistant text as its `text-delta` events
  stream in.

## Run

```sh
npm install
npm run dev          # transpiles the component, then starts Vite
# → open the printed URL, click "Run"
```

`npm run build` produces a production bundle the same way; `npm run preview`
serves it.

Both scripts first run `build:component`, which invokes
`@tau/embed-js`'s `npm run build` on `src/component.wasm`, transpiling it into
`sdk/embed-js/src/generated/` (gitignored) — the single-component location
`loadTau` imports from.

## The bundled component

`src/component.wasm` (committed) is a trivial 1-agent cassette project built
with:

```sh
tau build wasm crates/tau-cli/tests/fixtures/wasm-build/trivial \
  --allow-ungoverned -o examples/streaming-demo/src/component.wasm
```

Rebuild it with that command if the guest or IR format changes. Requires the
`wasm32-wasip2` Rust target.

## Scope

Single-agent only — the guest exports one prompt-only `run`. Multi-agent
rendering uses the separate `TraceEvent` vocab and is a later slice.
