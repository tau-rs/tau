# Design: `ir_format` acceptance window + walked `IrFeature` fit (D8-B)

**Date:** 2026-07-19
**Branch base:** `feat/ir-format-acceptance-window`
**ADR:** 0059 — *ir_format acceptance window + walked feature-fit*
**Status:** Approved (brainstorm), pending spec review

## Problem

`ir_format` is carried on every `IrModule` but never enforced. Two concrete
holes, both verified against `main @ d678802d`:

1. **Silent forward-incompatibility.** `from_canonical_bytes`
   (`crates/tau-ir/src/canonical.rs:31`) is a plain `serde_json::from_slice`.
   `IrModule` and its nested types do **not** use `deny_unknown_fields` (the
   only use in the crate is `budget.rs:12`). So a newer runtime's
   semantics-bearing optional field — exactly what `durable` was — is silently
   dropped by an older `tau`. The CLI bundle path only logs `ir_format`, and
   only on `--dry-run` (`crates/tau-cli/src/cmd/run.rs:118-123`); the wasm guest
   never inspects it (`crates/tau-wasm-guest/src/guest.rs:104`).

2. **Published-but-unrunnable schema.** `ir_format` v2.4.0 publishes
   `Branch`/`Parallel`/`Loop`/`Suspend`, but no backend executes them: the
   interpreter returns `RuntimeError::Internal` on all four
   (`crates/tau-runtime-core/src/interpreter/pipeline.rs:316-347`) and the wasm
   guest supports only single-agent, no-pipeline modules
   (`guest.rs:106-119`). An external producer targeting the published schema can
   build a valid, unrunnable bundle, and only discovers it mid-run.

This is the **interchange half** of the build-time capability policy. There is
no separate D7-B ADR filed; ADR-0059 covers both build-time and load-time
enforcement of feature fit.

## Goals

- A newer-but-incompatible module is **rejected at decode**, not silently
  degraded, with a clear message.
- Feature support is **derived by walking the module** — no declared feature
  list in the module that could lie or drift from what the walk sees.
- A module using a feature the runtime cannot execute is rejected **at load**
  (and at **build**, target-aware), not mid-run.
- Native and wasm lanes reach the **same verdict** on shared fixtures.
- `tau-ir` and `tau-runtime-core` stay `no_std` / no-default-features clean
  (`BTreeSet` is `alloc` — fine).

## Non-goals

