# `ir_format` acceptance window + walked `IrFeature` fit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reject forward-incompatible or unrunnable IR modules at decode/build/load with clear errors, instead of silently degrading (dropped fields) or failing mid-run.

**Architecture:** Two-phase closed decode in `tau-ir` (peek `ir_format` → semver acceptance window → `deny_unknown_fields` full decode). A walked `required_features(&IrModule)` derives the feature set by recursing the module (no declared list). Build-time fit runs in `tau-ir-lower` (next to the `capability_fit` precedent); load-time fit runs at the single interpreter chokepoint (`run_ir`/`run_ir_streaming`), which **both** the native CLI and the wasm guest already funnel through.

**Tech Stack:** Rust, `no_std` + `alloc` (`tau-ir`, `tau-runtime-core`), `serde`/`serde_json`, `thiserror`, `cargo nextest`.

## Global Constraints

- **Cargo discipline (CLAUDE.md):** every cargo command is
  `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Prefer `cargo nextest run`; doctests via `cargo test --doc`.
- **`tau-ir` and `tau-runtime-core` stay `no_std`** (`#![no_std]` + `extern crate alloc`). `BTreeSet`/`BTreeMap` are `alloc` — fine. No `std` in these crates. Verify with `--no-default-features` isolated checks.
- **`tau-ir` invariants:** `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]` — every new `pub` item needs a doc comment.
- **No diagnostic-code prefixes (Decision 1a):** errors are `thiserror` variants with prose messages matching existing `IrError`/`LowerError`. No `error[IRxxx]`.
- **No WIT/ABI change (Decision 2a):** the wasm guest stays `result<string, string>`; rejections surface as the `Err(String)` = `Display` of the structured error.
- **`ir_format` current value:** `IrFormatVersion::CURRENT == "v2.4.0"` (note the `v` prefix; parse after stripping it).
- **Two stacked PRs (Decision 3a):** PR1 (Tasks 1–5) on `feat/ir-format-acceptance-window`; PR2 (Tasks 6–11) on `feat/ir-feature-fit` stacked on PR1.

## Grounding notes (verified against `main @ d678802d`)

- `from_canonical_bytes` (`crates/tau-ir/src/canonical.rs:31`) is a bare `serde_json::from_slice`; returns `serde_json::Error`.
- **No `#[serde(untagged)]` and no `#[serde(flatten)]` anywhere in `tau-ir/src`** (grep-confirmed). The handoff's `PromptSource`/untagged caution is forward-looking (D6-B, unmerged) — it does **not** apply to current `main`. If D6-B lands first, re-check the untagged arms before applying `deny_unknown_fields`.
- Load sites of `from_canonical_bytes`: `crates/tau-cli/src/cmd/run.rs:114`, `crates/tau-wasm-guest/src/guest.rs:104`, `crates/tau-ir-conformance/src/bundle_mode.rs:169`, plus tests (`tau-ir/tests/*`, `tau-cli/tests/cmd_build_wasm.rs:23`).
- `verify --bundle` reproduce path: **confirm at impl** whether it decodes via `from_canonical_bytes`; if it decodes independently, gate it too.
- The wasm guest runs the **same** `tau-runtime-core` interpreter (`guest.rs:130` → `run_ir_streaming`). Its `agents.len() == 1` gate (`guest.rs:107`) is a workflow-shape limit, **orthogonal** to `IrFeature`. ⇒ one `SUPPORTED_FEATURES`, one load chokepoint.
- `run_ir` / `run_ir_streaming` (`crates/tau-runtime-core/src/interpreter/mod.rs:42,75`) both `Result<_, RuntimeError>`, both do entry lookup at the top — the load-gate insertion point.
- Interpreter control-flow arms return `RuntimeError::Internal { message }` (`pipeline.rs:316-347`). Keep as defense-in-depth; the load gate makes them unreachable from gated paths.
- Build lowering: `tau_ir_lower::lower_project(config, target, &caches)` (`build.rs:438`) assembles the `IrModule`; error → `LowerError` → `build.rs:116-119` exits 2. Feature-fit hooks in here.
- `tau-ir-lower` deps: `tau-ir` + `tau-ports` (NOT `tau-runtime-core`). `tau-runtime-core` deps `tau-ir`. So the shared feature table must live in `tau-ir`; `tau-runtime-core` can reference it.
- `TargetTriple` is `Copy` with field `adapter_family: AdapterFamily` (`Native|Container|Remote|Wasi|Passthrough`). Registry lookup: `tau_ports::target::registry::lookup`.
- `Tool` node field is `impl_: ToolImpl` (`node.rs`); `ToolImpl` is `Native|Mcp|Subflow|Step`.

---

## Task 1: `DecodeError` + version acceptance window (PR1)

**Files:**
- Create: `crates/tau-ir/src/decode.rs`
- Modify: `crates/tau-ir/src/lib.rs` (add `pub mod decode;`, re-export)
- Modify: `crates/tau-ir/src/canonical.rs` (remove `from_canonical_bytes`; keep `to_canonical_bytes`) — or re-home per Interfaces below

**Interfaces:**
- Produces: `tau_ir::decode::DecodeError`; `tau_ir::from_canonical_bytes(&[u8]) -> Result<IrModule, DecodeError>` (re-exported at crate root, replacing the `canonical` one). `to_canonical_bytes` stays in `canonical`.

> **Decision:** move `from_canonical_bytes` into the new `decode` module so decode-gating logic lives in one place; leave `to_canonical_bytes` in `canonical`. Update `lib.rs:42` re-export from `canonical::{from_canonical_bytes, to_canonical_bytes}` to `canonical::to_canonical_bytes` + `decode::from_canonical_bytes`. The public path `tau_ir::from_canonical_bytes` is unchanged for all callers.

