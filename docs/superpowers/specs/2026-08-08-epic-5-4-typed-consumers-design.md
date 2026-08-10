# EPIC 5.4 — Typed React/Angular consumers over `tau embed --host js`

**Status:** design approved 2026-08-08 · **Depends on:** EPIC 5.2 (`tau embed --host js`, unbuilt), EPIC 5.3 (`tau-sdk-codegen`, shipped)
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 5.4 — *"Typed React hook + Angular service (jco + ergonomic `tau embed` wrappers; Web Worker; `RunEvent` stream). Accept: typed npm package; demo renders streaming."*

## TL;DR

Ship a typed streaming path from a `tau build wasm` component to React/Angular apps. The
work layers cleanly onto the roadmap's own 5.2/5.4 split:

- **5.2** `tau embed --host js` emits `@tau/embed-js` — jco-transpiled host glue that
  instantiates the component and exposes `run(input): AsyncIterable<RunEvent>`.
- **5.4** thin typed consumers `@tau/react` (`useTauRun`) and `@tau/angular`
  (`TauRunService`) over `@tau/embed-js`, plus a streaming demo.

The `RunEvent` TS types are **generated from a frozen `RunEvent` JSON schema** via the
existing 5.3 `tau-sdk-codegen` emitter — single source of truth in the Rust enum, drift
tested. The wasm→JS transport is a **dumb JSON pipe** (a new host-import callback), so the
transport never encodes the event types; the types come from the schema.

## Ground truth (verified 2026-08-08, this repo)

- ✅ `tau build wasm` → `wasm32-wasip2` component. WIT world generated from allow-bounded
  caps at build (`crates/tau-cli/src/cmd/build_wasm.rs`, `crates/tau-wasm-guest/`).
- ✅ `tau-sdk-codegen` (EPIC 5.3) exists: `emit_ts.rs`, `emit_python.rs`, `schema.rs`;
  consumes a frozen JSON schema and emits typed SDKs. IR types use
  `#[derive(schemars::JsonSchema)]`; schemas frozen under `schemas/ir/*.schema.json`.
- ❌ `tau embed` — no `embed` subcommand in `tau-cli` (only "embedded IR" references).
- ❌ jco — absent from the tree.
- ❌ No `RunEvent` JSON schema. `RunEvent` (`crates/tau-runtime-core/src/stream.rs:129`)
  derives `Serialize/Deserialize` only, is `#[non_exhaustive]`, and is **externally
  tagged**: `{"TextDelta":{"delta":"…"}}`.
- ⚠️ **No streaming export exists.** The WIT world exports a single blocking
  `run: func(prompt: string) -> result<string, string>`. `guest.rs:138` does
  `collect_stream(...)` into a `Vec` then `serde_json::to_string(&events)` — the whole run
  is buffered into one JSON blob. Streaming across the component boundary must be added.
- ⚠️ Guest runs **exactly one agent** (`guest.rs:106` hard-errors if `agents.len() != 1`);
  no pipelines.

## Decisions

### D1 — Layering: 5.2 embed glue, 5.4 thin consumers (option A)

Honor the roadmap's split. `tau embed --host js` (5.2) owns the jco wiring and emits
`@tau/embed-js`; `@tau/react` / `@tau/angular` (5.4) are thin typed layers on top.

```
5.2  tau embed --host js  ─emits─▶  @tau/embed-js  ──dep──┬─▶ @tau/react
                                    (jco + shim + Worker)  └─▶ @tau/angular
                                                                  └─▶ examples/streaming-demo
```

Rejected: folding embed into 5.4. It makes 5.2 a hollow item and traps the jco wiring
inside the framework packages, so Vue/Svelte/vanilla hosts can't reuse it.

### D2 — Transport: host-import callback, events as serialized JSON (option A)

Add one host **import** mirroring the existing `complete`/`now-millis` imports:

```wit
interface host {
    emit-event: func(event-json: string);                        // NEW
    complete: func(request-json: string) -> result<string, string>;
    now-millis: func() -> u64;
    next-u64: func() -> u64;
}
world runner {
    import host;
    export run: func(prompt: string) -> result<string, string>;  // unchanged
}
```

The guest calls `emit-event` once per `RunEvent` instead of buffering into one blob. The JS
side supplies a closure that `JSON.parse`s + normalizes each event and pushes it into an
async-iterable queue; `run` resolving closes the queue.

