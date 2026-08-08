# EPIC 3.2 — Generate the WIT world from allow-bounded caps

**Status:** approved (design)
**Date:** 2026-07-24
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 3, story 3.2
**Depends on:** 3.1 (`tau-ports::target::wasi_map`, merged PR #511) — consumed read-only.
**Crate surface:** new `tau-ports::target::wit_world` module (pure generator);
orchestration wiring in `tau-cli::cmd::build_wasm`.

## Goal

At `tau build wasm`, generate the guest component's WIT `world` from the
used-and-`[allow]`-bounded capability set. The generated world imports EXACTLY
the WASI interfaces those caps require (plus their transitive closure and the
frozen `tau:host` interface) — no more. An ungranted capability is
un-importable at the ABI; the wasm cap surface equals the `[allow]`-bounded set.

## Non-goals (downstream stories — do NOT pull in)

- **3.3** — building the host `WasiCtx` from `WasiConfig` (allowed-hosts,
  preopens) and linking WASI imports host-side. The current `tau-wasm-host`
  links only the three `tau:host/host` imports; a guest that imports `wasi:*`
  cannot yet instantiate. That is 3.3's problem, not 3.2's.
- **Wiring the generated world into the guest's *compiled* ABI.** The guest's
  `wit_bindgen::generate!` needs vendored WASI `.wit` packages to resolve
  `import wasi:http/*`; none are vendored (see "Constraints"). 3.2 emits the
  world as a deterministic **artifact/manifest**, not as the guest's compiled
  world. Making it the compiled ABI is 3.3+.
- **3.4** — dropping the in-guest capability gate on wasm.
- **3.5** — `verify --bundle` re-deriving the world and byte-comparing. 3.2
  guarantees the *determinism* 3.5 relies on, but does not implement the
  compare.

## Constraints discovered during design

- **`tau build wasm` lives in `tau-cli`**, not `tau-pkg`. Entry
  `crates/tau-cli/src/cmd/build_wasm.rs::run` → `lower_to_wasm_ir` (lowers for
  the hardcoded `any-wasi-strict` triple). No tau-pkg conflict risk.
- **No WASI `.wit` packages are vendored** anywhere in the tree (only
  `wit/tau-host.wit`). There is no in-repo WASI resolver (`wit-parser` is a
  dev-dep used solely to parse the self-contained `tau-host.wit`). Therefore
  transitive-closure resolution MUST use a **hardcoded dep table**, and the
  generated world is a manifest — not standalone-resolvable WIT — until WASI
  packages are vendored (3.3+).
- **`build_wasm.rs` does not currently run the `[allow]`/GOV000 gate.** Only the
  main `tau build` path (`build.rs::evaluate_build_governance`) does. This is a
  pre-existing enforcement gap that 3.2 closes (see "Governance").

## Foundations consumed (read-only)

- **3.1 table** — `tau_ports::target::wasi_map`:
  `map_capability(&Capability) -> WasiMapping { imports: Vec<WitInterface>,
  config, disposition }`, `WitInterface::package_id() -> &'static str`,
  `Disposition::{Wasi, InGuest, HostMediated, Unsupported { reason }}`,
  `WASI_VERSION = "0.2.3"`. **3.2 does not modify `WitInterface`** — the
  transitive interfaces live in 3.2's own dep table as package-id strings.
- **Frozen host world** — `wit/tau-host.wit` (`package tau:host@0.1.0`,
  `interface host { complete; now-millis; next-u64 }`, `world runner { import
  host; export run: func(prompt: string) -> result<string, string> }`). The
  generated world is this world's superset with cap-derived WASI imports added.
- **Governance gate** — `tau_cli::cmd::check::{evaluate_governance,
  render_no_constitution, render_violations, CheckCtx, GovernanceFlags,
  GovernanceOutcome}` (`check/gate.rs`). Reused verbatim; not reimplemented.
- **Used caps** — the lowered `tau_ir::IrModule`
  (`module.workflow.capability_table: CapabilityTable(BTreeMap<ToolId,
  CapabilityRequirements { declared: Vec<Capability> }>)`), available inside
  `lower_to_wasm_ir`.
- **Canonicalization** — `tau_domain …capability::lattice::canon_caps(&[Capability])
  -> Vec<Capability>`.

## Architecture

```
                        tau build wasm ./project
                                │
                                ▼
   ┌─────────────────────────────────────────────────────────────┐
   │  tau-cli :: build_wasm::run()                                 │
   │                                                               │
   │  load_project ──► project.allow                               │
   │        │                                                      │
   │        ▼                                                      │
   │  evaluate_governance(project, ctx, flags)   [reused gate]     │
   │     ├ NoConstitution → GOV000, exit 2                         │
   │     ├ Violations     → refused, exit 2                        │
   │     └ Proceed        → continue                               │
   │        │                                                      │
   │        ▼                                                      │
   │  lower_to_wasm_ir ──► IrModule (used caps, gate-bounded)      │
   │        │                                                      │
   │        ▼  aggregate + canon_caps                              │
   │  Vec<Capability>                                              │
   │        │                                                      │
   │        ▼                                                      │
   │  tau-ports :: wit_world::generate_world(&caps)  [pure]        │
   │     ├ Err(UnsupportedOnWasm) → build error, exit 2            │
   │     └ Ok(String) ── WIT text                                  │
   │        │                                                      │
   │        ▼                                                      │
   │  write <out>.wit   (+ existing <out>.wasm)                    │
   └─────────────────────────────────────────────────────────────┘
```

### Why no `meet(used, ceiling)`

The governance gate's L1/L2 subset checks already enforce *tool-caps ⊆
agent-effective ⊆ root ceiling*. Once `evaluate_governance` returns `Proceed`,
the IR's used caps are provably within `[allow]`. Deriving the world from the
used caps is therefore already deriving it from the `[allow]`-bounded set; a
redundant `meet(used, ceiling)` would be speculative dead code. This mirrors how
`tau build` trusts the same gate for the native bundle's cap surface.

## Pure generator API (`tau-ports::target::wit_world`)

`no_std` + `alloc`, `#![forbid(unsafe_code)]`, `deny(missing_docs)` (workspace
lints), sibling of `wasi_map.rs`, re-exported from `target/mod.rs`.

```rust
/// Generate the guest component's WIT `world` from a capability set.
///
/// Folds each capability through `map_capability` (3.1), keeps the
/// `Disposition::Wasi` imports, unions them, expands the hardcoded transitive
/// closure, and renders a deterministic WIT `world` importing `tau:host` + the
/// resulting WASI interfaces and exporting `run`.
///
/// `InGuest` / `HostMediated` capabilities contribute no import. A capability
/// with `Disposition::Unsupported` (fs.exec, process.spawn) is a hard error —
/// it cannot be expressed on wasm.
pub fn generate_world(caps: &[Capability]) -> Result<String, WitWorldError>;

/// Error raised when a capability cannot be realized on the wasm target.
#[derive(Debug, thiserror::Error)]
pub enum WitWorldError {
    /// A capability maps to `Disposition::Unsupported` on wasm.
    #[error("capability `{cap}` cannot target wasm: {reason}")]
    UnsupportedOnWasm {
        /// Debug rendering of the offending capability.
        cap: String,
        /// The reason carried by `Disposition::Unsupported`.
        reason: &'static str,
    },
}
```

`thiserror` at this boundary is `no_std`-compatible (workspace already uses it
in `no_std` crates). If `thiserror`'s `no_std` support is unavailable in
`tau-ports`, fall back to a hand-written `core::fmt::Display` + `core::error::Error`
impl — settled during implementation, not a design decision.

### Transitive-closure dep table (hole #1)

Hardcoded, keyed by the 3.1 `WitInterface`, values are fully-qualified WASI
package-id `&'static str`s at `WASI_VERSION`:

| Direct `WitInterface` | Transitive package-ids added |
|---|---|
| `WasiHttpTypes` | `wasi:io/streams`, `wasi:io/poll`, `wasi:io/error`, `wasi:clocks/monotonic-clock` |
| `WasiHttpOutgoingHandler` | (via `WasiHttpTypes`) |
| `WasiFilesystemTypes` | `wasi:io/streams`, `wasi:io/poll`, `wasi:io/error`, `wasi:clocks/wall-clock` |
| `WasiFilesystemPreopens` | (via `WasiFilesystemTypes`) |

Second-order edges folded in: `wasi:io/streams → {io/error, io/poll}`;
`wasi:clocks/monotonic-clock → io/poll`. The union is taken over a
`BTreeSet<&str>` so output is deterministic and closure edges never
double-count. A **drift test** asserts the closure of each direct interface
matches the expected set (guards silent WASI-version/edge drift).

### Emitted WIT (deterministic)

- `package tau:generated@0.1.0;`
- `world runner { import host; <imports, sorted>; export run: func(prompt:
  string) -> result<string, string>; }`
- Imports rendered as `import <package-id>;` (e.g.
  `import wasi:http/outgoing-handler@0.2.3;`), **sorted** so cap order does not
  affect output. `import host;` mirrors the frozen `tau-host.wit` style.
- Empty cap set (or all `InGuest`/`HostMediated`) → host-only world (no
  `wasi:*` import). This is the "un-importable for ungranted caps" property.

The exact package header and `import host;` qualification form are a formatting
detail settled in implementation; the **import set** and **byte-determinism**
are the contract 3.5 depends on.

## Orchestration changes (`tau-cli::cmd::build_wasm`)

1. `BuildWasmArgs` gains `--allow-ungoverned` / `--no-governance` (mutually
   exclusive, matching `BuildArgs`).
2. `run()` evaluates the governance gate BEFORE lowering (mirror
   `build.rs::evaluate_build_governance`): `CheckCtx::load(project, false,
   Some(any-wasi-strict))` → `evaluate_governance` → GOV000 / refused / proceed.
3. After a successful lower, aggregate the used caps from
   `module.workflow.capability_table` (flatten each `declared`, `canon_caps`).
4. Call `wit_world::generate_world(&caps)`. On `Err(UnsupportedOnWasm)` print
   the diagnostic and exit 2. (Note: `any-wasi-strict` capability-fit already
   refuses process-exec/agent-spawn during lowering; the `Unsupported` arm here
   is defence-in-depth + the single place fs.exec is caught if fit ever widens.)
5. Write the WIT text to `<out>.wit` alongside the existing `<out>.wasm`.

## Testing (part of done, TDD)

**Unit — `tau-ports::wit_world` (pure):**
- net-only cap set → world imports `wasi:http/{types,outgoing-handler}` +
  `wasi:io/{streams,poll,error}` + `wasi:clocks/monotonic-clock` + `host`.
- fs-only → `wasi:filesystem/{types,preopens}` + `wasi:io/{streams,poll,error}`
  + `wasi:clocks/wall-clock` + `host`.
- mixed (net + fs) → union of both, deduped.
- empty / all-`InGuest` → host-only world, zero `wasi:*` imports.
- unsupported (fs.exec, process.spawn) → `Err(UnsupportedOnWasm)`.
- determinism: permuted cap input → byte-identical output.
- transitive-closure drift: each direct interface's closure == expected set.

Construct caps via `tau_domain::fixtures::*` (variants are `#[non_exhaustive]`;
struct literals are E0639).

**Integration — `tau-cli::build_wasm`:**
- ungoverned project (no `[allow]`) → GOV000, exit 2, no `.wit`/`.wasm`.
- over-reach (`used ⊄ ceiling`) → refused, exit 2.
- `process.spawn` tool → build error (via lowering fit and/or generator).
- happy path (governed, net-only) → `<out>.wit` exists with the expected
  import set; `<out>.wasm` still produced.

## Definition of done

Generated WIT world imports exactly the allow-bounded WASI interfaces (+
transitive + `tau:host`) for a given cap set, host-only for a cap set that
grants no WASI surface, deterministic output, `Unsupported`-on-wasm caps fail
the build, `[allow]`/GOV000 enforced on the wasm path. Unit + integration tests
green. PR open against `main`, CI green.