- [ ] **Step 1: Write the failing tests** in `crates/tau-ir/src/decode.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::to_canonical_bytes;
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ports::target::registry;

    fn module_at(version: &str) -> alloc::vec::Vec<u8> {
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion(version.into()),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        };
        to_canonical_bytes(&m)
    }

    #[test]
    fn equal_minor_decodes() {
        let bytes = module_at("v2.4.0");
        assert!(from_canonical_bytes(&bytes).is_ok());
    }

    #[test]
    fn lower_minor_decodes() {
        let bytes = module_at("v2.3.0");
        assert!(from_canonical_bytes(&bytes).is_ok());
    }

    #[test]
    fn newer_minor_is_too_new() {
        let bytes = module_at("v2.5.0");
        match from_canonical_bytes(&bytes) {
            Err(DecodeError::FormatTooNew { found, supported_up_to }) => {
                assert_eq!(found, "v2.5.0");
                assert_eq!(supported_up_to, "2.4.x");
            }
            other => panic!("expected FormatTooNew, got {other:?}"),
        }
    }

    #[test]
    fn newer_major_is_mismatch() {
        let bytes = module_at("v3.0.0");
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::FormatMajorMismatch { .. })
        ));
    }

    #[test]
    fn lower_major_is_mismatch() {
        let bytes = module_at("v1.9.0");
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::FormatMajorMismatch { .. })
        ));
    }

    #[test]
    fn malformed_version_is_bad_format() {
        let bytes = module_at("banana");
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::BadFormat { .. })
        ));
    }
}
```

- [ ] **Step 2: Run tests, verify they fail to compile** (`from_canonical_bytes` not yet in `decode`)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir decode`
Expected: FAIL (unresolved `from_canonical_bytes` / `DecodeError`).

- [ ] **Step 3: Implement `decode.rs`**

```rust
//! Version-gated, closed decode of canonical IR bytes.
//!
//! Two phases: peek `ir_format` and apply the semver acceptance window
//! (accept ⟺ major == CURRENT.major ∧ minor ≤ CURRENT.minor), then a
//! `deny_unknown_fields` full decode. Within an accepted window an unknown
//! field means a corrupt or lying module — rejected via the serde error.

use alloc::string::{String, ToString};
use serde::Deserialize;

use crate::module::{IrFormatVersion, IrModule};

/// Errors from [`from_canonical_bytes`].
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The module's `ir_format` is a newer minor than this `tau` reads.
    #[error("bundle uses ir_format {found}; this tau reads up to {supported_up_to}")]
    FormatTooNew {
        /// The module's declared `ir_format` (e.g. `v2.5.0`).
        found: String,
        /// Highest minor this `tau` accepts, rendered `MAJOR.MINOR.x`.
        supported_up_to: String,
    },
    /// The module's `ir_format` major differs from this `tau`'s.
    #[error("bundle uses ir_format {found}; this tau is a different major ({current})")]
    FormatMajorMismatch {
        /// The module's declared `ir_format`.
        found: String,
        /// This `tau`'s `ir_format` (`IrFormatVersion::CURRENT`).
        current: String,
    },
    /// The `ir_format` string is missing or not `vMAJOR.MINOR.PATCH`.
    #[error("ir_format {found:?} is missing or unparseable: {detail}")]
    BadFormat {
        /// The offending value (empty if absent).
        found: String,
        /// Why it could not be parsed.
        detail: String,
    },
    /// serde-level decode failure — including an unknown field inside an
    /// otherwise-accepted version window (`deny_unknown_fields`).
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

/// Minimal partial-decode struct: peek ONLY `ir_format`. No
/// `deny_unknown_fields` here, so unknown fields from a newer minor do not
/// mask the version error.
#[derive(Deserialize)]
struct FormatPeek {
    ir_format: IrFormatVersion,
}

/// Parse `vMAJOR.MINOR.PATCH` → `(major, minor, patch)`. Tolerates a missing
/// `v` prefix. Rejects extra dotted segments.
fn parse_semver(s: &str) -> Result<(u64, u64, u64), ()> {
    let body = s.strip_prefix('v').unwrap_or(s);
    let mut parts = body.split('.');
    let major: u64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let minor: u64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let patch: u64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    if parts.next().is_some() {
        return Err(());
    }
    Ok((major, minor, patch))
}

/// Deserialize canonical bytes to an [`IrModule`], enforcing the `ir_format`
/// acceptance window and a closed (`deny_unknown_fields`) decode.
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, DecodeError> {
    // Phase 1: peek ir_format only.
    let peek: FormatPeek = serde_json::from_slice(bytes)?;
    let found = peek.ir_format.0;
    let current = IrFormatVersion::CURRENT;

    let (fmaj, fmin, _) = parse_semver(&found).map_err(|_| DecodeError::BadFormat {
        found: found.clone(),
        detail: "expected vMAJOR.MINOR.PATCH".to_string(),
    })?;
    let (cmaj, cmin, _) = parse_semver(current).expect("CURRENT is well-formed");

    // Phase 2: acceptance window.
    if fmaj != cmaj {
        return Err(DecodeError::FormatMajorMismatch {
            found,
            current: current.to_string(),
        });
    }
    if fmin > cmin {
        return Err(DecodeError::FormatTooNew {
            found,
            supported_up_to: alloc::format!("{cmaj}.{cmin}.x"),
        });
    }

    // Phase 3: closed full decode (deny_unknown_fields lands in Task 2).
    let module: IrModule = serde_json::from_slice(bytes)?;
    Ok(module)
}
```

- [ ] **Step 4: Wire `lib.rs`** — add `pub mod decode;` (alphabetical, after `pub mod context;`… place after `pub mod canonical;` grouping is fine) and change the re-export line:

```rust
pub use canonical::to_canonical_bytes;
pub use decode::{from_canonical_bytes, DecodeError};
```

Remove `from_canonical_bytes` from `crates/tau-ir/src/canonical.rs` (delete the fn at `:29-33`); its in-module tests use it via `super::*` — change those to `use crate::from_canonical_bytes;` or keep a `use crate::decode::from_canonical_bytes;` in the test module.

- [ ] **Step 5: Run tests, verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS (new decode tests + existing canonical round-trip tests).

- [ ] **Step 6: `no_std` isolation check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir --no-default-features`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir/src/decode.rs crates/tau-ir/src/lib.rs crates/tau-ir/src/canonical.rs
git commit -m "feat(tau-ir): ir_format semver acceptance window on decode"
```

---

## Task 2: closed decode (`deny_unknown_fields`) across the IR type tree (PR1)

**Files (add `#[serde(deny_unknown_fields)]` to every `Deserialize` struct / struct-variant-bearing enum reachable from `IrModule`):**
- Modify: `crates/tau-ir/src/module.rs` (`IrModule`, `Workflow`)
- Modify: `crates/tau-ir/src/pipeline.rs` (`Pipeline`, `PipelineStep`, `StepRun`)
- Modify: `crates/tau-ir/src/node.rs` (`Agent`, `Tool`, `Deterministic`, `Subflow`, `ToolSpec`, …)
- Modify: `crates/tau-ir/src/check.rs`, `trigger.rs`, `subflow.rs`, `capability.rs`, `context.rs`, `durable.rs`, `model_ref.rs`, `tool_impl.rs`, `message.rs`
- Test: `crates/tau-ir/src/decode.rs` (extend the `tests` module)

