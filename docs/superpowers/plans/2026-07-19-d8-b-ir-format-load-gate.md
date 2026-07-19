# D8-B IR Format Load Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tau_ir::from_canonical_bytes` version-aware — reject a bundle whose IR-format *major* differs from this tau with a typed, actionable `IrError`, before attempting a full structural decode.

**Architecture:** A two-phase decode inside the single choke point `from_canonical_bytes`. Phase 1 deserializes a one-field `VersionPeek` to read `ir_format` and compare its major against the running tau's; a mismatch returns `IrError::FormatMajorMismatch` without touching the full shape. Phase 2 does the normal `IrModule` decode only when the major matches. Same major (any minor/patch) always decodes — honoring the semver forward-compat contract.

**Tech Stack:** Rust, `tau-ir` crate (`#![no_std]` + `alloc`), `serde` / `serde_json` (alloc path already in use), `thiserror`, `cargo nextest`.

## Global Constraints

- **CARGO RULES (project CLAUDE.md):** every cargo command MUST be `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`. Never bare `cargo`, never `--workspace`, always `-p`. Timeouts: test 300s, build/check 180s, clippy 240s.
- **Tests via nextest** (`cargo nextest run -p <crate>`), doctests via `cargo test --doc -p <crate>`.
- **`tau-ir` is `#![no_std]` + `alloc`.** No `std`-only APIs in non-test code. Error payloads are `String`/`alloc` only — never carry `serde_json::Error` by value (it is not `no_std`-portable as a stored field). Test code (`#[cfg(test)]`) may use `std` and `serde_json::Value`.
- **No `ir_format` bump.** Canonical *output* bytes stay byte-identical; `IrFormatVersion::CURRENT` stays `"v2.4.0"`. No schema regen, no golden changes.
- **Conventional commits**, imperative, scoped `feat(d8-b): …`. Branch: `feat/d8-b-ir-format-load-gate` (already checked out).
- **Semver gate rule:** reject iff `major(bundle) != major(current)`. Same major → always decode. Parse "major" = strip optional leading `v`, take the integer before the first `.`.

---

### Task 1: `IrError` variants + `IrFormatVersion::major()`

**Files:**
- Modify: `crates/tau-ir/src/error.rs` (add 3 variants to the `IrError` enum, before the closing `}` at line ~205)
- Modify: `crates/tau-ir/src/module.rs` (add `CURRENT_MAJOR` const + `major()` method to `impl IrFormatVersion`, ~line 40-45; add a `#[cfg(test)]` module or extend an existing one for the truth table)

**Interfaces:**
- Produces (used by Task 2):
  - `IrError::Decode(String)`
  - `IrError::FormatMajorMismatch { bundle: String, current: String, bundle_major: u64 }`
  - `IrError::FormatUnparseable { value: String }`
  - `IrFormatVersion::CURRENT_MAJOR: u64` (== `2`)
  - `IrFormatVersion::major(&self) -> Result<u64, ()>`

- [ ] **Step 1: Write the failing tests for `major()` + `CURRENT_MAJOR`**

Add to `crates/tau-ir/src/module.rs` (inside the existing `#[cfg(test)] mod tests` if present, else add one). The existing test module already asserts `IrFormatVersion::CURRENT` — put these beside it:

```rust
    #[test]
    fn major_parses_leading_v_and_bare() {
        assert_eq!(IrFormatVersion("v2.4.0".into()).major(), Ok(2));
        assert_eq!(IrFormatVersion("2.4.0".into()).major(), Ok(2));
        assert_eq!(IrFormatVersion("v10.0.0".into()).major(), Ok(10));
    }

    #[test]
    fn major_rejects_malformed() {
        assert_eq!(IrFormatVersion("".into()).major(), Err(()));
        assert_eq!(IrFormatVersion("garbage".into()).major(), Err(()));
        assert_eq!(IrFormatVersion("v.4".into()).major(), Err(()));
        assert_eq!(IrFormatVersion("v".into()).major(), Err(()));
    }

    #[test]
    fn current_major_agrees_with_current_string() {
        // A future CURRENT bump must not silently desync CURRENT_MAJOR.
        assert_eq!(
            IrFormatVersion::current().major(),
            Ok(IrFormatVersion::CURRENT_MAJOR)
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir major_parses -E 'test(major) + test(current_major)'`
Expected: FAIL to compile — `no method named major`, `no associated const CURRENT_MAJOR`.

- [ ] **Step 3: Implement `CURRENT_MAJOR` + `major()`**

In `crates/tau-ir/src/module.rs`, inside `impl IrFormatVersion` (after `CURRENT`, before/after `current()`):

