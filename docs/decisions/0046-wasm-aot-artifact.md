# ADR-0046: Wasm AOT artifact + WIT world (β.7.5)

**Status:** Proposed
**Date:** 2026-06-14
**Supersedes:** none

## Context

β.7.5 extends tau's "one engine, two modes" architecture with an
**ahead-of-time path**: `tau build wasm <project>` lowers the project's
workflow IR + `tau-runtime-core` + native tools into a single runnable
**WASI 0.2 wasm component**. The artifact runs in wasmtime and reproduces
the same observable side-effects as `tau dev` / `tau run --bundle`.

Two design forces shape every decision here:

1. **Philosophy: "your tools linked statically."** The wasm artifact must be
   self-contained — host imports are inference + nondeterminism only. Everything
   else (context manager, MCP facilitator, cassette replay) must compile _into_
   the guest.

2. **The IR interpreter (`run_ir`) never emitted `RunEvent`s.** That is a
   `Runtime::run_streaming` (legacy) concept. The _implemented_ conformance
   contract is `ConformanceReport` (D-7a multiset). Promoting the observable
   to a literal `RunEvent` stream is a **β.6** decision (the conformance gate
   lives there); this ADR records that tension explicitly rather than papering
   over it.

ADR-0040 split β.7 (the `tau dev` REPL) from β.7.5 (this) because the
in-wasm MCP-facilitator path ballooned after β.3 PR-5/PR-6. ADR-0047
(in-wasm MCP facilitator, forthcoming in Phase 5) records the facilitator
specifics; this ADR records the artifact shape, toolchain, and conformance
boundary.

Full design detail is in
`docs/superpowers/specs/2026-06-14-beta-7-5-wasm-aot-design.md`.

## Decisions

### Decision 1 — Approach B: fully-linked guest

Native tools, the context manager (β.4), the MCP facilitator, and the
cassette replayer all compile **into** the wasm guest. There is no thin-guest
option where host imports supply tool logic.

Rationale: the philosophy's "harness everywhere with statically linked tools"
wedge; it hits portability wedge #2 (MCP host in wasm). This is feasible
because `tau-mcp`, the context manager, and `tau-runtime-core` are all `no_std`
in core. Alternative A (thin guest, tool logic in host imports) is rejected
because it defeats the portability goal and collapses back to a different
remotely-hosted architecture with no wasm-isolation benefit.

### Decision 2 — Observable = `ConformanceReport` (D-7a multiset)

The guest exports a serialized `ConformanceReport`; the host reuses the
existing `assert_conform` to compare dev↔wasm. The observable is **not** a
literal `RunEvent` stream.

Rationale: `run_ir` never emitted `RunEvents` — promoting to a literal
`RunEvent` stream is a β.6 decision. Recording this as an explicit named gap
is correct; papering over it with a new internal stream just for wasm would
be speculative scope. The `ConformanceReport` D-7a multiset is the implemented
conformance contract and is sufficient to enforce dev↔wasm parity.

### Decision 3 — Toolchain: `wasm32-wasip2` + `wit-bindgen` (`no_std`), no `cargo-component`

Rust ≥ 1.82 emits a wasm component directly from a `wasm32-wasip2` crate; no
`cargo-component` wrapper is needed. `wit-bindgen` is used for guest binding
generation (supports `no_std`). `cargo-component` is experimental and std-leaning
— rejected.

### Decision 4 — The `tau:run` WIT world

> **Amended 2026-06-22:** the package was renamed `tau:run` → `tau:host` to match
> ADR-0056 (the embedding contract is `tau:host`). The historical text below is
> preserved; see `docs/reference/wit-host-world.md` for the current contract.

A single world `tau:run/runner`:

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

**Principle: host imports = inference + nondeterminism only; everything else
is in-guest.** The IR is **not** a parameter — it is baked into the guest at
build time (`include_bytes!`). JSON-string payloads at the boundary (not rich
WIT records) in v1 to minimise `wit-bindgen` surface and keep the guest
`no_std`. Richer WIT records are a γ refinement.

A `tau:mcp` interface for real (non-cassette) MCP transport is **reserved but
unused** in β.7.5; it is recorded in ADR-0047 as the γ.1 expansion.

### Decision 5 — Guest executor: single-threaded `block_on` + p2/p3 hedge

`run_ir` is `async` and its future is **non-Send** (RefCell internals). WASI 0.2
guests are single-threaded, so `Send` is irrelevant inside the guest. The
exported `run` is synchronous-looking to the host; internally it drives the
future to completion with a single-threaded `block_on` (`pollster`-style or a
small custom waker). No wasmtime async, no host event loop in v1.

**p2/p3 hedge:** WASI 0.3 / Component Model Async shipped 2026-06-11 but
cross-language tooling is immature. β.7.5 targets p2 + guest `block_on`. The
executor is isolated behind a thin seam so a p3 lift (wasmtime 46 GA, native
async) is a γ follow-on without reshaping the WIT world.

### Decision 6 — IR baking + cargo hand-off

`tau build wasm <project>` pipeline:

1. Load + validate the project (reuse existing `tau build` front-end).
2. Lower: `lower_project(config, any-wasi-strict, caches) → IrModule`.
   Capability-fit checks the project's required shapes against
   `any-wasi-strict`'s `{FsRead, FsWrite, NetHttp}` — a project needing
   `ProcessExec` / `AgentSpawn` is **refused at build time** (Rust-like
   build-time enforcement).
3. `to_canonical_bytes(module) → bytes`; write to a build-scratch file.
4. `include_bytes!` wiring: the guest embeds the bytes at build time.
   Reproducible: same source ⇒ same bytes.