**Interfaces:**
- Consumes: `from_canonical_bytes` from Task 1.
- Produces: no signature change — Phase 3 now rejects unknown fields.

> **Rule:** `deny_unknown_fields` is incompatible with `#[serde(flatten)]` and cannot sit on an untagged enum. Grep confirmed **neither exists** in `tau-ir/src` today, so apply uniformly. Do NOT add it to types that are only `Serialize` (none relevant) or to `IrFormatVersion` (a newtype `String`, no fields). Externally-tagged enums (the default here) accept it on their struct variants.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn unknown_top_level_field_is_rejected() {
    let mut bytes = module_at("v2.4.0");
    // Splice an unknown top-level key into the JSON object.
    let json = String::from_utf8(bytes).unwrap();
    let doctored = json.replacen('{', r#"{"bogus_top":1,"#, 1);
    bytes = doctored.into_bytes();
    assert!(matches!(
        from_canonical_bytes(&bytes),
        Err(DecodeError::Serde(_))
    ));
}

#[test]
fn unknown_nested_field_is_rejected() {
    // Build a module with a pipeline, then inject an unknown key inside
    // the nested "workflow" object.
    let target = registry::list_available().next().unwrap().triple;
    let m = IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target,
        workflow: Workflow::default(),
        triggers: alloc::vec::Vec::new(),
    };
    let json = String::from_utf8(to_canonical_bytes(&m)).unwrap();
    let doctored = json.replace(r#""workflow":{"#, r#""workflow":{"ghost":true,"#);
    assert!(matches!(
        from_canonical_bytes(doctored.as_bytes()),
        Err(DecodeError::Serde(_))
    ));
}

#[test]
fn all_known_fields_still_decode() {
    // The canonical bytes of a fully-populated module must still round-trip.
    let bytes = module_at("v2.4.0");
    assert!(from_canonical_bytes(&bytes).is_ok());
}
```

(Add `use alloc::string::String;` to the test module if not present.)

- [ ] **Step 2: Run, verify `unknown_*` tests fail** (unknown fields currently ignored)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir unknown`
Expected: FAIL (`unknown_top_level_field_is_rejected`, `unknown_nested_field_is_rejected` return `Ok`).

- [ ] **Step 3: Apply `#[serde(deny_unknown_fields)]`** to each type listed under Files. Example for `module.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct IrModule { /* … */ }

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Workflow { /* … */ }
```

Repeat the attribute for every struct/enum in the Files list. Work file-by-file; after each file run `cargo check -p tau-ir` to catch a type that has `flatten` (should be none).

- [ ] **Step 4: Run the full `tau-ir` suite + existing goldens**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS — including `canonical.rs` round-trip goldens (`pre_4_1_module_canonical_bytes_are_byte_stable`, `new_control_flow_variants_round_trip`) and the new `unknown_*` tests.

- [ ] **Step 5: Schema-feature conformance still green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir --features schema`
Expected: PASS (`schema_conformance.rs` valid fixtures still deserialize through `tau-ir`).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir/src
git commit -m "feat(tau-ir): closed decode — deny_unknown_fields across the IR tree"
```

---

## Task 3: wire the gate at every load site (PR1)

**Files:**
- Modify: `crates/tau-cli/src/cmd/run.rs:114-115` (message uses `{e}` — `DecodeError: Display`)
- Modify: `crates/tau-wasm-guest/src/guest.rs:104` (already `.map_err(|e| e.to_string())` — `DecodeError: Display`, no change needed beyond confirming it compiles)
- Modify: `crates/tau-ir-conformance/src/bundle_mode.rs:169` (adapt to `DecodeError`)
- Investigate + Modify (if applicable): `verify --bundle` reproduce path
- Modify: `crates/tau-ir/tests/trigger_hash_preservation.rs`, `canonical_idempotence.rs`, `crates/tau-cli/tests/cmd_build_wasm.rs` (only if the return-type change breaks `?`/`.expect`)

**Interfaces:**
- Consumes: `from_canonical_bytes -> Result<_, DecodeError>` (Tasks 1–2).

- [ ] **Step 1: Compile each consumer, fix fallout**

Run each and fix mechanically:
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir-conformance
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-wasm-guest --target wasm32-wasip2
```

`run.rs:115`: change `{e:?}` → `{e}` (DecodeError renders a clean prose message):
```rust
let module = tau_ir::from_canonical_bytes(&bytes)
    .map_err(|e| anyhow::anyhow!("decoding IR module from bundle: {e}"))?;
```

- [ ] **Step 2: Confirm the `verify --bundle` decode path**

Run: `grep -rn 'from_canonical_bytes\|canonical_ir_bytes' crates/tau-cli/src crates/tau-pkg/src`
If `verify --bundle` decodes independently of `from_canonical_bytes`, add the gate there too (route its bytes through `tau_ir::from_canonical_bytes`). If it only compares hashes without decoding, note that in the commit message and move on.

- [ ] **Step 3: Add a CLI-level rejection test** in `crates/tau-cli/tests/` (new or existing bundle test file)

```rust
// A bundle whose ir_payload declares ir_format v2.5.0 must be rejected by
// `tau run --bundle` with a "reads up to" message, not silently degraded.
// (Construct via the existing bundle-test harness; assert non-zero exit and
// the message substring "reads up to".)
```

Write it concretely against the existing bundle test harness in that directory (mirror the nearest existing `cmd_*` bundle test's setup).

- [ ] **Step 4: Run affected suites**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli crates/tau-ir-conformance crates/tau-wasm-guest crates/tau-ir/tests
git commit -m "feat: gate every IR load site on the ir_format acceptance window"
```

---

## Task 4: conformance fixtures + README version refresh (PR1)

**Files:**
- Create: `schemas/ir/conformance/invalid/unknown-top-level-field.json`
- Create: `schemas/ir/conformance/invalid/unknown-nested-field.json`
- Create: `schemas/ir/conformance/invalid/ir-format-minor-plus-1.json`
- Create: `schemas/ir/conformance/invalid/ir-format-major-plus-1.json`
- Modify: `schemas/ir/conformance/README.md` (stale `v2.3.0` → `v2.4.0`)
- Modify: `crates/tau-ir/tests/schema_conformance.rs` (add the four new invalid fixtures to the hardcoded `invalid_samples_are_rejected` name list; and, since minor/major-bumped fixtures are valid JSON-Schema but invalid to `tau-ir`'s decoder, assert them via `from_canonical_bytes` rather than the JSON-Schema validator — see Step 3)

**Interfaces:**
- Consumes: `from_canonical_bytes` acceptance window (Tasks 1–2).

- [ ] **Step 1: Create the fixtures.** Base each on `schemas/ir/conformance/valid/minimal.json`. For `unknown-top-level-field.json`, add `"bogus_top": 1` at the object root. For `unknown-nested-field.json`, add `"ghost": true` inside `"workflow"`. For the version fixtures, copy `minimal.json` and set `"ir_format": "v2.5.0"` and `"v3.4.0"` respectively.

- [ ] **Step 2: Refresh the README** — replace the `../tau-ir.v2.3.0.schema.json` reference (`README.md:5`) with `../tau-ir.v2.4.0.schema.json`, and any prose version mentions.

- [ ] **Step 3: Wire the tests.** The JSON-Schema validator will NOT reject `unknown-*` (schema is open) or version-bumped fixtures (schema doesn't gate the runtime window). Add a dedicated test that routes each new invalid fixture through `tau_ir::from_canonical_bytes` and asserts `Err`:

```rust
#[test]
fn decoder_rejects_forward_incompatible_and_unknown_fields() {
    for name in [
        "unknown-top-level-field",
        "unknown-nested-field",
        "ir-format-minor-plus-1",
        "ir-format-major-plus-1",
    ] {
        let bytes = std::fs::read(dir().join("conformance/invalid").join(format!("{name}.json")))
            .unwrap_or_else(|_| panic!("read fixture {name}"));
        assert!(
            tau_ir::from_canonical_bytes(&bytes).is_err(),
            "fixture {name} must be rejected by the decoder",
        );
    }
}
```

(Match `dir()`'s existing path convention in `schema_conformance.rs`.)

- [ ] **Step 4: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir --features schema`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add schemas/ir/conformance crates/tau-ir/tests/schema_conformance.rs
git commit -m "test(tau-ir): conformance fixtures for the acceptance window; refresh README to v2.4.0"
```

---

## Task 5: ADR-0059 + SUMMARY (PR1)

**Files:**
- Create: `docs/decisions/0059-ir-format-acceptance-window.md` (use `docs/decisions/template.md`)
- Modify: `docs/SUMMARY.md` (add the ADR line under the decisions section)
- Modify: `MEMORY.md` pointer is NOT part of the repo — skip.

**Interfaces:** none (docs).

- [ ] **Step 1: Write the ADR** from `template.md`. Title: *ir_format acceptance window + walked feature-fit*. Cover: the two holes (silent forward-incompat; published-but-unrunnable), the semver window rule (`major == CURRENT.major ∧ minor ≤ CURRENT.minor`), closed decode, walked `required_features`, build+load enforcement at one interpreter chokepoint, Decisions 1a/2a/3a, and that it is the interchange half of the build-time feature policy (no separate D7-B ADR filed — this ADR covers both halves). Reference EPIC 4.2 (#399) as the work that flips `SUPPORTED_FEATURES`.

- [ ] **Step 2: Add to `docs/SUMMARY.md`** next to `0058-ir-control-flow-blocks.md`.

- [ ] **Step 3: Build the book** (docs gate, per CLAUDE.md DOCS RULES):

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book`
Expected: only `[INFO]` lines; no linkcheck errors.

- [ ] **Step 4: Commit + open PR1**

```bash
git add docs/decisions/0059-ir-format-acceptance-window.md docs/SUMMARY.md
git commit -m "docs: ADR-0059 ir_format acceptance window + walked feature-fit"
git push -u origin feat/ir-format-acceptance-window
gh pr create --base main --title "feat(ir): ir_format acceptance window + closed decode" --body "<summary + ADR-0059 ref>"
```

---

## Task 6: `IrFeature` + walked `required_features` (PR2)

> Branch: `git checkout -b feat/ir-feature-fit` (stacked on `feat/ir-format-acceptance-window`).

**Files:**
- Create: `crates/tau-ir/src/feature.rs`
- Modify: `crates/tau-ir/src/lib.rs` (`pub mod feature;` + re-export)

**Interfaces:**
- Produces: `tau_ir::feature::IrFeature` (`#[non_exhaustive]`, `Copy + Ord`); `tau_ir::feature::required_features(&IrModule) -> BTreeSet<IrFeature>`.

- [ ] **Step 1: Write the failing tests** in `feature.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{Condition, GoalPredicate, Locus};
    use crate::ids::{AgentId, PipelineStepId};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::pipeline::{Pipeline, PipelineStep, StepRun};
    use tau_ports::target::registry;

    fn module_with(pipeline: Option<Pipeline>) -> IrModule {
        let target = registry::list_available().next().unwrap().triple;
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow { pipeline, ..Workflow::default() },
            triggers: alloc::vec::Vec::new(),
        }
    }

    #[test]
    fn agent_only_module_requires_nothing() {
        let m = module_with(None);
        assert!(required_features(&m).is_empty());
    }

    #[test]
    fn pipeline_requires_pipeline_feature() {
        let m = module_with(Some(Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("a".into()),
                run: StepRun::Agent(AgentId("a".into())),
                input: "${input}".into(),
            }],
        }));
        let f = required_features(&m);
        assert!(f.contains(&IrFeature::Pipeline));
        assert!(!f.contains(&IrFeature::Branch));
    }

    #[test]
    fn nested_branch_inside_loop_is_walked() {
        let inner = PipelineStep {
            id: PipelineStepId("inner".into()),
            run: StepRun::Branch {
                on: Condition { evaluates: Locus::Path("/x".into()), predicate: GoalPredicate::Exists },
                then: alloc::vec![],
                otherwise: alloc::vec![],
            },
            input: "${input}".into(),
        };
        let m = module_with(Some(Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("l".into()),
                run: StepRun::Loop {
                    body: alloc::vec![inner],
                    until: Condition { evaluates: Locus::Path("/y".into()), predicate: GoalPredicate::Exists },
                    max_iters: 3,
                },
                input: "${input}".into(),
            }],
        }));
        let f = required_features(&m);
        assert!(f.contains(&IrFeature::Loop));
        assert!(f.contains(&IrFeature::Branch)); // proves the walk recurses bodies
    }
}
```

- [ ] **Step 2: Run, verify fail to compile**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir feature`
Expected: FAIL (unresolved `required_features`).

- [ ] **Step 3: Implement `feature.rs`**

```rust
//! Walked feature-fit: the set of IR features a module actually uses.
//!
//! Derived by WALKING the module (recursing nested control-flow bodies), so
//! the set can never drift from the module's real shape — there is no
//! declared feature list to lie.

use alloc::collections::BTreeSet;

use crate::module::IrModule;
use crate::pipeline::{PipelineStep, StepRun};
use crate::tool_impl::ToolImpl;

/// A capability the IR can require of an executing backend. `#[non_exhaustive]`
/// so new IR shapes can extend it without a breaking change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IrFeature {
    /// An engine-sequenced `workflow.pipeline` is present.
    Pipeline,
    /// A `StepRun::Branch` block.
    Branch,
    /// A `StepRun::Parallel` block.
    Parallel,
    /// A `StepRun::Loop` block.
    Loop,
    /// A `StepRun::Suspend` block.
    Suspend,
    /// Postcondition checks (`workflow.checks` / `StepRun::Check`).
    Checks,
    /// Subflow edges or `ToolImpl::Subflow` tools.
    Subflow,
    /// MCP-contracted tools (`ToolImpl::Mcp`).
    McpTools,
    /// Statically-linked native tools (`ToolImpl::Native`).
    NativeTools,
    /// Deterministic step nodes (`workflow.steps` / `ToolImpl::Step`).
    DeterministicSteps,
    /// Trigger bindings.
    Triggers,
}

/// The set of [`IrFeature`]s this module requires an executing backend to
/// support. Walks the whole module, recursing nested control-flow bodies.
pub fn required_features(m: &IrModule) -> BTreeSet<IrFeature> {
    let mut f = BTreeSet::new();
    let wf = &m.workflow;

    if !m.triggers.is_empty() {
        f.insert(IrFeature::Triggers);
    }
    if !wf.checks.is_empty() {
        f.insert(IrFeature::Checks);
    }
    if !wf.edges.is_empty() {
        f.insert(IrFeature::Subflow);
    }
    if !wf.steps.is_empty() {
        f.insert(IrFeature::DeterministicSteps);
    }
    for tool in wf.tools.values() {
        match &tool.impl_ {
            ToolImpl::Native { .. } => {
                f.insert(IrFeature::NativeTools);
            }
            ToolImpl::Mcp { .. } => {
                f.insert(IrFeature::McpTools);
            }
            ToolImpl::Subflow { .. } => {
                f.insert(IrFeature::Subflow);
            }
            ToolImpl::Step { .. } => {
                f.insert(IrFeature::DeterministicSteps);
            }
        }
    }
    if let Some(pipeline) = &wf.pipeline {
        f.insert(IrFeature::Pipeline);
        for step in &pipeline.steps {
            walk_step(step, &mut f);
        }
    }
    f
}

/// Recurse one pipeline step, recording its feature and descending into any
/// nested bodies. Mirrors the typecheck walk (`validate_step_run`) so it
/// cannot miss what typecheck sees.
fn walk_step(step: &PipelineStep, f: &mut BTreeSet<IrFeature>) {
    match &step.run {
        StepRun::Agent(_) | StepRun::Tool(_) | StepRun::Deterministic(_) => {}
        StepRun::Check(_) => {
            f.insert(IrFeature::Checks);
        }
        StepRun::Branch { then, otherwise, .. } => {
            f.insert(IrFeature::Branch);
            for s in then.iter().chain(otherwise.iter()) {
                walk_step(s, f);
            }
        }
        StepRun::Parallel { branches } => {
            f.insert(IrFeature::Parallel);
            for branch in branches {
                for s in branch {
                    walk_step(s, f);
                }
            }
        }
        StepRun::Loop { body, .. } => {
            f.insert(IrFeature::Loop);
            for s in body {
                walk_step(s, f);
            }
        }
        StepRun::Suspend { .. } => {
            f.insert(IrFeature::Suspend);
        }
    }
}
```

- [ ] **Step 4: Wire `lib.rs`** — `pub mod feature;` + `pub use feature::{required_features, IrFeature};`

- [ ] **Step 5: Run + no_std check**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir feature
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir --no-default-features
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir/src/feature.rs crates/tau-ir/src/lib.rs
git commit -m "feat(tau-ir): IrFeature + walked required_features"
```

---

## Task 7: `backend_features` table + build-side profile (PR2)

**Files:**
- Modify: `crates/tau-ir/src/feature.rs` (add `backend_features`)

**Interfaces:**
- Produces: `tau_ir::feature::backend_features(AdapterFamily) -> BTreeSet<IrFeature>` — the canonical per-family support profile consulted by BOTH build (Task 8) and the load-gate equality guard (Task 9).

> **Design note (deviation from handoff, confirmed at grounding):** the wasm guest reuses the `tau-runtime-core` interpreter, so there is **one** backend feature set today. `backend_features` returns the interpreter set for every family; the `AdapterFamily` parameter is the seam where a genuinely-divergent future backend (e.g. a slimmed guest) would branch. The interpreter's own `SUPPORTED_FEATURES` const (Task 9) is asserted equal to `backend_features(Native)`, closing the drift gap.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn backend_omits_unimplemented_control_flow_today() {
    use tau_ports::target::adapter_family::AdapterFamily;
    let native = backend_features(AdapterFamily::Native);
    // Implemented today:
    assert!(native.contains(&IrFeature::Pipeline));
    assert!(native.contains(&IrFeature::Checks));
    // NOT implemented until EPIC 4.2:
    assert!(!native.contains(&IrFeature::Branch));
    assert!(!native.contains(&IrFeature::Parallel));
    assert!(!native.contains(&IrFeature::Loop));
    assert!(!native.contains(&IrFeature::Suspend));
    // One interpreter today ⇒ every family maps to the same set.
    assert_eq!(native, backend_features(AdapterFamily::Wasi));
    assert_eq!(native, backend_features(AdapterFamily::Passthrough));
}
```

- [ ] **Step 2: Run, verify fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir backend`
Expected: FAIL (unresolved `backend_features`).

- [ ] **Step 3: Implement**

```rust
use tau_ports::target::adapter_family::AdapterFamily;

/// The features an executing backend for `family` supports today. There is a
/// single backend (the `tau-runtime-core` interpreter) behind every target,
/// so every family maps to the same set until a divergent backend ships.
/// EPIC 4.2 (#399) adds `Branch`/`Parallel`/`Loop`/`Suspend` here.
pub fn backend_features(family: AdapterFamily) -> BTreeSet<IrFeature> {
    let interpreter = || {
        let mut f = BTreeSet::new();
        for x in [
            IrFeature::Pipeline,
            IrFeature::Checks,
            IrFeature::Subflow,
            IrFeature::McpTools,
            IrFeature::NativeTools,
            IrFeature::DeterministicSteps,
            IrFeature::Triggers,
        ] {
            f.insert(x);
        }
        f
    };
    match family {
        AdapterFamily::Native
        | AdapterFamily::Container
        | AdapterFamily::Remote
        | AdapterFamily::Wasi
        | AdapterFamily::Passthrough => interpreter(),
    }
}
```

(Confirm the import path `tau_ports::target::adapter_family::AdapterFamily` resolves; if `AdapterFamily` is re-exported at `tau_ports::target`, prefer that.)

- [ ] **Step 4: Run + no_std check**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir --no-default-features
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/feature.rs
git commit -m "feat(tau-ir): backend_features support profile keyed by adapter family"
```

---

## Task 8: build-time feature-fit (PR2)

**Files:**
- Create: `crates/tau-ir-lower/src/lower/feature_fit.rs`
- Modify: `crates/tau-ir-lower/src/lower.rs` (or wherever `lower_project` assembles the module + calls `capability_fit`) — call `feature_fit::check(&module, target)` after the module is assembled, before `Ok(module)`
- Modify: `crates/tau-ir-lower/src/error.rs` (add `LowerError::FeatureFitFailed`)
- Modify: `crates/tau-ir-lower/src/lower.rs` (module decl `mod feature_fit;`)

**Interfaces:**
- Consumes: `tau_ir::feature::{required_features, backend_features}` (Tasks 6–7).
- Produces: `LowerError::FeatureFitFailed { unsupported: Vec<IrFeature>, target: TargetTriple }`; build fails via the existing `lower_error → exit 2` path (`build.rs:116-119`).

- [ ] **Step 1: Add the error variant** in `crates/tau-ir-lower/src/error.rs`:

```rust
    /// The workflow uses IR feature(s) the build target's backend does not
    /// support (walked feature-fit). Strict — no override flag, matching the
    /// `CapabilityFitFailed` precedent.
    #[error("workflow requires unsupported IR feature(s) on target {target:?}: {unsupported:?}")]
    FeatureFitFailed {
        /// The features required but not supported by the target's backend.
        unsupported: alloc::vec::Vec<tau_ir::feature::IrFeature>,
        /// The build target.
        target: tau_ports::target::TargetTriple,
    },
```

(Add `use` imports as needed; `TargetTriple` may already be importable — mirror `capability_fit`'s usage.)

- [ ] **Step 2: Write the failing test** in `feature_fit.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::ids::{AgentId, PipelineStepId};
    use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
    use tau_ir::check::{Condition, GoalPredicate, Locus};
    use tau_ports::target::registry;

    #[test]
    fn branch_module_is_rejected_today() {
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow {
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("b".into()),
                        run: StepRun::Branch {
                            on: Condition { evaluates: Locus::Path("/x".into()), predicate: GoalPredicate::Exists },
                            then: alloc::vec![],
                            otherwise: alloc::vec![],
                        },
                        input: "${input}".into(),
                    }],
                }),
                ..Workflow::default()
            },
            triggers: alloc::vec::Vec::new(),
        };
        assert!(matches!(check(&m, &target), Err(LowerError::FeatureFitFailed { .. })));
    }

    #[test]
    fn agent_only_module_passes() {
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        };
        assert!(check(&m, &target).is_ok());
    }
}
```

- [ ] **Step 3: Run, verify fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower feature_fit`
Expected: FAIL (unresolved `check`).