```rust
    /// Major component of `CURRENT`. Kept as a literal (const string
    /// parsing is awkward) and pinned to `CURRENT` by
    /// `current_major_agrees_with_current_string`.
    pub const CURRENT_MAJOR: u64 = 2;

    /// Parse the semver MAJOR out of the version string.
    ///
    /// Strips an optional leading `v`, then parses the integer before the
    /// first `.`. Returns `Err(())` for any string that does not start
    /// `[v]<digits>.`.
    pub fn major(&self) -> Result<u64, ()> {
        let s = self.0.strip_prefix('v').unwrap_or(&self.0);
        let head = s.split('.').next().ok_or(())?;
        if head.is_empty() {
            return Err(());
        }
        head.parse::<u64>().map_err(|_| ())
    }
```

- [ ] **Step 4: Write the failing test for the new `IrError` variants**

Add to `crates/tau-ir/src/error.rs` a `#[cfg(test)]` module at the end of the file (the file currently has no test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn format_major_mismatch_renders() {
        let e = IrError::FormatMajorMismatch {
            bundle: "v3.0.0".into(),
            current: "v2.4.0".into(),
            bundle_major: 3,
        };
        let msg = e.to_string();
        assert!(msg.contains("major 3"), "got: {msg}");
        assert!(msg.contains("v2.4.0"), "got: {msg}");
        assert!(msg.contains("rebuild"), "got: {msg}");
    }

    #[test]
    fn format_unparseable_renders() {
        let e = IrError::FormatUnparseable { value: "garbage".into() };
        assert!(e.to_string().contains("garbage"));
    }

    #[test]
    fn decode_renders() {
        let e = IrError::Decode("expected value at line 1".into());
        assert!(e.to_string().contains("not valid JSON"));
    }
}
```

- [ ] **Step 5: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -E 'test(format_major) + test(format_unparseable) + test(decode_renders)'`
Expected: FAIL to compile — `no variant named FormatMajorMismatch`.

- [ ] **Step 6: Add the three `IrError` variants**

In `crates/tau-ir/src/error.rs`, add before the closing `}` of `pub enum IrError` (after the last variant `UnknownCheckLocus`, ~line 204):

```rust
    /// Canonical IR bytes are not valid JSON (structural decode failure).
    /// Carries a stringified `serde_json::Error` (the crate is `no_std`;
    /// the error is not stored by value).
    #[error("canonical IR is not valid JSON: {0}")]
    Decode(String),

    /// The IR-format MAJOR of the loaded bundle does not match the MAJOR
    /// this `tau` emits. A breaking IR-shape change: the bundle must be
    /// rebuilt with a compatible `tau`.
    #[error(
        "IR format major {bundle_major} is incompatible with this tau \
         (emits {current}); rebuild with a matching tau"
    )]
    FormatMajorMismatch {
        /// The bundle's full `ir_format` string, e.g. `v3.0.0`.
        bundle: String,
        /// This tau's `ir_format`, e.g. `v2.4.0`.
        current: String,
        /// The parsed major of `bundle`.
        bundle_major: u64,
    },

    /// The bundle's `ir_format` string is not a valid `vMAJOR.MINOR.PATCH`.
    #[error("IR format version {value:?} is not a valid vMAJOR.MINOR.PATCH string")]
    FormatUnparseable {
        /// The offending `ir_format` value as read from the bundle.
        value: String,
    },
```

Ensure `use alloc::string::String;` is present at the top of `error.rs` (it already is, line 4).

- [ ] **Step 7: Run all Task-1 tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -E 'test(major) + test(current_major) + test(format_major) + test(format_unparseable) + test(decode_renders)'`
Expected: PASS (6 tests).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ir/src/error.rs crates/tau-ir/src/module.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(d8-b): add IrError load-gate variants and IrFormatVersion::major()"
```

---

### Task 2: Two-phase `from_canonical_bytes` with the major gate

**Files:**
- Modify: `crates/tau-ir/src/canonical.rs` (rewrite `from_canonical_bytes`, lines 29-33; update the module doc lines 1-11 that describe decode; add a `VersionPeek` private struct; add a gate test module)

**Interfaces:**
- Consumes (from Task 1): `IrError::{Decode, FormatMajorMismatch, FormatUnparseable}`, `IrFormatVersion::{CURRENT, CURRENT_MAJOR, major}`.
- Produces (used by Task 3 callers, all already present): `pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, IrError>` (return type changed from `Result<IrModule, serde_json::Error>`).

- [ ] **Step 1: Write the failing gate tests**

Add a new test module at the end of `crates/tau-ir/src/canonical.rs`. These build a real module, serialize it, then rewrite only the `ir_format` string via `serde_json::Value` surgery so no real v3 shape is needed:

```rust
#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::module::{IrFormatVersion, IrModule};
    use crate::error::IrError;
    use alloc::string::{String, ToString};
    use tau_ports::target::registry;

    /// A minimal, valid v2.4.0 module serialized to canonical bytes.
    fn base_module() -> IrModule {
        let target = registry::list_available().next().unwrap().triple;
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: crate::module::Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        }
    }

    /// Serialize `base_module`, then overwrite its `ir_format` field with
    /// `version` and return the bytes.
    fn bytes_with_version(version: &str) -> alloc::vec::Vec<u8> {
        let m = base_module();
        let mut v: serde_json::Value =
            serde_json::from_slice(&to_canonical_bytes(&m)).unwrap();
        v["ir_format"] = serde_json::Value::String(version.to_string());
        serde_json::to_vec(&v).unwrap()
    }

    #[test]
    fn same_major_decodes_ok() {
        for ver in ["v2.4.0", "v2.3.0", "v2.5.0", "v2.9.9"] {
            let out = from_canonical_bytes(&bytes_with_version(ver));
            assert!(out.is_ok(), "{ver} should decode: {out:?}");
        }
    }

    #[test]
    fn major_mismatch_is_rejected() {
        for ver in ["v3.0.0", "v1.0.0"] {
            match from_canonical_bytes(&bytes_with_version(ver)) {
                Err(IrError::FormatMajorMismatch { bundle, .. }) => {
                    assert_eq!(bundle, ver);
                }
                other => panic!("{ver} expected FormatMajorMismatch, got {other:?}"),
            }
        }
    }

    #[test]
    fn unparseable_version_is_rejected() {
        for ver in ["garbage", "", "v.4"] {
            match from_canonical_bytes(&bytes_with_version(ver)) {
                Err(IrError::FormatUnparseable { value }) => assert_eq!(value, ver),
                other => panic!("{ver:?} expected FormatUnparseable, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_additive_field_is_tolerated() {
        // Forward-compat: a same-major bundle carrying a field this tau
        // does not know (future D6-B `assets`) still decodes.
        let m = base_module();
        let mut v: serde_json::Value =
            serde_json::from_slice(&to_canonical_bytes(&m)).unwrap();
        v["ir_format"] = serde_json::Value::String("v2.5.0".to_string());
        v["assets"] = serde_json::json!({ "logo": "deadbeef" });
        let bytes = serde_json::to_vec(&v).unwrap();
        assert!(from_canonical_bytes(&bytes).is_ok());
    }

    #[test]
    fn valid_version_bad_body_is_decode_error() {
        // ir_format ok, but the body is not an IrModule.
        let bytes = br#"{"ir_format":"v2.4.0","nonsense":true}"#;
        match from_canonical_bytes(bytes) {
            Err(IrError::Decode(_)) => {}
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn not_even_json_is_decode_error() {
        match from_canonical_bytes(b"not json at all") {
            Err(IrError::Decode(_)) => {}
            other => panic!("expected Decode, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -E 'test(gate_tests)'`
Expected: FAIL to compile — `from_canonical_bytes` still returns `serde_json::Error`, so the `IrError` match arms don't type-check.

- [ ] **Step 3: Rewrite `from_canonical_bytes` + `VersionPeek`**

In `crates/tau-ir/src/canonical.rs`, replace the current `from_canonical_bytes` (lines 29-33) and add the peek struct + imports. New code:

```rust
use crate::error::IrError;
use crate::module::IrFormatVersion;
use serde::Deserialize;

/// Phase-1 peek: read only `ir_format` from the canonical bytes. serde
/// ignores every other field, so this is shape-independent — it decodes
/// even when the full `IrModule` shape has changed across a major.
#[derive(Deserialize)]
struct VersionPeek {
    ir_format: IrFormatVersion,
}

/// Deserialize canonical bytes back to an `IrModule`, gated on IR-format
/// MAJOR compatibility.
///
/// Two-phase:
/// 1. Decode `VersionPeek` to read `ir_format`; compare its major against
///    [`IrFormatVersion::CURRENT_MAJOR`]. A different major is a breaking
///    IR-shape change → [`IrError::FormatMajorMismatch`] (the full decode
///    is never attempted). An unparseable version →
///    [`IrError::FormatUnparseable`].
/// 2. Same major → full `IrModule` decode; a serde failure maps to
///    [`IrError::Decode`].
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, IrError> {
    use alloc::string::ToString;

    // Phase 1: peek the version.
    let peek: VersionPeek =
        serde_json::from_slice(bytes).map_err(|e| IrError::Decode(e.to_string()))?;
    let bundle_major = peek.ir_format.major().map_err(|()| {
        IrError::FormatUnparseable { value: peek.ir_format.0.clone() }
    })?;
    if bundle_major != IrFormatVersion::CURRENT_MAJOR {
        return Err(IrError::FormatMajorMismatch {
            bundle: peek.ir_format.0.clone(),
            current: IrFormatVersion::CURRENT.to_string(),
            bundle_major,
        });
    }

    // Phase 2: full structural decode.
    serde_json::from_slice(bytes).map_err(|e| IrError::Decode(e.to_string()))
}
```

