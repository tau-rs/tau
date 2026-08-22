# EPIC 7.2 — Variant A wasm-guest embedding API + example (design)

Issue #414. Second EPIC 7 story, paired with 7.1 (#413, PR #624): 7.1 lets a
Rust product *link* tau natively (Variant B); 7.2 lets any product *load and
run the built wasm component* (Variant A). Acceptance per the roadmap:
example loads + runs.

## Problem

`tau build --target wasm` produces a WASI 0.2 component (guest + baked IR +
capability world), but no product can run it:

1. **The only existing host is conformance-only.** `tau-wasm-host` exists to
   prove determinism (β.6 `WasmProfile`): its four `tau:host/runner` imports
   are hardcoded fakes — `complete` pops a canned cassette queue, `now-millis`
   is a fixed-step counter, `next-u64` is a seeded SplitMix64, and
   `emit-event` buffers into a `Vec` returned only after the run ends.
2. **No injection seam.** `run_component(bytes, prompt, canned_responses)` /
   `run_component_with_caps(..)` expose no parameter through which a caller
   can supply live inference, a real clock, real entropy, or a live event
   consumer. A product wanting sandboxed workflows-as-data (the Variant A
   value: capability enforcement at the wasm boundary, workflow updates
   without recompiling the product, polyglot hosts) has nothing to call.
3. **The host contract is unproven live.** The 4-import WIT world has only
   ever been exercised by deterministic fakes; whether it suffices for a
   live embedder (sync `complete` vs async product LLM clients, per-event
   streaming latency) has never been demonstrated.

## Approaches considered

- **A. Port-trait seam in `tau-wasm-host`** (chosen): define an `EmbedPorts`
  trait naming the four host functions; the deterministic fakes become one
  private impl of it, so the conformance API keeps its exact behavior on top
  of the new seam. One wasmtime/WASI/egress stack, one WIT binding, one
  documented contract — the contract future hosts (7.3 WAMR) re-implement
  and future language packages (EPIC 5 lane) wrap.
- **B. New product-host crate, `tau-wasm-host` untouched.** Rejected:
  duplicates the linker/WASI-caps/egress/bindgen stack (~250 lines); two
  copies of the WIT binding drift independently — the single-source rule the
  guest codebase already fights for (cf. tau-native-tools, ADR-0054).
- **C. Closure bag** (`RunConfig` of four boxed `FnMut`s). Functionally
  identical to A; rejected for idiom — tau expresses "things the embedder
  supplies" as port traits (`ToolDispatcher`, `Clock`, `RandomSource`,
  7.1's prelude), and a named trait is a documentable contract four
  anonymous closures are not.

## Design (approach A)

### 1. `EmbedPorts` trait (tau-wasm-host)

```rust
/// The four host functions a product supplies to run a tau component —
/// the Rust face of the `tau:host/runner` WIT imports.
pub trait EmbedPorts: Send {
    /// Live inference. Typed: the host layer (de)serializes across the
    /// WIT boundary; the `Err` string crosses into the guest's `result`
    /// error arm. The WIT import is synchronous — an async product client
    /// blocks inside its impl (e.g. `block_on`).
    fn complete(&mut self, req: CompletionRequest) -> Result<CompletionResponse, String>;
    /// Wall clock in milliseconds.
    fn now_millis(&mut self) -> u64;
    /// Next value from the product's entropy source.
    fn next_u64(&mut self) -> u64;
    /// One serialized `RunEvent` per call, in order, live as the guest
    /// emits it. Raw JSON so tau-wasm-host gains no tau-runtime-core
    /// dependency; products wanting typed events deserialize themselves
    /// (the example demonstrates this).
    fn on_event(&mut self, event_json: &str);
}
```

Methods take `&mut self` (matches wasmtime bindgen host traits; ports live
in the `Store` for the duration of one run).

### 2. `run_component_with_ports` + `HostState` refactor

```rust
pub fn run_component_with_ports(
    wasm_bytes: &[u8],
    prompt: &str,
    ports: &mut dyn EmbedPorts,
    caps: &[tau_domain::Capability],
    sandbox_root: &Path,
) -> Result<String, WasmHostError>
```

- `HostState` replaces its hardcoded `responses`/`clock_millis`/
  `prng_state`/`emitted` fields with the ports handle; `host::Host for
  HostState` delegates the four imports to it. `complete` does
  `serde_json` request-parse / response-serialize around the trait call
  (a malformed request from the guest is a host-side error string, not a
  panic). WASI caps / egress wiring is unchanged and shared.