- [ ] **Step 4: Implement `feature_fit.rs`**

```rust
//! Strict build-time IR feature-fit (mirrors `capability_fit`). Refuses the
//! build if the workflow walks any [`IrFeature`] the target's backend does
//! not support. **No override flag.**

use alloc::vec::Vec;
use tau_ir::feature::{backend_features, required_features, IrFeature};
use tau_ir::module::IrModule;
use tau_ports::target::TargetTriple;

use crate::error::LowerError;

/// Returns `Ok(())` iff every feature the module requires is supported by the
/// target's backend profile. On a miss, `Err(LowerError::FeatureFitFailed)`
/// with the full unsupported set.
pub(super) fn check(module: &IrModule, target: &TargetTriple) -> Result<(), LowerError> {
    let supported = backend_features(target.adapter_family);
    let required = required_features(module);
    let unsupported: Vec<IrFeature> = required.difference(&supported).copied().collect();
    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(LowerError::FeatureFitFailed {
            unsupported,
            target: *target,
        })
    }
}
```

- [ ] **Step 5: Call it in `lower_project`.** Find where the assembled `IrModule` is returned (grep `Ok(module)` / the end of `lower_project` in `crates/tau-ir-lower/src/lower.rs`), and insert before the return:

```rust
    feature_fit::check(&module, target)?;
    Ok(module)
```