`run`'s return value changes meaning: it no longer carries the event array (events now flow
through `emit-event`). Its `ok` payload becomes a terminal completion sentinel (empty
string, or the serialized `RunOutcome` for hosts that don't consume the stream); its `err`
payload carries a fatal error that aborts before/around the stream. The run's own outcome is
also observable in-band as the terminal `run-completed` / `fatal-error` events, so
stream-only consumers never need the return value. The `run` WIT *signature* is unchanged;
only the payload semantics move.

Rejected:
- **WIT `stream<run-event>`** (typed variant at the boundary): requires the guest to grow
  async-component-model support it does not use, *and* forces a hand-maintained WIT
  `variant` mirroring a `#[non_exhaustive]` enum — a second source of truth and the exact
  drift EPIC 5.3 was built to eliminate. Typed-but-hand-synced is worse than
  untyped-wire-plus-generated-types.
- **WIT `stream<string>`**: carries the async-stream cost of the above while still shipping
  JSON strings; buys nothing over the callback.

The callback path needs no async-component-model, reuses the existing host-import pattern,
and yields structured-cloneable POJOs — required for the Web Worker path anyway.

### D3 — Types: frozen `RunEvent` schema + hand-written TS union, both drift-guarded

The DX an app developer feels comes from a typed, ergonomic event union that provably tracks
the Rust enum — not from *how* the TS is produced. The 5.3 `tau-sdk-codegen` emitter is NOT
a schema-driven type generator (`emit_ts::render_package` ignores its schema arg and returns
a static template; `SchemaModel` handles only string-`enum` variants, not the
externally-tagged `oneOf` object-union shape `RunEvent` produces). Building a general
JSON-Schema→TS-union compiler is out of scope for this epic. Instead, a two-hop
drift-guarded chain gives the same guarantee:

```
tau-runtime-core: RunEvent  #[cfg_attr(feature="schema", derive(schemars::JsonSchema))]
        │ schemars (schema feature)
        ▼
schemas/run-event/run-event.v1.schema.json   ← frozen + committed
        │  ── HOP 1: schema-freeze test (schema tracks Rust) ──
        │  ── HOP 2: TS-coverage test (TS tracks schema) ──
        ▼
@tau/embed-js: RunEvent.ts   ← HAND-WRITTEN union, full autocomplete
```

- **Hop 1 — schema-freeze test:** freshly-generated schemars output == committed
  `run-event.v1.schema.json`, mirroring `crates/tau-ir/tests/schema_export.rs` (gated behind
  a `schema` feature; `UPDATE_SCHEMA=1` regenerates; pretty-print + trailing newline +
  string `assert_eq!`). Guarantees the schema can't drift from the Rust enum.
- **Hop 2 — TS-coverage test:** parses the frozen schema, extracts its variant set, and
  asserts `RunEvent.ts` covers exactly those variants (and `normalize.ts` handles each).
  Guarantees a new Rust variant cannot ship without updating the TS.

A new `RunEvent` variant therefore fails a test unless both the schema and the TS are
updated — the drift protection we wanted, without a bespoke TS compiler. Upgrade to full
generation later if more types need TS emission.

`RunEvent` stays **externally tagged**; a small (~40-line) `normalize.ts` maps serde's
`{"TextDelta":{"delta":"Hi"}}` → the ergonomic `{type:"text-delta", delta:"Hi"}`.

Adding `JsonSchema` to `RunEvent` transitively requires it on its field types
(`StopReason`, `TokenUsage`, `ToolResult` in tau-ports; `RunOutcome` in tau-runtime-core;
`serde_json::Value` is natively supported). tau-ports/tau-domain already expose `schema`
features to thread this through.

Rejected for now: flipping `RunEvent` to internally-tagged serde
(`#[serde(tag="type", rename_all="kebab-case")]`). That would make wire == schema == TS and
delete the shim, but it is a breaking change to the β.7.5 single-channel conformance
observable and the CLI trace render (#528), forcing a conformance-fixture re-baseline. Keep
the shim; flip later as its own change if desired.

### D4 — jco placement: dev-dependency of the emitted package

`tau embed --host js` emits the scaffold (`index.ts`, `normalize.ts`, `worker.ts`,
generated `RunEvent.ts`) plus a `package.json` listing **jco as a devDependency** with a
`build` script that runs `jco transpile` on the component `.wasm`. This keeps the Rust CLI
free of a Node toolchain dependency; jco stays in JS-land. tau's drift test guards the
*emitted scaffold*, not jco's transpile output (downstream and deterministic).

## Public surface

```ts
// @tau/embed-js (5.2)
export interface TauComponent { run(input: RunInput): AsyncIterable<RunEvent>; }
export function loadTau(wasm: BufferSource | URL): Promise<TauComponent>;
export function loadTauInWorker(wasm: URL): Promise<TauComponent>;

// @tau/react (5.4)
export function useTauRun(tau: TauComponent): {
  start: (input: RunInput) => void;
  events: RunEvent[]; text: string;                 // text = concatenated text-delta
  status: "idle" | "running" | "done" | "error";
  outcome: RunOutcome | null; error: { kind: string; detail: string } | null;
};

// @tau/angular (5.4)
@Injectable({ providedIn: "root" })
export class TauRunService {
  run(tau: TauComponent, input: RunInput): Observable<RunEvent>;  // completes on run-completed
  text(tau: TauComponent, input: RunInput): Observable<string>;   // scan() of text-delta
}
```

Generated `RunEvent` union (illustrative — normalized, kebab `type` tags; the emitter is
authoritative):

```ts
export type StopReason = "end-turn" | "max-tokens" | "tool-use" | "stop-sequence";
export type RunEvent =
  | { type: "run-started" }
  | { type: "context-step-ran"; step: string; tokensIn: number; tokensOut: number }
  | { type: "inference-call-started" }
  | { type: "inference-call-completed"; stopReason: StopReason; tokensIn: number; tokensOut: number }
  | { type: "text-delta"; delta: string }
  | { type: "tool-call-started"; id: string; name: string; args: unknown }
  | { type: "tool-call-completed"; id: string; name: string; result: { ok: unknown } | { err: string } }
  | { type: "turn-completed"; stopReason: StopReason; usage?: TokenUsage; turn: number }
  | { type: "run-completed"; outcome: RunOutcome }
  | { type: "fatal-error"; kind: string; detail: string; contextJson?: string };
```

## Package layout (drift-disciplined, mirrors EPIC 5.3)

```
sdk/embed-js/        # emitted by `tau embed --host js` (5.2); scaffold COMMITTED + drift-tested
  package.json       # "@tau/embed-js"; jco as devDependency; build = jco transpile
  src/
    component.ts     # jco transpile output (gitignored build artifact)
    normalize.ts     # RunEvent {"TextDelta":…} → {type:"text-delta",…} shim
    RunEvent.ts      # hand-written union; drift-guarded by TS-coverage test vs schema
    index.ts         # loadTau / loadTauInWorker
    worker.ts        # Web-Worker host
sdk/react/           # 5.4  "@tau/react"   peerDep react; dep @tau/embed-js
  src/useTauRun.ts
sdk/angular/         # 5.4  "@tau/angular" peerDep @angular/core, rxjs
  src/tau-run.service.ts
examples/streaming-demo/   # 5.4 acceptance: Vite React app renders `text` live
```

## Scope guards (explicit non-goals)

- **Single-agent only.** The guest rejects >1 agent (`guest.rs:106`); `text`/`events`
  assume one stream. Multi-agent rendering uses the separate `TraceEvent` vocab and is a
  later slice. **Do not modify the single-agent limit** — flag it as a dependency for
  multi-agent streaming and move on.
- **No pipeline-IR-in-wasm.**
- **No internally-tagged serde flip** (deferred per D3; keep the shim).

## Testing

- **Rust** — `RunEvent` schema-freeze test (hop 1): schemars output == committed
  `run-event.v1.schema.json` (mirrors `crates/tau-ir/tests/schema_export.rs`).
- **Rust** — TS-coverage test (hop 2): parse the frozen schema, assert `RunEvent.ts`
  covers exactly its variant set.
- **Rust** — `tau embed --host js` drift test: committed scaffold == fresh emit (mirrors
  5.3 `crates/tau-sdk-codegen/tests/drift.rs`).
- **Rust** — `wit_host_drift.rs` update: the new `emit-event` host import must be added to
  `HOST_PORT_REGISTRY` + param-shape assertions (ADR-0056 freeze test fails otherwise).
- **TS** — `normalize.ts` unit tests over every `RunEvent` variant, fixtures derived from
  the schema.
- **Acceptance** — the Vite demo renders a live streaming `text`.

## Sequencing

The work spans two roadmap epics; suggested PR slices (finalized in the plan):

1. **5.2-a** — `RunEvent` `JsonSchema` derive + frozen `run-event.v1.schema.json` +
   schema-freeze test. (Rust; enables the emitter.)
2. **5.2-b** — `emit-event` host import + guest emits per event (transport). (Rust/WIT.)
3. **5.2-c** — `tau embed --host js` emits `@tau/embed-js` (jco scaffold + generated
   `RunEvent.ts` + normalize + worker) + drift test. (Rust CLI + emitted TS.)
4. **5.4-a** — `@tau/react` `useTauRun`.
5. **5.4-b** — `@tau/angular` `TauRunService`.
6. **5.4-c** — `examples/streaming-demo` (acceptance).

## References

- **Living implementation tree (keep current):**
  `docs/superpowers/implementation-trees/tau-sdk-consumers.md` — the running map
  of the JS/TS consumer surface (transport → `@tau/embed-js` → consumers → demo →
  next slices). It is a **living document**: every PR that touches this surface
  must fold its status changes and discoveries back into the tree.
- Roadmap: `docs/superpowers/plans/vision-roadmap.md` EPIC 5.
- 5.3 pattern to mirror: `docs/superpowers/specs/2026-07-23-epic-5-3-sdk-codegen-design.md`,
  `docs/superpowers/plans/2026-07-23-epic-5-3-sdk-codegen.md`, `crates/tau-sdk-codegen/`.
- `RunEvent`: `crates/tau-runtime-core/src/stream.rs:129`.
- wasm build + guest + WIT: `crates/tau-cli/src/cmd/build_wasm.rs`,
  `crates/tau-wasm-guest/src/guest.rs`, `wit/tau-host.wit`.
- IR schema discipline: `schemas/ir/`.
```