Note: `IrFormatVersion` field `.0` is the raw `String` (tuple struct, `module.rs:26`).

- [ ] **Step 4: Fix the lying module doc**

Replace `crates/tau-ir/src/canonical.rs` lines 1-11 (the doc block that falsely claims fixed field order / BTreeMap sorting / `None → null` no-skip). Corrected doc:

```rust
//! Deterministic serialization of an `IrModule` to canonical bytes.
//!
//! `to_canonical_bytes` writes the derived `serde_json` compact encoding:
//! fields in Rust declaration order, `BTreeMap` fields alphabetically,
//! `skip_serializing_if` honored (absent optional fields are omitted, not
//! `null`). Two successive calls yield identical bytes.
//!
//! `from_canonical_bytes` is version-gated (D8-B): it rejects a bundle whose
//! `ir_format` MAJOR differs from this crate's before attempting a full decode.
//!
//! NOTE: the canonical form is scheduled to become a specification (sorted-key
//! compact JSON) under D9-C / `ir_format 3.0.0`; until then it is the derived
//! serde encoding described above.
```

(This doc now matches reality; do not claim sorting that the code does not do.)

- [ ] **Step 5: Run the Task-2 tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -E 'test(gate_tests)'`
Expected: PASS (6 tests).

- [ ] **Step 6: Run the full `tau-ir` test suite (regression: existing round-trip tests still green)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Then doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir`
Expected: PASS. The pre-existing `pipeline_canonical_tests` (`.expect("round-trips")`) still pass because they use `IrFormatVersion::current()` (major 2).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir/src/canonical.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(d8-b): version-gate from_canonical_bytes (two-phase major check)"
```

---

### Task 3: Verify downstream consumers compile and pass

No consumer code changes are required — all three callers format the error via `Debug`/`Display`, which `thiserror` provides on `IrError`. This task proves that.

**Files:**
- Verify only (no edits expected): `crates/tau-cli/src/cmd/run.rs:114`, `crates/tau-wasm-guest/src/guest.rs:104`, `crates/tau-ir-conformance/src/bundle_mode.rs:169`.

**Interfaces:**
- Consumes: `from_canonical_bytes(bytes) -> Result<IrModule, IrError>` (Task 2).

- [ ] **Step 1: Build the three consumers against the new signature**

Run each (separate target dir already shared via sccache):

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir-conformance
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-wasm-guest
```

Expected: all three PASS. `run.rs` uses `{e:?}` (Debug ✓), `guest.rs` uses `e.to_string()` (Display ✓), `bundle_mode.rs` uses `{e}` (Display ✓).

- [ ] **Step 2: If any consumer fails to compile, fix the error arm minimally**

Only if Step 1 fails: adjust the offending `.map_err`/`match` to use `{e}` or `{e:?}` (both available on `IrError`). Do **not** change control flow. Re-run the failing `cargo check`. (Expected: not needed.)

- [ ] **Step 3: Run the conformance test suite (exercises the real load path)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
Expected: PASS — bundle-mode decode of a real `v2.4.0` bundle still succeeds (same major).

- [ ] **Step 4: Clippy the touched crate**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir --all-targets`
Expected: no warnings (workspace denies warnings in CI).

- [ ] **Step 5: Commit any consumer fixes (only if Step 2 ran)**

```bash
git add -A
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(d8-b): update IR-decode error arms for IrError return type"
```

If Step 2 did not run, skip this commit — Task 3 is verification-only.

---

## Post-plan: PR

After all tasks green, open the PR against `main`:

```bash
git push -u origin feat/d8-b-ir-format-load-gate
gh pr create --base main --title "feat(d8-b): IR format load gate (version-aware from_canonical_bytes)" \
  --body "<summary + link to spec; note this is the enabling prerequisite for D9-C ir_format 3.0.0; no ir_format bump>"
```

Enrol auto-merge per CLAUDE.md: `gh pr merge <PR#> --squash --delete-branch --auto`.

## Self-Review notes

- **Spec coverage:** major-only gate (Task 2), two-phase decode (Task 2), three `IrError` variants (Task 1), `major()` + `CURRENT_MAJOR` (Task 1), all six spec test cases (Tasks 1-2), caller impact table (Task 3), no ir_format bump / no schema-golden change (Global Constraints + Task 2 Step 6). Covered.
- **Type consistency:** `major() -> Result<u64, ()>`, `CURRENT_MAJOR: u64`, variant field names (`bundle`, `current`, `bundle_major`, `value`) identical across error.rs definition (Task 1) and canonical.rs construction (Task 2). Consistent.
- **No placeholders:** every code step shows full code; every run step shows the exact command + expected result.