Add `mod feature_fit;` next to `mod capability_fit;`.

- [ ] **Step 6: Run**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir-lower --no-default-features
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir-lower/src
git commit -m "feat(tau-ir-lower): strict build-time IR feature-fit"
```

---

## Task 9: load-time feature-fit at the interpreter chokepoint (PR2)

**Files:**
- Create: `crates/tau-runtime-core/src/interpreter/feature_gate.rs` (or inline in `interpreter/mod.rs`)
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs` (`run_ir` + `run_ir_streaming` — gate at the top)
- Modify: `crates/tau-runtime-core/src/error.rs` (add `RuntimeError::UnsupportedFeature`)
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs` (`SUPPORTED_FEATURES` const + drift-guard test)

**Interfaces:**
- Consumes: `tau_ir::feature::{required_features, backend_features, IrFeature}`.
- Produces: `RuntimeError::UnsupportedFeature { features: Vec<...> }`; gated at both interpreter entry points ⇒ covers native CLI (`run_via_ir`) and wasm guest (`guest.rs:130`).

- [ ] **Step 1: Add the error variant** in `crates/tau-runtime-core/src/error.rs` (mirror an existing variant's shape):

```rust
    /// The module walks IR feature(s) this interpreter does not implement yet.
    /// Caught at load (before stepping), replacing the mid-run `Internal`
    /// control-flow errors as the user-facing surface.
    #[error("workflow requires IR feature(s) this runtime does not support: {features:?}")]
    UnsupportedFeature {
        /// The unsupported features, as debug names.
        features: alloc::vec::Vec<alloc::string::String>,
    },
