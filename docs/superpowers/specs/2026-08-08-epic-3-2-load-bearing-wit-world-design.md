# EPIC 3.2 (follow-on) — Make the generated WIT world load-bearing

**Status:** approved (design)
**Date:** 2026-08-08
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 3, story 3.2 (DoD close)
**Depends on:**
- 3.1 (`tau-ports::target::wasi_map`, merged PR #511) — consumed read-only.
- 3.2-generator (merged PR #517) — `tau-ports::target::wit_world::generate_world`
  + `tau-cli::cmd::build_wasm::world_from_module`; consumed read-only, extended.
**Crate surface:** `tau-wasm-guest` (bindgen world + `build.rs` + vendored WASI
`.wit`); `tau-cli::cmd::build_wasm` (world injection). No change to the pure
generator in `tau-ports`.

## Why this spec exists

PR #517 shipped the story-as-worded: `generate_world(caps) -> String` folds the
3.1 mapping table, unions `Disposition::Wasi` imports, expands a hardcoded
transitive closure, and `tau build wasm` writes the result to `<out>.wit`
beside `<out>.wasm`. It is deterministic and `[allow]`-bounded.

But that `.wit` is **descriptive, not load-bearing**. Its own header says so
(`wit_world.rs:10-12`): "without vendored WASI `.wit` packages it is not
standalone-resolvable WIT." Concretely:

- The guest (`tau-wasm-guest`) is compiled by `wit_bindgen::generate!({ world:
  "runner", path: "../../wit" })` against the **frozen** `wit/tau-host.wit`
  world, which imports **only** `tau:host/host` and no WASI. The generated
  `.wit` is written *beside* the binary and never feeds the compilation.
- So the Epic DoD — *"an ungranted cap is un-importable at the ABI"* — is **not
  structurally met**. The manifest and the component can diverge; a cap could be
  absent from the `.wit` while the binary's real ABI is unchanged.

This follow-on closes that gap at the **compile-time** tier (cut line **A3**,
below): the guest is compiled against the *generated* world, so the world the
component is bound to is cap-derived and reproducible, and an ungranted cap's
WASI interface is provably absent from that world.

## Cut line (A3) and why not A1/A2

The DoD *"un-importable at the ABI"* is a **negative property**: ungranted ⇒ the
interface is absent from the world the guest is compiled against ⇒ the guest
physically cannot bind or name it. There is currently **zero WASI anywhere** —
not in the guest, not linked in the host — so `wasi:http`/`wasi:filesystem`
exist only as strings in the generated manifest. Three depths close the gap:

- **A3 (this spec):** the guest is compiled against the *generated* world
  (dynamic `wit_bindgen` + vendored WASI `.wit`). Ungranted ⇒ no bindings
  generated ⇒ un-importable at compile time. Reproducible from caps.
- **A1 = story 3.3:** host builds a `WasiCtx` from the same caps and links only
  granted interfaces. **Out of scope.**
- **A2 = story 3.4 (+ guest rewrite):** guest routes real effects through
  `wasi:http`/`wasi:filesystem` and the in-guest gate is dropped. **Out of
  scope.**

A3 is the honest, vertical, single-spec slice: it makes the world *bind the
artifact* and be *reproducible* while leaving host provisioning (3.3) and effect
routing + gate removal (3.4) as their own stories, exactly as the roadmap
sequences them.

## Non-goals (downstream stories — do NOT pull in)

- **3.3** — building the host `WasiCtx`/linker from caps (allowed-hosts,
  preopens). This spec does not link or instantiate WASI in `tau-wasm-host`.
- **3.4** — routing guest effects through WASI; dropping the in-guest gate.
- **3.5** — recording/comparing the generated `.wit` in a bundle. `tau build
  wasm` still produces **no bundle**; A3 only guarantees the `.wit` is
  byte-deterministic and emitted, which 3.5 will consume.
- No `WASI_VERSION` bump: stays `0.2.3` (the single source of truth in
  `wasi_map.rs`); the vendored deps must match it.
- No guest re-architecture: effects still flow through `tau:host/host`.

## Foundations consumed (read-only)

- `tau-ports::target::wit_world::generate_world(&[Capability]) -> Result<String,
  WitWorldError>` — the deterministic renderer (PR #517). Unchanged.
- `tau-cli::cmd::build_wasm::world_from_module(&IrModule) -> Result<String>` —
  aggregates used caps → `canon_caps` → `generate_world`. Unchanged; its output
  is now *injected into the build* instead of only written loosely.
- `wit/tau-host.wit` — frozen `tau:host@0.1.0` (interface `host` + `world
  runner`), guarded by `tau-wasm-host/tests/wit_host_drift.rs`. Left frozen.
- Existing per-build injection pattern: `TAU_IR_BYTES → build.rs → $OUT_DIR →
  include_bytes!` (`tau-wasm-guest/build.rs`). Reused in shape for the world.

## Architecture

### World injection — mirror the IR-baking pattern

The guest already receives per-build data through an env var read by its
`build.rs`. The world takes the **same shape** — no source-file mutation, no
scratch-crate copy:

```
tau build wasm:
  caps ─► world_from_module() ─► tempfile ─► env TAU_WORLD_WIT=<tmp>
                                                 │  (mirrors TAU_IR_BYTES)
  cargo build -p tau-wasm-guest --target wasm32-wasip2
      │
      ├─ guest build.rs:
      │     TAU_WORLD_WIT set?  yes ─► write it     to  wit-gen/runner.wit
      │                         no  ─► copy baseline from wit-baseline/runner.wit  (CI path)
      │     rerun-if-env-changed=TAU_WORLD_WIT; rerun-if-changed=<tmp>
      │
      └─ guest.rs:
            wit_bindgen::generate!({ world:"tau:generated/runner",
                                     path:["wit-gen","wit"] })   ← cap-derived
```

- `wit-gen/` is **gitignored**: the tree never goes dirty, no restore step, and
  the standalone CI build (`ci.yml:234`, no env) still compiles via the baseline
  fallback in `build.rs`.
- The bindgen path is `["wit-gen", "wit"]`. `wit-gen/` holds the **only**
  `runner.wit` on the path (the dynamic, cap-derived one — package
  `tau:generated`); `wit/` holds `tau-host.wit` + `deps/` and contains **no**
  `runner.wit` (so the `tau:generated` package is defined exactly once — no
  duplicate-package resolve error).
- The committed empty-cap baseline lives at `wit-baseline/runner.wit`, **off the
  bindgen path**. `build.rs` copies it into `wit-gen/runner.wit` when
  `TAU_WORLD_WIT` is unset (the CI standalone build), so the guest always has a
  resolvable world without dirtying the tree or colliding on the path.
- Concurrency: two simultaneous wasm builds race on `wit-gen/`. This is the same
  pre-existing property as the shared in-workspace guest build (which also
  races on `TAU_IR_BYTES`/`$OUT_DIR`); documented, not introduced here.

**Rejected alternatives.** In-place overwrite of a committed `runner.wit`
dirties the tree mid-build and needs failure-safe restore. Scratch-crate copy
re-resolves path-deps and is heavier. The `wit_bindgen::generate!` macro path is
a compile-time literal and cannot read `$OUT_DIR`, so the world must live at a
crate-relative path; `wit-gen/` (gitignored, build.rs-populated) is the minimal
clean choice consistent with the existing IR pattern.

### Vendored WASI `.wit` (dep table)

Vendor the WASI 0.2.3 packages the generated world imports, into
`crates/tau-wasm-guest/wit/deps/`, so the world is standalone-resolvable:

| package | interfaces used by the generated world |
|---|---|
| `wasi:io@0.2.3` | `error`, `poll`, `streams` |
| `wasi:clocks@0.2.3` | `monotonic-clock`, `wall-clock` |
| `wasi:filesystem@0.2.3` | `types`, `preopens` |
| `wasi:http@0.2.3` | `types`, `outgoing-handler` |

Pinned to `WASI_VERSION` (`wasi_map.rs:22 = "0.2.3"`). A test asserts the
vendored package versions match that constant so the closure table and the
vendored `.wit` cannot drift apart.

### Baseline / generator invariant

`generate_world(&[])` renders exactly `world runner { import host; export run:
func(prompt: string) -> result<string, string>; }`. The committed baseline
`wit-baseline/runner.wit` (package `tau:generated@0.1.0`) is byte-identical to
that output. A test asserts `generate_world(&[]) ==
read("wit-baseline/runner.wit")` so the baseline used by CI standalone builds
cannot drift from the generator.

`wit/tau-host.wit` remains frozen (its `interface host` is imported by the
generated world; its own `world runner` is now unused by the guest but left in
place so the drift test is untouched). The macro disambiguates with the fully
qualified `world: "tau:generated/runner"`.

## Risks (design holes) and the gating spike

| Risk | Impact | Mitigation |
|---|---|---|
| **`no_std` + WASI bindgen.** Does `wit_bindgen` generate `wasi:http`/`wasi:filesystem` binding modules that **compile in the `no_std` guest** (unused, but must compile)? | If they pull `std`, compiling the guest against the cap-world is **blocked**. | **Plan step 0 = a spike** (build the guest against a net+fs world; confirm `no_std` compiles). Gates which DoD tier ships. |
| **Tree-shaking.** Granted-but-unused imports may be dropped from the `.wasm`. | Cannot assert "binary world == generated world" for granted caps yet. | A3 asserts the **negative** property + reproducible text, not binary equality (that is A2/3.4). |

### Graceful degradation (DoD tiers)

The spec ships one of two tiers depending on the spike:

- **Tier 1 (preferred, if the spike passes):** the guest is **compiled against
  the generated world**. DoD verified by building a fixture and inspecting the
  component's declared world (`wasm-tools component wit`): ungranted interface
  absent, granted present. Compile-time un-importability is real.
- **Tier 2 (fallback, if `no_std` + WASI bindgen fails):** the guest stays bound
  to the frozen host-only world; A3 instead **validates** the generated `.wit`
  with `wit-parser` (resolve against the vendored deps; prove well-formed +
  cap-exact + version-pinned) without recompiling the guest against it. Weaker
  ("validated, resolvable, reproducible artifact" vs "the guest is compiled
  against it") but strictly better than #517's unresolvable manifest.

Both tiers preserve the reproducibility guarantee below.

## Definition of done

- `tau build wasm` injects the cap-derived world into the guest build via
  `TAU_WORLD_WIT` (Tier 1) **or** validates it against vendored deps with
  `wit-parser` (Tier 2). The loose `<out>.wit` emission is retained.
- Vendored WASI 0.2.3 deps present and version-pinned to `WASI_VERSION`.
- **DoD (Tier 1):** for a fixture granting `fs.read` but not `net.http`,
  `wasm-tools component wit <out>.wasm` shows `wasi:filesystem` imports and **no
  `wasi:http` import**. For a net-granting fixture, `wasi:http` present.
- **DoD (Tier 2):** the generated `.wit` resolves cleanly against the vendored
  deps via `wit-parser` for every fixture; an ungranted interface is absent from
  the resolved world.
- **Reproducibility (3.5 prep):** `<out>.wit` is byte-deterministic from caps;
  a test regenerates it and byte-compares. No bundle wiring.
- Standalone `cargo build -p tau-wasm-guest --target wasm32-wasip2` (no env)
  still compiles via the baseline fallback (CI green).
- `wit_host_drift.rs` unchanged and green.

## Testing (part of done, TDD)

- **Spike (step 0):** build the guest against a net+fs world; record whether
  `no_std` compiles. Selects Tier 1 vs Tier 2.
- **Generator/baseline invariant:** `generate_world(&[]) ==
  read("crates/tau-wasm-guest/wit-baseline/runner.wit")`.
- **Version pin:** vendored WASI package versions all equal `WASI_VERSION`.
- **Reproducibility:** `wasm_world_for_project(fixture)` regenerated twice is
  byte-identical; equals the emitted `<out>.wit`.
- **DoD (Tier 1)** — e2e, `#[ignore]` (builds the guest, like `roundtrip.rs`):
  build the `net-http` and `over-reach`/`fs`-only fixtures; assert presence /
  absence of `wasi:http` and `wasi:filesystem` in `wasm-tools component wit`.
- **DoD (Tier 2)** — unit: resolve each fixture's generated `.wit` with
  `wit-parser` against vendored deps; assert ungranted interface absent.
- **CI standalone build** stays green (no env → baseline world).

## File-change summary

```
crates/tau-wasm-guest/
  wit-baseline/runner.wit NEW committed baseline (== generate_world(&[]); off bindgen path)
  wit/deps/…              NEW vendored WASI 0.2.3 (io, clocks, filesystem, http)
  wit/tau-host.wit        unchanged (frozen); no runner.wit added here
  wit-gen/runner.wit      NEW gitignored (build.rs writes the cap-derived world here)
  build.rs                +TAU_WORLD_WIT handling (mirror TAU_IR_BYTES)
  src/guest.rs            bindgen world→"tau:generated/runner", path ["wit-gen","wit"]
  .gitignore              +wit-gen/
crates/tau-cli/src/cmd/build_wasm.rs
  run(): set TAU_WORLD_WIT tempfile before build_guest_with_ir (keep <out>.wit emit)
crates/tau-cli/tests/…    fixtures + e2e (Tier 1) or unit (Tier 2) DoD assertions
```
