# β.7.5 — IR-to-wasm AOT compiler: design spec

**Status:** Approved (brainstorm 2026-06-14; amended 2026-06-16 — see "Amendments")
**Sub-project:** ROADMAP Phase β · β.7.5 (split from β.7 per ADR-0040)
**Branch:** `feat/beta-7-5-wasm-aot`
**ADRs introduced:** 0046 (wasm AOT artifact + WIT world) · 0049 (single-channel
typed conformance observable — supersedes ADR-0048 Decision 1). The in-wasm
MCP-facilitator ADR shifts to **0050** (0047 was taken by the β.5 credential
provider chain; 0049 is this amendment's conformance decision).
**Supersedes prose:** the β.2 footnote "AOT lands in β.7" (already amended by ADR-0040).

---

## Amendments (2026-06-16)

Two decisions locked in follow-up brainstorming after recon against the
post-β.6 tree. Both close design holes the original brainstorm assumed away.

- **Amendment 1 — `SkillResolver` port (Decision 1).** The original spec
  assumed `tau-runtime-core` was already `no_std`/wasm-ready. Recon proved it
  is **not**: an unconditional `tau-pkg` dependency (for skill resolution)
  pulls `tokio` + `rustix`, so core cannot cross-compile to `wasm32-wasip2`.
  Fix — finish the β.1 port pattern that skipped skill resolution: a new
  `tau-ports::SkillResolver` port, host/guest adapters, and **drop `tau-pkg`
  from core entirely**. See §4.2 and §6.2.
- **Amendment 2 — single-channel typed conformance observable (Decision 2 =
  ADR-0049, supersedes ADR-0048 Decision 1).** β.6's `ConformanceEvent`
  stream merges two sources (typed `RunEvent` from `run_ir_streaming` + a std
  `tracing` `Captor` that string-parses log lines via `map_tracing`). A
  `no_std` guest cannot run the std subscriber. Fix — promote the 4
  tracing-only gate events to **typed `RunEvent` variants**; conformance then
  sources **only** from `run_ir_streaming`. Tracing events stay for logging
  (no logging regression). See §5, §10, §14.

These supersede the in-table assumptions in D1 and D2 (annotated inline below).

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
| D1 | **Approach B — fully-linked guest.** Native tools, context manager (β.4), and the MCP facilitator + cassette replayer all compile *into* the guest. | Philosophy: "your tools linked statically." Hits wedge #2 (MCP host in wasm). Already feasible — `tau-mcp`, the context manager, and `RunEvent` are all `no_std` in core. **(Amended 2026-06-16 — Decision 1:** core is *not* yet wasm-ready: its unconditional `tau-pkg` dep blocks `wasm32-wasip2`. The `SkillResolver` port in §4.2/§6.2 closes this. The other `no_std` claims hold.) |
| D2 | **Observable = `ConformanceReport` (D-7a multiset)**, not a literal `RunEvent` stream. The guest returns a serialized `ConformanceReport`; the host reuses the existing `assert_conform`. | The IR interpreter (`run_ir`) never emitted `RunEvent`s — that's a `Runtime::run_streaming` (legacy) concept. D-7a is the *implemented* conformance contract. Promoting to a literal `RunEvent` stream is a **β.6** decision (the conformance *gate* lives there); recorded as a named gap, not papered over. **(Amended 2026-06-16 — Decision 2 / ADR-0049:** β.6 promoted the gate to a typed `RunEvent` stream sourced single-channel from `run_ir_streaming`; the guest now exports that typed stream — see §5/§10. The D-7a multiset `ConformanceReport` remains the cross-*mode* `tau-ir-conformance` observable.) |
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
This is why `assert_conform` over the typed `RunEvent` stream (§5/§10) is
sufficient.

### 4.2 The `no_std` boundary — `SkillResolver` port (Decision 1)

The original brainstorm assumed `tau-runtime-core` was already wasm-ready.
Recon falsified that: core carries an **unconditional `tau-pkg` dependency**
used only for skill resolution, and `tau-pkg` is std-only (it pulls `tokio`
and `rustix`). Core therefore **cannot cross-compile to `wasm32-wasip2`** as
it stands — the fully-linked guest (D1) is impossible until that edge is cut.

The cut is the β.1 port pattern, finished. β.1 already moved override-aware
capability resolution behind a port (`CapabilityResolver` in
`crates/tau-ports/src/capability_resolver.rs`) precisely so core could drive
the agent loop without linking `tau-pkg`. Skill resolution was the one
consumer that β.1 skipped, and it is the sole remaining `tau-pkg` edge. We
finish the pattern:

- **New port** `tau-ports::SkillResolver` — mirrors `CapabilityResolver`
  (trait: `Send + Sync`):

  ```rust
  pub trait SkillResolver: Send + Sync {
      fn resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError>;
  }
  ```

- **`RunOptions.skill_resolver: Option<Arc<dyn tau_ports::SkillResolver>>`** —
  the host shell stuffs an impl in; a shell with no skill layer omits it and
  core falls back (same shape as the optional `capability_resolver`).
- **Adapters.** Host: `TauPkgSkillResolver` lives in `tau-runtime-tokio`,
  wrapping `tau_pkg::{find_installed_skill, Scope}`. Guest: `NoSkillResolver`
  (rejects/empties) and `BakedSkillResolver` (resolves against skills baked
  into the artifact) — both `no_std`, zero `tau-pkg`.
- **`tau-runtime-core` drops its `tau-pkg` dependency entirely.** It depends
  on the *trait* only. Dependencies point inward: engine → port; the
  std-heavy `tau-pkg` adapter sits in the host crate, outside the wasm cut.

After this, the D1 fully-linked guest is actually buildable: core +
`tau-ir` + `tau-native-tools` + `tau-mcp` all compile `no_std`, and the only
std crates left (`tau-pkg`, `tau-runtime-tokio`) live entirely on the host
side of the WIT boundary.

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
    /// Returns the serialized **typed `RunEvent` stream** emitted by
    /// `run_ir_streaming` (ADR-0049 single-channel observable) on success:
    /// a JSON array of `RunEvent`s in causal order. β.6's conformance gate
    /// maps this directly to `ConformanceEvent` — no host-side tracing
    /// subscriber, no string-parsing.
    export run: func(prompt: string) -> result<string, string>;
}
```

Notes:
- The IR is **not** a parameter — it is baked into the guest at build time
  (`include_bytes!`). `run` takes only the runtime prompt.
- **Single-channel typed observable (Decision 2 / ADR-0049).** The guest
  drives `run_ir_streaming` and serializes the typed `RunEvent` stream it
  yields. That stream now carries the 4 events that were previously
  *tracing-only* in β.6's dual-channel design — `RunStarted`,
  `ContextStepRan { step, tokens_in, tokens_out }`, `InferenceCallStarted`,
  `InferenceCallCompleted { stop_reason, tokens_in, tokens_out }` — so a
  `no_std` guest needs **no** std `tracing` subscriber to produce a complete
  conformance stream. Tracing events still fire for logging; conformance
  just stops re-parsing them.
- JSON-string payloads at the boundary (not rich WIT records) in v1 to
  minimise `wit-bindgen` surface and keep the guest `no_std`. Richer WIT
  records are a γ refinement.
- A `tau:mcp` interface for **real** (non-cassette) transport is
  **reserved but unused** in β.7.5 — recorded in ADR-0050 (the in-wasm
  MCP-facilitator ADR, renumbered from 0047) as the γ.1 expansion. Cassette
  replay needs no host import (it is in-guest).

## 6. Crate inventory

| Crate | std | New? | Role |
|---|---|---|---|
| `tau-wasm-guest` | `no_std`+alloc | new | The component crate. Links `tau-runtime-core` + `tau-ir` + `tau-native-tools` + `tau-mcp` (facilitator+cassette) + baked IR. `block_on(run_ir)`, records a `ConformanceReport`, exports `run`. `wit-bindgen` for imports. Own `#[global_allocator]` (dlmalloc), `#[panic_handler]`=abort, `cabi_realloc`. |
| `tau-wasm-host` | std | new | wasmtime embedder. `wasmtime::component::bindgen!` + `Linker` to satisfy `tau:host`. Cassette `LlmBackend`, deterministic `Clock`/`RandomSource`. Determinism `Config`. Instantiates + calls `run`, deserializes the report. Used by `WasmMode` conformance and a CLI smoke. |
| `tau-native-tools` | `no_std`+alloc | new | Deterministic `read_temp`(→`32`) and `set_fan`(records state) as pure fns. **Linked by both** the dev dispatcher path (conformance `DevMode`) **and** the guest — identical tool code ⇒ parity. Registered into a `ToolDispatcher`/`DeterministicRegistry`-shaped surface. |
| `wit/tau-run.wit` | — | new | The world above. Vendored; consumed by guest (`wit-bindgen::generate!`) and host (`bindgen!`). |