5. Shell `cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
   (with CARGO hygiene).
6. Emit the artifact; print its hash.

Per-project Rust codegen of arbitrary user tools is deferred to γ; β.7.5
links a fixed `no_std` tool library and bakes the IR.

### Decision 7 — Determinism `Config`

Host `wasmtime::Config`:

- `cranelift_nan_canonicalization(true)` — canonicalise all NaNs
  (f32/f64/v128).
- `wasm_relaxed_simd(false)` or `relaxed_simd_deterministic(true)`.
- Fuel-based interruption (not epoch) if any bounding is needed.

Time + randomness are **host imports** seeded deterministically for conformance
(the existing `Clock` / `RandomSource` ports from β.1). Guest iteration order
is guarded by `BTreeMap` / `hashbrown` discipline already in `tau-runtime-core`.

### Decision 8 — New triple `any-wasi-strict` (Reserved → Available)

Graduates the `any-wasi-strict` triple: `Platform::Any + AdapterFamily::Wasi +
Strict`. Capability shapes: `{FilesystemRead, FilesystemWrite, NetworkHttp}`.
**No `ProcessExec` / `AgentSpawn`** — wasm guests cannot subprocess or
OS-spawn, and this is enforced at build time (see Decision 6).

### Decision 9 — Wasm/WASI triples are exempt from the process-gate adapter registry

`AdapterFamily::Wasi` has **no `RegistryKind`** by design. Capability
enforcement for wasm happens at the **WASI boundary** (the wasm host + the WIT
capability imports), not via an OS process-gate adapter. The
`registry_shape_coverage_check` test asserts this positively: a Wasi Available
triple must have no process-gate registration.

`tau check --target any-wasi-strict` currently emits a non-fatal
"no local adapter; static cross-check only" Warning — which is correct until
the wasm host runner (`tau-wasm-host`) lands in β.7.5 Phase 3.

## Consequences

**Positive:**

- The same `run_ir` and `tau-native-tools` implementations run in both the dev
  and wasm profiles — divergence can only enter via the 3 pinned host imports
  or wasmtime codegen (NaN-canon + deterministic relaxed-SIMD). This is
  structural parity, not aspirational.
- `tau build wasm` refuses at build time any project that requires capabilities
  wasm guests cannot satisfy (`ProcessExec`, `AgentSpawn`) — consistent with
  tau's build-time enforcement discipline.
- The p2/p3 executor seam means WASI 0.3 async is a γ lift with no WIT world
  or API break.
- ADR-0047 (in-wasm MCP facilitator) can land independently in Phase 5 without
  reshaping this ADR's decisions.

**Negative / obligations:**

- `tau-wasm-guest` is a new `no_std` + alloc crate; CI must `rustup target add
  wasm32-wasip2` and build the exact release profile early (PR-2) to catch
  `no_std`/LTO toolchain bugs.
- The observable is `ConformanceReport`, not a literal `RunEvent` stream.
  Any consumer that expects a stream (β.6 conformance gate) is a **named gap**:
  promoting to a literal stream is deferred to β.6.
- Real (non-cassette) MCP transport from inside wasm is **reserved but
  unimplemented** in β.7.5. Projects that require live MCP connections cannot
  `tau build wasm` until the γ.1 `tau:mcp` host import lands.
- `tau check --target any-wasi-strict` emits a non-fatal Warning ("no local
  adapter; static cross-check only") until the wasm host runner lands.
  This is intentional and correct.

> **Status (PR-E2, 2026-06-20):** the guest now drives `run_ir_streaming`
> over the baked IR and returns the serialized typed `RunEvent` stream; the
> `dev == wasm` conformance arm (`WasmMode`) is flipped live in PR-G.

## Alternatives considered

- **Approach A — thin guest, tools in host imports:** rejected because it
  defeats the portability goal. The guest would merely be a protocol adapter,
  not a self-contained harness. Wasm-isolation benefit evaporates.
- **Literal `RunEvent` stream observable:** deferred to β.6, not rejected.
  `run_ir` never emitted `RunEvent`s; adding a new internal stream purely for
  wasm would be speculative scope at this phase. `ConformanceReport` is the
  implemented contract.
- **`cargo-component`:** rejected; experimental, std-leaning, and adds build
  complexity without benefit over `wasm32-wasip2` + raw `wit-bindgen`.
- **WASI 0.3 async in v1:** deferred to γ; cross-language tooling is immature
  (shipped 2026-06-11, 3 days before β.7.5 started). The thin executor seam
  keeps the door open.
- **Process-gate adapter for wasm triples:** rejected; capability enforcement
  for wasm is at the WASI host boundary, not via OS process-gate adapters.
  Forcing a `RegistryKind` for `AdapterFamily::Wasi` would be architecturally
  wrong and would mislead `tau check` into treating wasm the same as a native
  triple.

## References

- Design spec:
  `docs/superpowers/specs/2026-06-14-beta-7-5-wasm-aot-design.md`
- Plan: `docs/superpowers/plans/2026-06-14-beta-7-5-wasm-aot.md`
- Related ADRs: [ADR-0034](0034-target-triple-registry.md) (target registry),
  [ADR-0037](0037-workflow-ir.md) (workflow IR),
  [ADR-0040](0040-tau-dev-repl.md) (`tau dev` REPL + β.7/β.7.5 split),
  [ADR-0038](0038-mcp-facilitator.md) (MCP facilitator)
- ADR-0047 — in-wasm MCP facilitator (forthcoming, Phase 5)
