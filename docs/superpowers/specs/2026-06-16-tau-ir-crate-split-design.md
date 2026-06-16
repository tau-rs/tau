# tau-ir Crate Split — Design

**Date:** 2026-06-16
**Status:** Approved (brainstorming)
**Goal:** Make `tau-runtime-core` structurally incapable of linking `tau-pkg` (and therefore `tokio`/`rustix`/`fs4`), so it cross-compiles to `wasm32-wasip2`, by splitting `tau-ir` into a pure no_std IR crate and a separate std-side lowering crate.

This is the second half of the β.7.5 "core builds for wasm" work. The first half — the `SkillResolver` port that removed core's *direct* `tau-pkg` dependency — is already implemented and reviewed (see *Relationship to the SkillResolver port* below). This spec covers the *transitive* `tau-pkg` leak that the port work cannot reach.

---

## Problem

`tau-runtime-core` depends on `tau-ir` (non-optional). `tau-ir` declares `#![no_std]` but its **default feature `with-std-adapters`** gates a `lower` module that depends on `tau-pkg` (→ `tokio`, `fs4` → `rustix 0.38.44`). `rustix 0.38.44` uses `feature(wasip2)`, which is rejected on the stable channel, so:

```
cargo build -p tau-runtime-core --target wasm32-wasip2 --no-default-features
# error[E0554]: `#![feature]` may not be used on the stable release channel  (rustix)
# error: Only features sync,macros,io-util,rt,time are supported on wasm.    (tokio)
```

Two compounding facts (both verified 2026-06-16):

1. `with-std-adapters` is a **default** feature and pulls `tau-pkg`. Core sets `tau-ir = { default-features = false }`, but the member-level override does **not** suppress it — the real non-dev build graph is `tau-pkg → tau-ir → tau-runtime-core` (feature unification does not reliably honor member-level `default-features = false` for inherited workspace deps).
2. `tau-ir` **cannot compile** `--no-default-features` at all: `IrError::McpBuild(#[from] crate::lower::McpBuildError)` (`crates/tau-ir/src/error.rs:98`) references the feature-gated `lower` module unconditionally → `E0433: could not find lower in the crate root`.

**Why a crate split rather than a feature flip:** a feature flag is an interface the build resolver can silently violate (exactly what happened with `default-features = false`). A crate boundary is one it cannot. The kernel's std-freedom must be **structural**, not a flag that downstream `features = [...]` or unification can flip back on. This matches tau's "any check that *could* run at build time *must* run at build time" discipline.

**Decisive enabling facts:**
- `tau-runtime-core` uses **only pure `tau-ir`** — `IrModule`, `check`, `context`, `pipeline`, `template`, `Workflow`, `CapabilityTable`, `ToolImpl`, `NativeFnRef`, `IrFormatVersion`, `Deterministic`. It touches **nothing** behind `with-std-adapters` (`lower`, `message` envelope adapters, `extern crate std`). So this is a feature *split into crates*, not a crate *rewrite*.
- `IrError` is referenced only in `tau-ir/src/error.rs` (its definition), `lib.rs` (re-export), and `lower/` (every construction site). The pure modules (`check`, `module`, `template`, …) and the kernel never touch it. The **only** concrete out-of-crate type reference is `tau-cli/src/cmd/build.rs:356` (`Option<tau_ir::error::IrError>`); all other workspace mentions are comments or use the `Display` *string*. `IrError` is therefore *entirely* a lowering concern — it relocates wholesale (no split needed).

---

## Crate topology

```
        ┌─────────────────────────────────────────────────────────┐
        │  tau-ir  (PURE: #![no_std] + alloc, genuinely no_std —    │
        │          builds for wasm32-unknown-unknown)               │
        │  ── IR types: module, node, pipeline, subflow, capability  │
        │  ── interpreter support: check, context, template, budget  │
        │  ── canonical + hash + ids                                 │
        │  ── message PAYLOAD conversions (un-gated, no_std)         │
        │  ── IrError  (IR-structural validation ONLY)              │
        └───────────────▲─────────────────────▲────────────────────┘
                        │                     │
       depends on pure  │                     │  depends on pure
        ┌───────────────┴──────────┐   ┌──────┴───────────────────────────┐
        │  tau-runtime-core         │   │  tau-ir-lower  (STD side)          │
        │  (kernel — only pure IR)  │   │  ── lower/: ProjectConfig→IrModule │
        │  NO tau-pkg, NO tokio     │   │  ── dep: tau-pkg (std)             │
        └───────────────────────────┘   │  ── LowerError (wraps IrError +    │
                                         │     McpBuildError + Parse)         │
                                         └────────────────────────────────────┘
                                            ▲          ▲            ▲
                                  tau-cli ──┘  conformance ──┘  ir-conformance ─┘
```

- **`tau-ir`** keeps its name — it *is* the IR types; the kernel and most consumers want the pure thing, so it carries no suffix.
- **`tau-ir-lower`** is a new crate holding lowering + the `tau-pkg` dependency. Namespaced under `tau-ir`; says exactly what it does. (`tau-ir-build` rejected — collides with the `tau build` verb and `tau-pkg::bundle::build`.)
- The dependency graph **structurally** guarantees the kernel cannot link `tau-pkg`/`tokio`/`rustix`.

---

## What moves

| Item (today in `tau-ir`) | Destination | Notes |
|---|---|---|
| `lower/` module (whole: `mod.rs`, `parse.rs`, `resolve.rs`, `typecheck.rs`, `capability_fit.rs`, `mcp_build_error.rs`) | `tau-ir-lower` | the `ProjectConfig → IrModule` translation; carries the `tau-pkg` dep |
| `error.rs` (the `IrError` type, **all** variants) | `tau-ir-lower`, renamed `LowerError` | `IrError` is referenced only by `error.rs`/`lib.rs`/`lower/` — it is entirely a lowering error |
| gated `Message` envelope `From` impls (`SystemTime` ones, `message.rs:163,183`) | **delete** if zero users (verified: grep shows none); else → `tau-ir-lower` | dead-code candidate — confirm during implementation |
| `with-std-adapters` feature | **deleted** | the boundary is the crate, not a flag; `extern crate std` in `tau-ir` is removed |
| `tau-pkg` dep (and the `[dev-dependencies] tau-pkg` used by lower tests) | `tau-ir-lower` | only `lower` used it |

### The error type (the contract-defining decision)

`IrError` does **not** split. Evidence (see *Decisive enabling facts*) shows it is constructed only inside `lower/` and consumed by type only at `tau-cli/src/cmd/build.rs:356` — there are no pure-crate or kernel consumers to justify keeping a stripped-down `IrError` in `tau-ir`. So the entire type **relocates to `tau-ir-lower` and is renamed `LowerError`** (its variants — `Parse`, `McpBuild`, `CapabilityFitFailed`, the `Unknown*` family, the pipeline/context variants — are unchanged). The `#[from] McpBuildError` wiring stays intra-crate (both now live in `tau-ir-lower`), so the `E0433` that breaks `--no-default-features` today disappears.

After the move, **pure `tau-ir` has no IR-validation error type** — its public error surface is empty of lowering/parse/MCP concepts because it has no such surface at all. A consumer that only *interprets* a pre-built `IrModule` (the kernel) never sees `LowerError`; a consumer that *builds* IR from source (`tau build`, conformance) imports `tau_ir_lower::LowerError`.

The single concrete consumer reference becomes:
```rust
// tau-cli/src/cmd/build.rs
pub lower_error: Option<tau_ir_lower::LowerError>,   // was: Option<tau_ir::error::IrError>
```

**Implementation note:** every `IrError::Variant` construction site lives inside the moving `lower/` files, so the `IrError` → `LowerError` rename is a within-`tau-ir-lower` mechanical rewrite — no pure-crate code references the variants. If a `cargo build` after the move reports a pure-`tau-ir` module constructing `IrError`, that is itself lowering code and moves too (grep confirms there is none today).

---

## no_std hardening for `tau-ir`

After the split, `tau-ir` has no std-gated code of its own. To remove its own remaining std-puller:

- Change `serde_json` in `crates/tau-ir/Cargo.toml` to `{ default-features = false, features = ["alloc"] }`. (Today it pulls std by default.)
- The other deps are already no_std-clean: `tau-domain`/`tau-ports` (no_std + serde), `chrono` (alloc), `sha2` (default-features = false), `hashbrown`, `foldhash`, `thiserror` (default-features = false).

This makes `tau-ir`'s *own* code std-free and builds clean on `wasm32-wasip2`. Genuine bare-metal no_std (`wasm32-unknown-unknown`) is not yet reachable because of the `uuid`/`tau-domain` randomness gate — see *Known limitation* under Structural enforcement.

---

## Structural enforcement (CI)

Both crates are guarded at **`wasm32-wasip2`** (`--no-default-features`):

| Crate | Target | Rationale |
|---|---|---|
| `tau-ir` | **`wasm32-wasip2`** (std target) | proves no tokio/rustix; the stricter `wasm32-unknown-unknown` (genuine bare-metal no_std) is **blocked by `uuid`** — `tau-domain` pulls `uuid`, whose `getrandom` backend won't compile on `wasm32-unknown-unknown` without an explicit randomness source. `tau-ir`'s own source is leak-free; the ceiling is the dep graph, not this crate. See *Known limitation*. |
| `tau-runtime-core` | **`wasm32-wasip2`** (std target) | core's real ceiling — `globset` (used by `apply_scope_paths`) is hard-std, so bare-metal is impossible regardless |

Both guards run `--no-default-features`. They live in the existing `runtime-core-no-std` job in `.github/workflows/ci.yml` (which already runs `cargo check -p tau-runtime-core --no-default-features`). The job adds:

```yaml
      - name: tau-ir builds for wasm32-wasip2 (no tokio/rustix)
        run: |
          rustup target add wasm32-wasip2
          cargo build -p tau-ir --no-default-features --target wasm32-wasip2
      - name: tau-runtime-core builds for wasm32-wasip2
        run: |
          rustup target add wasm32-wasip2
          cargo build -p tau-runtime-core --no-default-features --target wasm32-wasip2
```

### Known limitation — genuine bare-metal no_std (`wasm32-unknown-unknown`)

After this split `tau-ir` is source-level no_std-pure (zero `std`/`tau-pkg` references), but its dep graph still pulls `uuid` via `tau-domain`. `uuid` (v4/v7 → `getrandom`) refuses to compile on `wasm32-unknown-unknown` without an explicit randomness backend, so the strictest guard is not reachable here. Routing `uuid` minting in `tau-domain` through the existing `RandomSource` port (the way ULID already is) would unblock it — that is a separate `tau-domain` sub-project, out of scope for this split. Filed as a follow-up.

---

## Consumer migration

| Crate | `tau-ir` (pure) | `tau-ir-lower` | tau-pkg reachable? |
|---|---|---|---|
| `tau-runtime-core` | ✓ | — | **no** ✅ |
| `tau-runtime-tokio` | ✓ | — | no (grep-clean of `lower`) |
| `tau-ts-extract` | ✓ | — | no |
| `tau-cli` | ✓ (transitive) | ✓ (uses `lower`) | yes (host build tool — fine) |
| `tau-conformance` | ✓ | ✓ | yes |
| `tau-ir-conformance` | ✓ | ✓ | yes |

Migration is mechanical: rewrite `tau_ir::lower::X` → `tau_ir_lower::X` in the three lowering consumers (`tau-cli` `cmd/{build,run,dev/session}.rs`, `tau-conformance` `scenario.rs`, `tau-ir-conformance` `{bundle_mode,dev_mode}.rs`), and rewrite the single type reference `tau_ir::error::IrError` → `tau_ir_lower::LowerError` (`tau-cli/src/cmd/build.rs:356`). A `cargo check --workspace` deterministically catches any path mis-mapped.

`tau-ir-lower`'s `Cargo.toml` depends on `tau-ir` (pure) + `tau-pkg` + whatever `lower` already used (`serde_json` with std is fine here — this crate is std-side). `tau-ir`'s `[dev-dependencies] tau-pkg` (used only by `lower` tests) moves to `tau-ir-lower` along with those tests.

---

## Relationship to the SkillResolver port (already landed)

The two halves are complementary and both required for `core` to reach zero `tau-pkg`:

- **SkillResolver port** (committed `7c93ada`, `55857d2`, `1024b15`, `bccbf0c`, `2340362`): a `tau_ports::SkillResolver` trait + `TauPkgSkillResolver` adapter in `tau-runtime-tokio`, removing core's **direct** `tau-pkg` dependency. This stands as-is.
- **tau-ir crate split** (this spec): removes the **transitive** `tau-pkg` dependency via `tau-ir`'s default feature.

Final state after both: `cargo build -p tau-runtime-core --no-default-features --target wasm32-wasip2` succeeds.

---

## Testing

- **`tau-ir` (pure):** existing IR tests stay; they must pass on host AND the crate must `cargo build --no-default-features --target wasm32-unknown-unknown`. Add no new logic — this is a relocation, so behavior is unchanged. The `IrError` variants that remain keep their existing unit coverage.
- **`tau-ir-lower`:** the `lower` module's existing tests move here verbatim (they already exercise `ProjectConfig → IrModule` + `McpBuildError` + the `Parse` path). They reference `LowerError` (renamed from `IrError`) — a behavior-preserving rename, so the assertions are unchanged beyond the type name.
- **Workspace:** `cargo check --workspace` and `cargo nextest run` across affected crates green. Conformance suites (`tau-conformance`, `tau-ir-conformance`) green — they exercise the real lowering path and are the integration safety net for the move.
- **The guard:** both wasm targets build in CI.

---

## Out of scope

- Dropping `globset` from `tau-runtime-core` (would let core target bare-metal too) — separate concern, not required for `wasm32-wasip2`.
- Any change to the IR format, lowering semantics, or interpreter behavior — this is a pure relocation + error-type refactor.
- Re-litigating the SkillResolver port design — done and reviewed.