```

- [ ] **Step 2: Write the failing test** in `interpreter/mod.rs` (or `feature_gate.rs`)

```rust
#[cfg(test)]
mod feature_gate_tests {
    use super::*;
    use tau_ports::target::adapter_family::AdapterFamily;

    #[test]
    fn supported_features_matches_shared_table() {
        // Drift guard: the interpreter's const must equal the shared
        // build-time profile for its adapter family.
        let shared = tau_ir::feature::backend_features(AdapterFamily::Native);
        let ours: alloc::collections::BTreeSet<_> =
            SUPPORTED_FEATURES.iter().copied().collect();
        assert_eq!(ours, shared);
    }
}
```

Plus the **honesty test** (feature ∈ set ⇒ executes past load; feature ∉ set ⇒ rejected AT LOAD). For the ∉ case, drive `run_ir` with a Branch module and assert `Err(RuntimeError::UnsupportedFeature { .. })` (not `Internal`). Reuse the crate's existing interpreter test harness/mock dispatcher (grep the test module in `interpreter/` for the mock `ToolDispatcher`). Write the ∉ case concretely against that harness:

```rust
    #[test]
    fn branch_module_rejected_at_load_not_mid_run() {
        // Build a one-step Branch pipeline module (see canonical.rs tests for
        // the shape), wrap in Arc, call run_ir with the mock dispatcher, and
        // assert the error is UnsupportedFeature, proving the load gate fires
        // before the mid-run Internal arm.
    }