Changed crates: `tau-ports` (add `any-wasi-strict` target + the new
`SkillResolver` port — §6.2), `tau-runtime-core` (**drop `tau-pkg` dep**;
add `RunOptions.skill_resolver` + the 4 new typed `RunEvent` variants),
`tau-runtime-tokio` (add `TauPkgSkillResolver` host adapter),
`tau-conformance` (single-channel sourcing — §10), `tau-ir-conformance`
(add `WasmMode`), `tau-cli` (`tau build wasm` subcommand + bake/cargo-invoke).

### 6.1 Workspace / CARGO discipline

Adds 3 crates → 37 members. The wasm crates compile for a foreign target
(`wasm32-wasip2`); CI must `rustup target add wasm32-wasip2`. Per
CLAUDE.md CARGO RULES, every cargo invocation sets `CARGO_TARGET_DIR`,
`-p`, timeout, `CARGO_INCREMENTAL=0`. The guest build runs under
`tau build wasm` (shelled cargo) — it must inherit the same hygiene and
pin the target dir so it never contends with the host build lock.

### 6.2 `SkillResolver` port + adapters (Decision 1)

The wasm cut from §4.2 lands as one new port and three adapters:

| Item | Crate | std | Role |
|---|---|---|---|
| `SkillResolver` trait + `ResolvedSkill` + `SkillResolveError` | `tau-ports` | `no_std` | The port. Mirrors `CapabilityResolver` (`Send + Sync`, `fn resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError>`). |
| `RunOptions.skill_resolver: Option<Arc<dyn SkillResolver>>` | `tau-runtime-core` | `no_std` | Optional injection point; absent ⇒ core falls back (no skill layer). Same shape as `capability_resolver`. |
| `TauPkgSkillResolver` | `tau-runtime-tokio` | std | Host adapter wrapping `tau_pkg::{find_installed_skill, Scope}`. The std-heavy `tau-pkg` link lives **here**, outside the wasm cut. |
| `NoSkillResolver`, `BakedSkillResolver` | guest (`tau-wasm-guest`) | `no_std` | `NoSkillResolver` = empty/reject; `BakedSkillResolver` = resolve against skills baked into the artifact. Zero `tau-pkg`. |