- The deterministic behavior moves into a private `DeterministicPorts`
  (cassette `VecDeque`, `CLOCK_STEP_MILLIS` counter, `PRNG_SEED`
  SplitMix64, event buffer). `run_component` /
  `run_component_with_caps` keep their exact public signatures and
  return `(payload, emitted)` by draining the buffer — behavior must
  stay byte-identical (the existing conformance/e2e tests are the
  guard; none of them change).
- Engine config stays `determinism_config()` (NaN canonicalization
  etc.) for both paths — harmless for live hosts, and keeps one config.
- The `Ok` payload remains the guest's sentinel string (empty today) —
  returned as-is for ABI honesty and forward-compat.

### 3. `tau_wasm_host::embed` prelude

`pub mod embed` mirroring 7.1's `tau_runtime_core::embed`: pure
re-exports pinning the supported Variant A surface —
`run_component_with_ports`, `EmbedPorts`, `WasmHostError`,
`tau_ports::{CompletionRequest, CompletionResponse, StopReason}`,
`tau_domain::Capability`. Module doc is the embedding API documentation,
mirrored by the docs how-to. The conformance entrypoints
(`run_component*`) stay at the crate root — they are not part of the
embedding surface.

### 4. Example product: `crates/tau-wasm-embed-example`

A ~150-line binary, the Variant A twin of `tau-embed-example`:

```
$ tau build --target wasm -o product.wasm      # in any governed project
$ cargo run -p tau-wasm-embed-example -- product.wasm "hello"
{"type":"RunStarted",...}      ← printed live, one line per event
...
run completed: <n> events
```

- Args: `<component.wasm> [prompt]` (no baked fixture — the point of
  Variant A is workflow-as-data, so the workflow arrives as a file).
- `ProductPorts: EmbedPorts`: echo LLM (canned `CompletionResponse`, as
  7.1's `EchoBackend` — a real product calls its inference service),
  `SystemTime` wall clock, clock-seeded xorshift entropy (documented:
  real products supply OS entropy), `on_event` parses each line into a
  typed `tau_runtime_core::stream::RunEvent` (proving the typed
  contract via the product's own dep) and prints the raw JSON.
- `#![forbid(unsafe_code)]`, no tokio (sync throughout).

### 5. Acceptance gate

- `crates/tau-cli/tests/embed_wasm_e2e.rs` (twin of 7.1's
  `embed_rust_e2e.rs`): reuse `build_wasm_e2e.rs`'s helper to lower the
  existing `tests/fixtures/wasm-build/trivial` project and build the
  guest for `wasm32-wasip2`, then `cargo run -p tau-wasm-embed-example
  -- <artifact> "prompt"` and assert exit 0 + a `RunCompleted` event
  line on stdout. This *is* the roadmap acceptance ("example loads +
  runs"). Same tier/skip conventions as the existing wasm e2e (requires
  `wasm32-wasip2`).
- Unit: a `tests/` case in tau-wasm-host driving
  `run_component_with_ports` with a recording `EmbedPorts` impl against
  the existing test component fixture path used by `roundtrip.rs`,
  asserting live event delivery order matches the buffered API's output.

### 6. Docs

`docs/how-to/embed-wasm-component.md` mirroring `embed-rust-native.md`
(when to choose Variant A vs B, the four ports, worked example walk-
through), plus its `SUMMARY.md` line. Book built locally before the PR
per DOCS RULES.

## Out of scope (follow-ups filed at PR time)

- **On-shelf language packages** ("`npm install @tau/runtime`"): idiomatic
  JS/Python wrappers around this contract + publishing infra — EPIC 5
  lane follow-up issue.
- `tau embed --host wasm` codegen template (would scaffold an
  `EmbedPorts` impl; needs this API stable first).
- Async WIT `complete` (ABI change; needs guest + world bump).
- WAMR/MCU host (7.3, gated).

## Dependency

Builds on main *after* PR #624 (EPIC 7.1) merges — shared naming
(`embed` prelude convention), docs cross-links, and the example crate
pattern. No code dependency on 7.1's prelude itself (Variant A never
links tau-runtime-core into the host path; only the example's typed
`RunEvent` parse uses it, as any product would).
