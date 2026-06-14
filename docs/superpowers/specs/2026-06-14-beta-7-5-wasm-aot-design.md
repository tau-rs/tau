# β.7.5 — IR-to-wasm AOT compiler: design spec

**Status:** Approved (brainstorm 2026-06-14)
**Sub-project:** ROADMAP Phase β · β.7.5 (split from β.7 per ADR-0040)
**Branch:** `feat/beta-7-5-wasm-aot`
**ADRs introduced:** 0046 (wasm AOT artifact + WIT world) · 0047 (in-wasm MCP facilitator)
**Supersedes prose:** the β.2 footnote "AOT lands in β.7" (already amended by ADR-0040).

---

## 1. Purpose

Add an ahead-of-time path that lowers a tau project's workflow IR +
`tau-runtime-core` + the project's native tools into a **single runnable
WASI 0.2 wasm component**. New CLI surface:

```
tau build wasm <project>
```

The artifact runs in wasmtime and reproduces the **same observable side
effects** as `tau dev` / `tau run --bundle` for the same project. This is
the *release* profile of the philosophy's "two profiles, one engine":
`tau dev` interprets on the host; `tau build wasm` lowers to a portable
artifact. `tau dev` is unchanged.

This sub-project realises philosophy wedge #2 — **a portable agent harness
(MCP host / facilitator) on wasm** — which is greenfield (portable MCP
*hosts* essentially don't exist). γ.1 extends the same artifact to Spin /
browser hosts.

## 2. Why this is its own sub-project

ADR-0040 split β.7 (the `tau dev` REPL) from β.7.5 (this) because the
in-wasm MCP-facilitator path's complexity ballooned after β.3 PR-5/PR-6.
The component-model integration and the in-wasm MCP scope each warrant
their own ADR (0046, 0047). β.7 gave us a working dev/host path
(`run_ir` + `ForwardingDispatcher`) to test the wasm profile against.

## 3. Decisions locked in brainstorming

| # | Decision | Rationale |
|---|---|---|
| D1 | **Approach B — fully-linked guest.** Native tools, context manager (β.4), and the MCP facilitator + cassette replayer all compile *into* the guest. | Philosophy: "your tools linked statically." Hits wedge #2 (MCP host in wasm). Already feasible — `tau-mcp`, the context manager, and `RunEvent` are all `no_std` in core. |
| D2 | **Observable = `ConformanceReport` (D-7a multiset)**, not a literal `RunEvent` stream. The guest returns a serialized `ConformanceReport`; the host reuses the existing `assert_conform`. | The IR interpreter (`run_ir`) never emitted `RunEvent`s — that's a `Runtime::run_streaming` (legacy) concept. D-7a is the *implemented* conformance contract. Promoting to a literal `RunEvent` stream is a **β.6** decision (the conformance *gate* lives there); recorded as a named gap, not papered over. |
| D3 | **In-guest MCP facilitator + in-guest cassette replay** for the conformance weather server. Real stdio/HTTP MCP transport is a host import, deferred to γ. | `tau-mcp` core (facilitator handlers + cassette replayer) is already `no_std`; only transport lives in `tau-mcp-tokio`. Cassette replay is pure data replay → runs in-guest deterministically with zero host imports. |
| D4 | **3 host imports only: `LlmBackend`, `Clock`, `RandomSource`.** | The exact ports the runtime-core design (2026-05-30 §"what the wasm shells need") earmarked for wasm. Principle: **host imports = inference + nondeterminism; everything else is in the guest.** Inference is *always* delegated (tau is not an inference engine), so `LlmBackend` stays a port even in wasm. |
| D5 | **`tau build wasm` = lower → bake IR → `cargo build --target wasm32-wasip2` → emit `.wasm`.** Per-project Rust *codegen* of arbitrary tools deferred to γ; β.7.5 links a fixed `no_std` tool library and bakes the IR. | "`tau build` is a front-end transform handed off to cargo + the wasm toolchain" (philosophy). The only per-project generated artifact is the baked IR bytes. |
| D6 | **Graduate one wasi triple `any-wasi-strict` Reserved→Available.** | The target registry (ADR-0034) was built for exactly this. The `wasi-*` namespace was reserved-by-absence. |
| D7 | **Toolchain: `wasm32-wasip2` (stable Tier-2) + `wit-bindgen` (`no_std`).** No `cargo-component`. Single-threaded `block_on` guest executor. | `wasm32-wasip2` emits a component directly (Rust ≥1.82); `cargo-component` is experimental and std-leaning. WASI 0.2 guests are single-threaded, so `run_ir`'s **non-Send** (RefCell) future needs no `Send` bound. |

## 4. Architecture

The discipline is **one engine, two modes** — the *same* `tau-runtime-core`
interpreter runs in both profiles. The observable boundary is the
`ToolDispatcher`/`ConformanceReport` in both: in dev it is captured by a
host-side `RecordingDispatcher`; in wasm the same recording happens
*inside the guest* and the report is returned across the component
boundary.

```
DEV PROFILE  (tau dev / run --bundle)      RELEASE PROFILE  (tau build wasm → wasmtime)
=====================================      ============================================
 tau-cli host                               tau-wasm-host  (wasmtime embed, std)
  ForwardingDispatcher                        Config: nan-canon + det. relaxed-simd
   + RecordingDispatcher → Report             supplies WIT imports ▼
        │ ToolDispatcher (Rust trait)               tau:host/llm.complete   (cassette)
        ▼                                            tau:host/clock.now      (deterministic)
 tau-runtime-core::run_ir                            tau:host/random.next    (deterministic)
   interpreter (non-Send future)                          ▲ WIT host imports
        │                                                 │
   in-proc tools / llm / mcp              ┌───────────────┴──────────────────────────┐
                                          │  GUEST  tau-wasm-guest (.wasm, no_std)     │
                                          │   baked IR bytes  (include_bytes!, AOT)    │
                                          │   tau-runtime-core::run_ir + block_on exec │
                                          │   ├─ tau-native-tools  read_temp/set_fan   │
                                          │   ├─ context manager (β.4)                 │
                                          │   ├─ tau-mcp facilitator + cassette replay │
                                          │   └─ RecordingDispatcher → ConformanceReport│
                                          │   export: run(prompt) -> report-json       │
                                          └────────────────────────────────────────────┘

PARITY GUARANTEE: both profiles run the identical `run_ir` + identical tool
code (tau-native-tools linked in both). Divergence can only enter through
the 3 host imports (made deterministic) or wasm codegen (NaN-canon +
deterministic relaxed-SIMD). assert_conform diffs the two reports.
```

### 4.1 Parity is structural, not aspirational

The strongest parity argument B affords: **dev and wasm execute the same
Rust `run_ir` and the same `tau-native-tools` implementations.** The only
code that differs is the host-import glue (3 ports) and the Cranelift
codegen of the guest. Both nondeterminism vectors are pinned (D4 + §7).
This is why `assert_conform` over `ConformanceReport`s is sufficient.

## 5. The WIT world

A single world `tau:run/runner`. The guest **imports** the three ports and
**exports** `run`.

```wit
package tau:run@0.1.0;

interface host {
    /// Delegated inference. `request-json` is a serialized
    /// tau_ports::llm::CompletionRequest; returns a serialized
    /// CompletionResponse. (Cassette-backed in conformance.)
    complete: func(request-json: string) -> result<string, string>;

    /// Monotonic-ish wall clock in milliseconds (deterministic in conformance).
    now-millis: func() -> u64;

    /// Next u64 from the host RandomSource (deterministic in conformance).
    next-u64: func() -> u64;
}

world runner {
    import host;

    /// Drive the baked IR from its entry agent with `prompt`.
    /// Returns a serialized ConformanceReport (D-7a observable) on success.
    export run: func(prompt: string) -> result<string, string>;
}
```

Notes:
- The IR is **not** a parameter — it is baked into the guest at build time
  (`include_bytes!`). `run` takes only the runtime prompt.
- JSON-string payloads at the boundary (not rich WIT records) in v1 to
  minimise `wit-bindgen` surface and keep the guest `no_std`. Richer WIT
  records are a γ refinement.
- A `tau:mcp` interface for **real** (non-cassette) transport is
  **reserved but unused** in β.7.5 — recorded in ADR-0047 as the γ.1
  expansion. Cassette replay needs no host import (it is in-guest).

## 6. Crate inventory

| Crate | std | New? | Role |
|---|---|---|---|
| `tau-wasm-guest` | `no_std`+alloc | new | The component crate. Links `tau-runtime-core` + `tau-ir` + `tau-native-tools` + `tau-mcp` (facilitator+cassette) + baked IR. `block_on(run_ir)`, records a `ConformanceReport`, exports `run`. `wit-bindgen` for imports. Own `#[global_allocator]` (dlmalloc), `#[panic_handler]`=abort, `cabi_realloc`. |
| `tau-wasm-host` | std | new | wasmtime embedder. `wasmtime::component::bindgen!` + `Linker` to satisfy `tau:host`. Cassette `LlmBackend`, deterministic `Clock`/`RandomSource`. Determinism `Config`. Instantiates + calls `run`, deserializes the report. Used by `WasmMode` conformance and a CLI smoke. |
| `tau-native-tools` | `no_std`+alloc | new | Deterministic `read_temp`(→`32`) and `set_fan`(records state) as pure fns. **Linked by both** the dev dispatcher path (conformance `DevMode`) **and** the guest — identical tool code ⇒ parity. Registered into a `ToolDispatcher`/`DeterministicRegistry`-shaped surface. |
| `wit/tau-run.wit` | — | new | The world above. Vendored; consumed by guest (`wit-bindgen::generate!`) and host (`bindgen!`). |

Changed crates: `tau-ports::target` (add `any-wasi-strict`), `tau-ir-conformance`
(add `WasmMode`), `tau-cli` (`tau build wasm` subcommand + bake/cargo-invoke).

### 6.1 Workspace / CARGO discipline

Adds 3 crates → 37 members. The wasm crates compile for a foreign target
(`wasm32-wasip2`); CI must `rustup target add wasm32-wasip2`. Per
CLAUDE.md CARGO RULES, every cargo invocation sets `CARGO_TARGET_DIR`,
`-p`, timeout, `CARGO_INCREMENTAL=0`. The guest build runs under
`tau build wasm` (shelled cargo) — it must inherit the same hygiene and
pin the target dir so it never contends with the host build lock.

## 7. Determinism

Host `wasmtime::Config`:
- `cranelift_nan_canonicalization(true)` — canonicalise all NaNs (f32/f64/v128).
- `wasm_relaxed_simd(false)` *or* `relaxed_simd_deterministic(true)`.
- Fuel-based interruption (not epoch) if any bounding is needed.
- Preallocated/limited linear memory (avoid `grow` nondeterminism).

Time + randomness are **host imports** seeded deterministically for
conformance (the existing `Clock`/`RandomSource` ports from β.1). Iteration
order is the guest's responsibility — core already uses `BTreeMap`/
`hashbrown` discipline. A dedicated byte-equal fixture (PR-6) guards
dev↔wasm float/codegen parity.

## 8. Guest executor & async model

`run_ir` is `async` and its future is **non-Send** (RefCell internals).
WASI 0.2 guests are single-threaded, so `Send` is irrelevant inside the
guest. The exported `run` is synchronous-looking to the host; internally
it drives the future to completion with a single-threaded
`block_on` (`pollster`-style or a small custom waker). No wasmtime async,
no host event loop in v1.

**p2/p3 hedge:** WASI 0.3 / Component Model Async shipped 2026-06-11 but
cross-language tooling is immature. β.7.5 targets **p2 + guest block_on**.
The executor is isolated behind a thin seam so a p3 lift (wasmtime 46 GA,
native async) is a γ follow-on without reshaping the world.

## 9. The build pipeline (`tau build wasm <project>`)

1. Load + validate the project (`ProjectConfig::parse_str`, lockfile/install
   checks) — reuse the existing `tau build` front-end.
2. Lower: `lower_project(config, any-wasi-strict, caches) → IrModule`.
   Capability-fit checks the project's required shapes against
   `any-wasi-strict`'s `{FsRead, FsWrite, NetHttp}` — a project needing
   `ProcessExec`/`AgentSpawn` is **refused at build time** (Rust-like
   build-time enforcement).
3. `to_canonical_bytes(module) → bytes`; write to a build-scratch file.
4. `include_bytes!` wiring: the guest reads the bytes file (path via build
   env or a generated `ir_bytes.rs`). Reproducible: same source ⇒ same bytes.
5. Shell `cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
   (with CARGO hygiene) → produces the `.wasm` component.
6. Emit the artifact to `-o`/default path; print its hash.

Running for the DoD is done by `tau-wasm-host` (embedded wasmtime),
exercised by `WasmMode` conformance. A user-facing `tau run --wasm
<artifact>` is a γ concern, not β.7.5.

## 10. Conformance integration

`WasmMode` is a third `ExecutionMode` next to `DevMode`/`BundleMode`:
1. Build the guest with the fixture's IR baked (or a prebuilt guest +
   runtime-loaded IR for test speed — see PR-6 open choice).
2. Run it via `tau-wasm-host`, supplying the fixture's cassette LLM +
   deterministic clock/random.
3. Deserialize the returned `ConformanceReport`.
4. `assert_conform(dev_report, wasm_report)` — reuse verbatim.

New CI lane: `conformance (wasm)` (needs the wasm target + wasmtime).
Per ADR-0039's 3-tier CI: Tier-1 PR loop runs the simplified scenario;
the broader fixture sweep can be a nightly/label lane if build time bites.

## 11. Scope — the "simplified fan-monitor"

**DoD gate (must pass under `WasmMode`):** the *simplified* fan-monitor —
one agent + `read_temp` + `set_fan` + cassette LLM. `tau build wasm`
emits a component that runs it in wasmtime and produces a
`ConformanceReport` equal to `tau dev`'s.

**In-scope conformance coverage (wedge-#2 evidence, PR-6):** fixtures
`07_mcp_weather_cassette` (in-guest MCP facilitator + cassette) and
`13_context_pipeline` (in-guest β.4 context manager) pass under `WasmMode`.
These are cheap because the code is already `no_std`.

**Out of scope (γ or later):**
- Real (non-cassette) MCP transport from inside wasm (host import reserved).
- Per-project Rust codegen of arbitrary user tools (β.7.5 links a fixed lib).
- Real tool I/O via `wasi:filesystem`/`wasi:http` (mock tools are pure).
- Literal `RunEvent` stream observable (β.6 decision).
- `tau run --wasm`, Spin/browser hosts (γ.1), WASI 0.3 async lift.
- Tree-shaking polish beyond linker GC / `wasm-metadce` smoke.

**Invariant:** every existing test stays green — all 5 bespoke plugins,
sandbox e2e, Phase-2 bundle tests, Skills, tau-workflow v1, all 13
conformance fixtures under dev+bundle.

## 12. Multi-PR plan shape (~6 PRs, 4–8 wk)

1. **Triple + CLI skeleton + ADR-0046 skeleton.** Graduate `any-wasi-strict`
   (Reserved→Available, shapes `{FsRead,FsWrite,NetHttp}`); `tau build wasm`
   parses + errors "not yet implemented"; registry/`tau target`/`tau check
   --target` tests. Small, unblocks naming.
2. **WIT world + `tau-wasm-guest` scaffold.** Builds to `wasm32-wasip2`
   with stub imports + a hardcoded trivial IR; exports `run`; CI builds the
   component + asserts the exact release profile (catch the `_rdl_*` LTO bug
   early). Proves the toolchain.
3. **`tau-wasm-host` round-trip.** wasmtime embed + `bindgen!` + `Linker`
   satisfying `tau:host` with deterministic stubs; determinism `Config`;
   instantiate + call `run` + deserialize. Proves host↔guest.
4. **IR baking.** `tau build wasm` lowers the project IR, bakes the bytes,
   shells cargo, emits the real `.wasm` driving the baked IR.
5. **Fan-monitor in-guest + ADR-0047.** `tau-native-tools` (shared
   read_temp/set_fan, used by dev too); cassette `LlmBackend` host import;
   in-guest MCP facilitator + cassette; context manager. Full simplified
   scenario runs in the guest.
6. **`WasmMode` conformance + CI.** Third `ExecutionMode`; simplified
   scenario gate; fixtures 07 + 13 under wasm; `conformance (wasm)` lane;
   byte-equal parity fixture; finalize ADRs 0046/0047 + mdBook pages.

## 13. Risks

1. **p2/p3 whiplash** — WASI 0.3 async is 3 days old; wasmtime 46 will
   default it on. Mitigation: thin executor seam, stay p2, re-eval at 46 GA.
2. **`no_std` + release-LTO toolchain bugs** (`_rdl_*` symbol emission under
   `lto=true`; `std` creeping via WASI helper crates). Mitigation: CI the
   exact release profile from PR-2; use raw `wit-bindgen`, not the `wasi`
   crate.
3. **dev↔wasm float/codegen parity** — NaN-canon covers NaNs, not float
   *formatting*. Mitigation: shared `tau-native-tools`, NaN-canon, and a
   dedicated byte-equal conformance fixture (PR-6) as the guard.
4. **Build-time cost** — shelling cargo for a wasm target per `tau build
   wasm` is slow on cold cache. Mitigation: CARGO hygiene + dedicated
   target dir; consider a prebuilt guest + runtime-loaded IR for the test
   path (PR-6 open choice).
5. **MCP facilitator no_std gaps** — `tau-mcp` is declared `no_std` but
   may have untested std-leaning corners when linked into a wasm guest.
   Mitigation: PR-5 compiles it for `wasm32-wasip2` first as a smoke.

## 14. ADRs

- **ADR-0046 — wasm AOT artifact + WIT world.** Records: Approach B,
  `wasm32-wasip2` + `wit-bindgen`, the `tau:run` world, the 3-host-import
  boundary (D4), guest `block_on` executor + p2/p3 hedge, IR baking + the
  cargo hand-off, determinism `Config`, observable = `ConformanceReport`
  with the `RunEvent`-stream-deferred-to-β.6 note (D2 tension recorded
  explicitly).
- **ADR-0047 — in-wasm MCP facilitator.** Records: facilitator runs
  in-guest (no_std `tau-mcp`); cassette replay in-guest (zero host import);
  real stdio/HTTP transport is a host import **reserved** for γ.1; the
  reserved `tau:mcp` WIT slot. This is the ADR the ROADMAP β.7.5 note
  demanded.

## 15. Open questions for the plan

- PR-6 test speed: bake IR per fixture (true AOT, slow) vs a prebuilt guest
  that loads IR at runtime (fast, slightly less faithful). Decide in
  writing-plans; the DoD `tau build wasm` path must bake regardless.
- Exact `LlmBackend` (de)serialization at the WIT boundary — reuse
  `tau_ports::llm` serde shapes; confirm they're `no_std`-serializable.
- Whether `tau-native-tools` registers via `ToolDispatcher` or
  `DeterministicRegistry` (the fan-monitor tools are effectively
  deterministic) — align with how fixture 01 wires them today.