```

- [ ] **Step 3: Run, verify fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core feature_gate`
Expected: FAIL.

- [ ] **Step 4: Implement the const + gate.** In `interpreter/mod.rs`:

```rust
/// The IR features this interpreter implements. EPIC 4.2 (#399) adds
/// `Branch`/`Parallel`/`Loop`/`Suspend`. Kept in sync with
/// `tau_ir::feature::backend_features` by `supported_features_matches_shared_table`.
pub const SUPPORTED_FEATURES: &[tau_ir::feature::IrFeature] = &[
    tau_ir::feature::IrFeature::Pipeline,
    tau_ir::feature::IrFeature::Checks,
    tau_ir::feature::IrFeature::Subflow,
    tau_ir::feature::IrFeature::McpTools,
    tau_ir::feature::IrFeature::NativeTools,
    tau_ir::feature::IrFeature::DeterministicSteps,
    tau_ir::feature::IrFeature::Triggers,
];

fn ensure_supported(module: &IrModule) -> Result<(), RuntimeError> {
    use alloc::string::ToString;
    let supported: alloc::collections::BTreeSet<_> = SUPPORTED_FEATURES.iter().copied().collect();
    let required = tau_ir::feature::required_features(module);
    let missing: alloc::vec::Vec<_> = required
        .difference(&supported)
        .map(|f| alloc::format!("{f:?}"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::UnsupportedFeature { features: missing })
    }
}
```