**`tau-runtime-core` drops its `tau-pkg` dependency entirely** — it links the
trait, never the adapter. This is the single edge that makes the D1
fully-linked guest cross-compile to `wasm32-wasip2`. Dependencies point
inward; the std adapter sits in the host crate.

## 7. Determinism

Host `wasmtime::Config`:
- `cranelift_nan_canonicalization(true)` — canonicalise all NaNs (f32/f64/v128).
- `wasm_relaxed_simd(false)` *or* `relaxed_simd_deterministic(true)`.
- Fuel-based interruption (not epoch) if any bounding is needed.
- Preallocated/limited linear memory (avoid `grow` nondeterminism).

Time + randomness are **host imports** seeded deterministically for
conformance (the existing `Clock`/`RandomSource` ports from β.1). Iteration
order is the guest's responsibility — core already uses `BTreeMap`/
`hashbrown` discipline. A dedicated byte-equal fixture (PR-G) guards
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

Two conformance axes touch β.7.5; Decision 2 / ADR-0049 reshapes how the
second one sources its stream.

**Cross-*mode* multiset (`tau-ir-conformance`).** `WasmMode` is a third
`ExecutionMode` next to `DevMode`/`BundleMode`. Build the guest, run it via
`tau-wasm-host` with the fixture's cassette LLM + deterministic clock/random,
deserialize the result, and `assert_conform` over the D-7a multiset
(unchanged). This is the cheap "did the same tool calls happen" check.

**Cross-*profile* ordered stream (`tau-conformance`, the β.6 gate — the
capstone).** This is where ADR-0049 lands. β.6 today (ADR-0048 Decision 1)
sources its `ConformanceEvent` stream from **two** channels: typed
`RunEvent`s from `run_ir_streaming`, plus a std `tracing` `Captor` that
string-parses log lines via `map_tracing`, interleaved at the generator's
yield barrier. A `no_std` wasm guest **cannot run that std subscriber**, so
the wasm profile could never reproduce the dual-channel stream.

ADR-0049 promotes the 4 tracing-only gate events — `RunStarted`,
`ContextStepRan`, `InferenceCallStarted`, `InferenceCallCompleted` — to
**typed `RunEvent` variants**. After that:

1. The wasm guest drives `run_ir_streaming` and exports the serialized typed
   stream via the WIT `run` export (§5) — no host subscriber needed.
2. Conformance maps `ConformanceEvent` from **`run_ir_streaming` alone**
   (single channel), in **both** the dev and wasm profiles.