- No structured error over the wasm WIT boundary (Decision 2a — see below).
- No `error[IRxxx]` diagnostic-code convention (Decision 1a — see below).
- Not landing execution of Branch/Parallel/Loop/Suspend — that is EPIC 4.2
  (#399). This work *forces* 4.2 to flip the support sets (honesty test).

## Locked decisions (from brainstorm)

- **1a — no diagnostic codes.** Errors are structured `thiserror` variants with
  rich prose messages, matching every existing `IrError` / `LowerError`. No
  `error[IR001]` / `error[IRFIT001]` bracket-code prefix (the repo has no such
  convention).
- **2a — keep the wasm WIT as `result<string, string>`.** The guest returns the
  rejection's `Display` string on the `Err` arm. No WIT/ABI change, no
  host-embedder churn. `wit/tau-host.wit:26` is unchanged.
- **3a — two stacked PRs.** PR1 (version gate + closed decode) is mergeable
  alone; PR2 (feature-fit) stacks on it. ADR-0059 + conformance README refresh
  land with PR1.
- **B — dedicated decode error type.** New `DecodeError` in `tau-ir`
  (`no_std` `thiserror`); `from_canonical_bytes` changes signature.
  Kept distinct from lowering's `LowerError` because decode ≠ validation.

## Architecture

### Component 1 — `tau-ir` two-phase decode (PR1)

New module-decode surface in `crates/tau-ir/src/canonical.rs`.

```rust
// crates/tau-ir/src/decode.rs (new) — or extend canonical.rs
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("bundle uses ir_format {found}; this tau reads up to {supported_up_to}")]
    FormatTooNew { found: String, supported_up_to: String },
    #[error("bundle uses ir_format major {found}; this tau is major {current}")]
    FormatMajorMismatch { found: String, current: String },
    #[error("ir_format is missing or unparseable: {0}")]
    BadFormat(String),
    #[error(transparent)]
    Serde(#[from] serde_json::Error), // carries serde's unknown-field message
}

pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, DecodeError>;
```

Two phases, in order:

1. **Peek only `ir_format`.** A minimal partial-decode struct
   (`struct FormatPeek { ir_format: IrFormatVersion }`, *without*
   `deny_unknown_fields`) so unknown fields from a newer minor do **not** mask
   the version error. Parse `found` and `CURRENT` after stripping the `v`
   prefix (`ir_format` values are `"v2.4.0"`-shaped, `module.rs:40`). Use a
   tiny internal `(major, minor, patch)` parse — no new semver dep.
2. **Gate.** Accept ⟺ `major == CURRENT.major ∧ minor ≤ CURRENT.minor`.
   - newer minor → `FormatTooNew { found, supported_up_to }`
     (`supported_up_to` rendered as `"2.<CURRENT.minor>.x"`).
   - different major → `FormatMajorMismatch`.
3. **Closed full decode.** Add `#[serde(deny_unknown_fields)]` across the IR
   type tree (`IrModule`, `Workflow`, and nested step/pipeline/check/trigger
   types). Within an accepted window, an unknown field means corrupt or lying
   input → reject (serde error, wrapped `DecodeError::Serde`).
   - **CAUTION:** `serde(untagged)` types (`PromptSource`, D6-B) interact with
     `deny_unknown_fields` — untagged enums try each arm, and `deny_unknown_fields`
     on the arms changes which arm matches. Test each untagged arm explicitly for
     both accept and reject.

### Component 2 — wire the gate at every load site (PR1)

Grep `from_canonical_bytes` is exhaustive. Sites (verified):

| Site | File | Handling |
|---|---|---|
| CLI `run --bundle` | `crates/tau-cli/src/cmd/run.rs:114` | map `DecodeError` → CLI error, exit non-zero |
| CLI `verify --bundle` | bundle reproduce path — **confirm** it decodes via `from_canonical_bytes` (grep at impl); if it decodes independently, gate there too | same |
| wasm guest | `crates/tau-wasm-guest/src/guest.rs:104` | return `DecodeError::to_string()` on the `Err(string)` arm (Decision 2a) |
| conformance loader | `crates/tau-ir-conformance/src/bundle_mode.rs:169` | propagate |
| tests | `tau-ir/tests/*`, `tau-cli/tests/cmd_build_wasm.rs:23` | update to new signature |

### Component 3 — `IrFeature` + walked `required_features` (PR2)

```rust
// crates/tau-ir
#[non_exhaustive]
pub enum IrFeature {
    Pipeline, Branch, Parallel, Loop, Suspend,
    Checks, Subflow, McpTools, NativeTools, DeterministicSteps, Triggers,
    // extend as the IR grows
}

pub fn required_features(m: &IrModule) -> BTreeSet<IrFeature>;
```

`required_features` is derived by **walking** the module, recursing nested
control-flow bodies. It reuses the typecheck recursion shape
(`crates/tau-ir-lower/src/lower/typecheck.rs:263`, `validate_step_run`, which
already recurses `Branch.then`/`otherwise`, `Parallel` branches, `Loop.body`)
so the feature walk cannot miss what typecheck sees. No declared list in the
module — walked truth can't drift.

### Component 4 — backend support sets, next to executing code (PR2)

Support sets live beside the code that executes them, so they can't drift:

- **`tau-runtime-core` interpreter:** `pub const SUPPORTED_FEATURES: &[IrFeature]`.
  Confirm the exact set against `pipeline.rs` / `agent_loop.rs` arms — expected
  today: `Pipeline, Checks, Subflow, McpTools, NativeTools, DeterministicSteps,
  Triggers`; **NOT** `Branch/Parallel/Loop/Suspend` (those still return
  `RuntimeError::Internal` until EPIC 4.2).
- **`tau-wasm-guest`:** its much smaller set — today effectively single-agent,
  no pipeline (confirm against `guest.rs:106-119`).

### Component 5 — enforce at BOTH ends (PR2)

- **BUILD (target-aware).** In `crates/tau-cli/src/cmd/build.rs`, after
  `lower_project` succeeds (`build.rs:438`, `target` in scope), map the resolved
  triple → backend profile via `tau_ports::target::registry::lookup` +
  `adapter_family` (`Native`/`Container`/`Wasi`) and require
  `required_features(module) ⊆ profile_features`, else a strict `LowerError`
  (no override flag), following the `capability_fit.rs:36` precedent. Message
  shape (prose, per 1a): `"workflow uses Branch, unsupported by target
  wasm32-... (supports: ...) — supported from EPIC 4.2"`.
- **LOAD.** At the same sites as PR1, *after* decode: `required ⊆ SELF
  SUPPORTED_FEATURES` → structured load error. This is the arm that protects
  against modules `tau` never compiled. It **replaces** the interpreter's
  mid-run `RuntimeError::Internal` (`pipeline.rs:316-347`) as the user-facing
  surface; keep those arms as defense-in-depth but they become unreachable from
  any gated load path.

## Error handling summary

| Condition | Where caught | Surface |
|---|---|---|
| newer minor / wrong major | decode (PR1) | `DecodeError::FormatTooNew` / `FormatMajorMismatch` |
| unknown field in accepted window | decode (PR1) | `DecodeError::Serde` (deny_unknown_fields) |
| required feature ⊄ target profile | build (PR2) | strict `LowerError`, exit 2 |
| required feature ⊄ self support | load (PR2) | structured load error, non-zero |
| control-flow reaches interpreter arm | run (defense-in-depth) | `RuntimeError::Internal` (now unreachable via gated load) |
| wasm guest, any of the above | guest (PR1/PR2) | `Err(String)` = `Display` of the above (2a) |

## Testing

**PR1**
- Version window truth table: equal-minor, lower-minor, minor+1, major+1,
  major−1 (all against `CURRENT = v2.4.0`).
- `deny_unknown_fields`: unknown top-level field, unknown nested field — both
  rejected within an accepted window.
- `PromptSource` untagged arms: each arm decodes when valid, rejects on unknown
  field.
- All existing goldens + schema conformance fixtures still decode.
- New invalid conformance fixtures under `schemas/ir/conformance/invalid/`:
  unknown top-level field, unknown nested field, `minor+1`, `major+1`.
- Refresh `schemas/ir/conformance/README.md` version (currently stale at
  v2.3.0; schema + tests are already v2.4.0).

**PR2**
- **Feature-set honesty test (one per backend, the drift guard):** for every
  `IrFeature`, a minimal module exercising it; assert
  `feature ∈ SUPPORTED ⟹ executes past load` and
  `feature ∉ SUPPORTED ⟹ rejected AT LOAD` (not mid-run). This is what forces
  EPIC 4.2 to flip the sets — note it in issue #399.
- Build-time: `required ⊄ target` rejected for a wasm target on a
  Branch-using module.
- Wasm lane: guest rejects `minor+1` and unsupported-feature modules with
  structured (string) errors across the WIT boundary; native/wasm verdict
  parity on shared fixtures.
- No-default-features isolated checks: `-p tau-ir`, `-p tau-runtime-core`
  (`no_std` intact; `BTreeSet` is `alloc`).

## PR sequencing

1. **PR1** — `feat/ir-format-acceptance-window`: `DecodeError`, two-phase
   decode, `deny_unknown_fields`, all load sites wired, conformance fixtures +
   README refresh, **ADR-0059**, SUMMARY.md entry.
2. **PR2** — `feat/ir-feature-fit` stacked on PR1: `IrFeature`,
   `required_features` walk, backend `SUPPORTED_FEATURES`, build + load
   enforcement, honesty tests. Update ADR-0059 if PR1's is a placeholder.

## Conventions

- `feat/*` branches; conventional, imperative, scoped commits.
- CLAUDE.md cargo rules (`CARGO_TARGET_DIR`, `-p`, `timeout`,
  `CARGO_INCREMENTAL=0`); `cargo nextest`.
- Cross-reference: this is the interchange half of the build-time feature
  policy; EPIC 4.2 (#399) flips the support sets when it lands execution.