Insert `ensure_supported(&module)?;` as the first line of both `run_ir` and `run_ir_streaming` (before the entry lookup).

- [ ] **Step 5: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS. (The existing mid-run `Internal` control-flow tests, if any, may now be unreachable — if a test asserted the mid-run message, update it to expect `UnsupportedFeature` at load, or keep both: the mid-run arms remain as defense-in-depth.)

- [ ] **Step 6: no_std check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core --no-default-features`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-runtime-core/src
git commit -m "feat(tau-runtime-core): load-time IR feature-fit at the interpreter chokepoint"
```

---

## Task 10: wasm-lane parity (PR2)

**Files:**
- Modify/Add: `crates/tau-wasm-guest/tests/` or the wasm-host integration test (`crates/tau-wasm-host/tests/`) — assert a Branch module is rejected across the WIT boundary as `Err(String)` containing the feature name.

**Interfaces:**
- Consumes: the load gate (Task 9) fires inside `run_ir_streaming`, which the guest calls; the error propagates as `e.to_string()` on the `Err(string)` WIT arm (`guest.rs`).

- [ ] **Step 1: Locate the existing wasm round-trip test** (grep `run(` / `BAKED_IR` under `crates/tau-wasm-host/tests` and `crates/tau-wasm-guest`). Confirm how a baked IR module is provided to the guest in tests.

- [ ] **Step 2: Write the parity test** — bake a Branch module, invoke the guest `run`, assert `Err` whose string contains `"Branch"` (or the `UnsupportedFeature` prose). If baking requires a build step, mirror `cmd_build_wasm.rs`'s harness.

- [ ] **Step 3: Run** (wasm lane per its existing CI invocation — confirm the target)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-wasm-host crates/tau-wasm-guest
git commit -m "test(wasm): native/guest parity — feature-fit rejection across the WIT boundary"
```

---

## Task 11: ADR update + #399 note + open PR2 (PR2)

**Files:**
- Modify: `docs/decisions/0059-ir-format-acceptance-window.md` (fill in the feature-fit half if PR1 left it as a stub; document the single-interpreter deviation and the `SUPPORTED_FEATURES` drift guard)

**Interfaces:** none.

- [ ] **Step 1: Finalize ADR-0059** — add the walked-feature-fit section, the build+load enforcement, the "one interpreter today" finding, and the honesty/drift-guard tests. State explicitly that EPIC 4.2 (#399) must add the four control-flow variants to `SUPPORTED_FEATURES` + `backend_features` and that the honesty test will fail until it does.

- [ ] **Step 2: Note in issue #399** (comment via `gh issue comment 399`) that the feature-set honesty test in `tau-runtime-core` and `backend_features` in `tau-ir` must be flipped when 4.2 lands control-flow execution.

- [ ] **Step 3: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book`
Expected: clean.

- [ ] **Step 4: Commit + open PR2**

```bash
git add docs/decisions/0059-ir-format-acceptance-window.md
git commit -m "docs: ADR-0059 feature-fit half + single-interpreter finding"
git push -u origin feat/ir-feature-fit
gh pr create --base feat/ir-format-acceptance-window --title "feat(ir): walked IrFeature fit at build + load" --body "<summary; stacked on #<PR1>>"
```

---

## Self-Review

**Spec coverage:**
- Version acceptance window (peek → gate) → Task 1. ✅
- Closed decode / `deny_unknown_fields` (+ untagged caution, N/A today) → Task 2. ✅
- All load sites wired → Task 3. ✅
- Conformance fixtures + README refresh → Task 4. ✅
- ADR-0059 + SUMMARY → Tasks 5, 11. ✅
- `IrFeature` + walked `required_features` → Task 6. ✅
- Backend support set + build-time fit → Tasks 7, 8. ✅
- Load-time fit replacing mid-run `Internal` (defense-in-depth kept) → Task 9. ✅
- Feature-set honesty test (drift guard) → Task 9. ✅
- Wasm-lane parity across WIT → Task 10. ✅
- `no_std` isolated checks → Tasks 1,2,6,7,8,9. ✅
- Version window truth table (equal/lower minor, minor+1, major±1) → Task 1. ✅

**Deviations from handoff (flagged for reviewer):**
1. **One backend, not two.** The guest reuses the `tau-runtime-core` interpreter, so there is a single `SUPPORTED_FEATURES` + one load chokepoint (`run_ir`/`run_ir_streaming`), not separate interpreter/guest sets. The guest's single-agent limit is orthogonal and unchanged.
2. **No `error[IRxxx]` codes** (Decision 1a) — prose `thiserror` messages.
3. **Guest error stays `result<string,string>`** (Decision 2a) — no WIT change.
4. Build-time fit lives in `tau-ir-lower` (next to `capability_fit`), riding the existing `lower_error → exit 2` path, rather than a fresh hook in `build.rs`.

**Placeholder scan:** Tasks 3/9/10 contain intentionally-descriptive test stubs where the exact harness must be read at impl time (existing bundle-test / mock-dispatcher / wasm-bake harnesses) — each names the file to mirror. All novel production code is fully literal.

**Type consistency:** `from_canonical_bytes -> Result<IrModule, DecodeError>` (Tasks 1–3); `required_features(&IrModule) -> BTreeSet<IrFeature>` (Tasks 6,8,9); `backend_features(AdapterFamily) -> BTreeSet<IrFeature>` (Tasks 7,8,9); `LowerError::FeatureFitFailed { unsupported: Vec<IrFeature>, target: TargetTriple }` (Task 8); `RuntimeError::UnsupportedFeature { features: Vec<String> }` (Task 9). Consistent across tasks.