3. The dev profile's `Captor` + `map_tracing` + dual-channel interleave are
   **deleted**. Tracing events stay for logging (no logging regression);
   conformance simply stops re-parsing them.
4. The **frozen `ConformanceEvent` output contract and golden file are
   unchanged** — only the *sourcing* changes, so the existing β.6 golden and
   the `dev == golden` assertion keep passing.

This also retires the `#[ignore]`'d `fan_monitor_dev_matches_wasm` assertion:
once the guest emits the typed stream, the `dev == wasm` arm of `tau-conformance`
goes live (PR-G).

New CI lane: `conformance (wasm)` (needs the wasm target + wasmtime).
Per ADR-0039's 3-tier CI: Tier-1 PR loop runs the simplified scenario;
the broader fixture sweep can be a nightly/label lane if build time bites.

## 11. Scope — the "simplified fan-monitor"

**DoD gate (must pass under `WasmMode`):** the *simplified* fan-monitor —
one agent + `read_temp` + `set_fan` + cassette LLM. `tau build wasm`
emits a component that runs it in wasmtime and produces a
`ConformanceReport` equal to `tau dev`'s.

**In-scope conformance coverage (wedge-#2 evidence, PR-G):** fixtures
`07_mcp_weather_cassette` (in-guest MCP facilitator + cassette) and
`13_context_pipeline` (in-guest β.4 context manager) pass under `WasmMode`.
These are cheap because the code is already `no_std`.

**Out of scope (γ or later):**
- Real (non-cassette) MCP transport from inside wasm (host import reserved).
- Per-project Rust codegen of arbitrary user tools (β.7.5 links a fixed lib).
- Real tool I/O via `wasi:filesystem`/`wasi:http` (mock tools are pure).
- `tau run --wasm`, Spin/browser hosts (γ.1), WASI 0.3 async lift.
- Tree-shaking polish beyond linker GC / `wasm-metadce` smoke.

**Invariant:** every existing test stays green — all 5 bespoke plugins,
sandbox e2e, Phase-2 bundle tests, Skills, tau-workflow v1, all 13
conformance fixtures under dev+bundle.

## 12. Multi-PR plan shape (revised 2026-06-16 — Decision 3)

The two amendments add two **prerequisite** PRs (A, B) ahead of the original
scaffold work. Original **PR-1** (`any-wasi-strict` triple + `tau build wasm`
skeleton + ADR-0046 skeleton) **already shipped** (#350); the rest renumber
to PR-C…PR-G so the new sequence reads A/B/C–G.

- **PR-A — `SkillResolver` port (Decision 1).** New `tau-ports::SkillResolver`
  + `ResolvedSkill`/`SkillResolveError`; `RunOptions.skill_resolver`;
  `TauPkgSkillResolver` host adapter in `tau-runtime-tokio`;
  `NoSkillResolver`/`BakedSkillResolver` guest adapters. **Drop `tau-pkg` from
  `tau-runtime-core`.** DoD: core compiles for `wasm32-wasip2` (CI smoke);
  host behavior unchanged. *Unblocks the fully-linked guest (D1).*
- **PR-B — typed events + single-channel conformance (Decision 2 / ADR-0049).**
  Promote `RunStarted`, `ContextStepRan`, `InferenceCallStarted`,
  `InferenceCallCompleted` to typed `RunEvent` variants emitted by
  `run_ir_streaming`; source `tau-conformance`'s `ConformanceEvent` from that
  single channel; delete the `Captor` + `map_tracing` + dual interleave.
  Tracing events stay for logging. DoD: `dev == golden` still passes against
  the unchanged golden; no logging regression.
- **PR-C — WIT world + `tau-wasm-guest` scaffold.** Builds to `wasm32-wasip2`
  with stub imports + a hardcoded trivial IR; exports `run` returning the
  typed stream; CI builds the component + asserts the exact release profile
  (catch the `_rdl_*` LTO bug early). Proves the toolchain. *(was PR-2)*
- **PR-D — `tau-wasm-host` round-trip.** wasmtime embed + `bindgen!` +
  `Linker` satisfying `tau:host` with deterministic stubs; determinism
  `Config`; instantiate + call `run` + deserialize. Proves host↔guest.
  *(was PR-3)*
- **PR-E — IR baking.** `tau build wasm` lowers the project IR, bakes the
  bytes, shells cargo, emits the real `.wasm` driving the baked IR. *(was
  PR-4)*
- **PR-F — fan-monitor in-guest + ADR-0050.** `tau-native-tools` (shared
  read_temp/set_fan, used by dev too); cassette `LlmBackend` host import;
  in-guest MCP facilitator + cassette; context manager. Full simplified
  scenario runs in the guest. (ADR-0050 = the in-wasm MCP facilitator ADR,
  renumbered from 0047.) *(was PR-5)*
- **PR-G — `WasmMode` + the β.6/Phase-β capstone.** Third `ExecutionMode`;
  simplified scenario gate; fixtures 07 + 13 under wasm; `conformance (wasm)`
  lane; byte-equal parity fixture; **flip the `#[ignore]`'d
  `fan_monitor_dev_matches_wasm` live** (the `dev == wasm` arm — the β.6 /
  Phase-β capstone); finalize ADRs 0046/0050 + mdBook pages. *(was PR-6)*

## 13. Risks

1. **p2/p3 whiplash** — WASI 0.3 async is 3 days old; wasmtime 46 will
   default it on. Mitigation: thin executor seam, stay p2, re-eval at 46 GA.
2. **`no_std` + release-LTO toolchain bugs** (`_rdl_*` symbol emission under
   `lto=true`; `std` creeping via WASI helper crates). Mitigation: CI the
   exact release profile from PR-C; use raw `wit-bindgen`, not the `wasi`
   crate.
3. **dev↔wasm float/codegen parity** — NaN-canon covers NaNs, not float
   *formatting*. Mitigation: shared `tau-native-tools`, NaN-canon, and a
   dedicated byte-equal conformance fixture (PR-G) as the guard.
4. **Build-time cost** — shelling cargo for a wasm target per `tau build
   wasm` is slow on cold cache. Mitigation: CARGO hygiene + dedicated
   target dir; consider a prebuilt guest + runtime-loaded IR for the test
   path (PR-G open choice).
5. **MCP facilitator no_std gaps** — `tau-mcp` is declared `no_std` but
   may have untested std-leaning corners when linked into a wasm guest.
   Mitigation: PR-F compiles it for `wasm32-wasip2` first as a smoke.

## 14. ADRs

> **ADR numbering (amended 2026-06-16).** 0046 = wasm AOT artifact + WIT
> world. 0047 was claimed by the β.5 **credential provider chain** (#351),
> so the in-wasm MCP-facilitator ADR shifts to **0050**. 0048 = β.6
> cross-target conformance gate (dual-channel). **0049** (this amendment) =
> single-channel typed conformance observable, **supersedes ADR-0048
> Decision 1**.

- **ADR-0046 — wasm AOT artifact + WIT world.** Records: Approach B,
  `wasm32-wasip2` + `wit-bindgen`, the `tau:run` world, the 3-host-import
  boundary (D4), guest `block_on` executor + p2/p3 hedge, IR baking + the
  cargo hand-off, determinism `Config`. **The observable is now the typed
  `RunEvent` stream (ADR-0049), not the D-7a `ConformanceReport`** — the old
  "RunEvent-stream-deferred-to-β.6" note is closed by ADR-0049.
- **ADR-0049 — single-channel typed conformance observable.** *Status:
  Accepted (2026-06-16). Supersedes ADR-0048 Decision 1 (dual-channel
  sourcing).* Records: promote the 4 tracing-only gate events to typed
  `RunEvent` variants; conformance sources only from `run_ir_streaming`;
  delete the dev `Captor` + `map_tracing` + dual interleave; tracing events
  stay for logging; the frozen `ConformanceEvent` contract + golden are
  unchanged. See `docs/decisions/0049-single-channel-typed-conformance-observable.md`.
- **ADR-0050 — in-wasm MCP facilitator** *(renumbered from 0047; forthcoming,
  PR-F).* Records: facilitator runs in-guest (no_std `tau-mcp`); cassette
  replay in-guest (zero host import); real stdio/HTTP transport is a host
  import **reserved** for γ.1; the reserved `tau:mcp` WIT slot. This is the
  ADR the ROADMAP β.7.5 note demanded.

## 15. Open questions for the plan

- PR-G test speed: bake IR per fixture (true AOT, slow) vs a prebuilt guest
  that loads IR at runtime (fast, slightly less faithful). Decide in
  writing-plans; the DoD `tau build wasm` path must bake regardless.
- Exact `LlmBackend` (de)serialization at the WIT boundary — reuse
  `tau_ports::llm` serde shapes; confirm they're `no_std`-serializable.
- Whether `tau-native-tools` registers via `ToolDispatcher` or
  `DeterministicRegistry` (the fan-monitor tools are effectively
  deterministic) — align with how fixture 01 wires them today.
