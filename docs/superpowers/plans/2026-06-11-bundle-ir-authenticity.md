# Bundle IR authenticity cross-check — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tau run --bundle` refuse to execute a v2 bundle whose embedded IR does not match what the local (verified) `tau.toml` lowers to, closing the S3 capability-escalation surface, and clarify the integrity-vs-authenticity language.

**Architecture:** `verify_bundle` (tau-pkg) gains a final step that compares the bundle's recorded `ir_payload.canonical_ir_hash` against a caller-supplied hash recomputed by re-lowering the cwd source. tau-cli owns the re-lowering (it has `lower_ir`); tau-pkg owns the comparison + typed errors (mirroring the existing `verify_reproducible` caller-supplied-`ir_payload` split). Fail-closed: a v2 bundle whose source cannot be re-lowered is refused.

**Tech Stack:** Rust (workspace crates `tau-pkg`, `tau-cli`), `thiserror`, `sha2`, `tau-ir` lowering, `assert_cmd`/`tempfile` for tests.

**CARGO RULES:** every cargo command in this plan uses the main-agent target dir and is scoped + timed + non-incremental, e.g.
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg`.
Subagents executing a task substitute `CARGO_TARGET_DIR=target/agent-impl` (or `target/agent-<role>`).

---

## Background (read before starting)

The gap and the design are in `docs/superpowers/specs/2026-06-11-bundle-ir-authenticity-design.md`. In one paragraph:

`tau run --bundle` (`crates/tau-cli/src/cmd/run.rs:78-120`) verifies a `.tau` bundle, then — if the bundle carries an `ir_payload` (a "v2" bundle) — decodes and executes that IR via `run_via_ir`. `verify_bundle` proves the cwd `tau.toml` matches a recorded hash (step 6) and that the IR bytes match *their own* stored hash (step 9), but nothing ties the executed IR to the `tau.toml` the user inspected. A malicious bundle can pair a benign `tau.toml` with an IR that wires up extra tools / capabilities, recompute the self-hash, and pass verification. This plan adds the missing edge: re-lower the verified source and require the recomputed canonical IR hash to equal the bundle's recorded one.

**Key ordering fact that keeps existing tests green:** the new check is **step 10 — the last step**. Existing `cmd_run_bundle.rs` failure tests (drift / self-hash / missing-install) fail at earlier steps and keep their current exit-3 reasons. The clean-fixture test re-lowers with the *same binary* that built the bundle (same `tau_version`, same deterministic native-tool cache, empty MCP cache), so the recomputed hash matches and it still passes — **as long as run.rs wires the real re-lowering** (never an interim `None`, which would turn every v2 bundle into `IrSourceUnverifiable`). Task 1 wires it in the same commit for this reason.

---

## File Structure

- `crates/tau-pkg/src/bundle/verify_error.rs` — **modify**: add 2 `VerifyError` variants.
- `crates/tau-pkg/src/bundle/verify.rs` — **modify**: add `recomputed_ir_hash` field to `VerifyOptions`, add `verify_ir_matches_source` step fn, call it as step 10, update in-crate `vopts` test helper, add unit tests.
- `crates/tau-pkg/tests/bundle_verify_e2e.rs` — **modify**: update 2 `VerifyOptions { .. }` literals for the new field.
- `crates/tau-cli/src/cmd/run.rs` — **modify**: extract `verify_bundle_against_source` helper (re-lower + verify), call it from the `--bundle` block, extend `bundle_verify_exit_code`, add the faithful divergence test module.
- `crates/tau-pkg/src/install.rs` — **modify (doc only)**: add the S2 trust-boundary note to the module doc-comment.
- `SECURITY.md` — **modify**: add a "Trust boundaries" subsection (S2 install + S3 bundle correspondence).

---

## Task 1: tau-pkg cross-check logic + tau-cli wiring (the feature)

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify_error.rs`
- Modify: `crates/tau-pkg/src/bundle/verify.rs`
- Modify: `crates/tau-pkg/tests/bundle_verify_e2e.rs`
- Modify: `crates/tau-cli/src/cmd/run.rs`

- [ ] **Step 1: Write the failing unit test for the comparison step**

In `crates/tau-pkg/src/bundle/verify.rs`, inside the existing `#[cfg(test)] mod tests`, add (the helper `build_minimal_bundle` already builds a v1 bundle; we attach a synthetic v2 `ir_payload` exactly like the existing `verify_detects_ir_payload_drift` test does):

```rust
/// Parse the minimal fixture bundle and attach a synthetic, internally
/// consistent v2 `ir_payload` (its `canonical_ir_hash` matches its
/// bytes). Returns the manifest + the genuine IR hash.
fn manifest_with_ir(root: &std::path::Path) -> (BundleManifest, String) {
    use crate::bundle::manifest::IrPayload;
    use sha2::{Digest, Sha256};
    let path = build_minimal_bundle(root);
    let s = std::fs::read_to_string(&path).unwrap();
    let mut m = BundleManifest::parse_str(&s).unwrap();
    let bytes: Vec<u8> = b"genuine ir bytes".to_vec();
    let mut h = Sha256::new();
    h.update(&bytes);
    let hash_bytes: [u8; 32] = h.finalize().into();
    let hash_hex = crate::tree_hash::to_hex_lower(&hash_bytes);
    m.ir_payload = Some(IrPayload {
        ir_format: "v1.0.0".to_string(),
        canonical_ir_hash: hash_hex.clone(),
        canonical_ir_bytes_hex: crate::tree_hash::to_hex_lower(&bytes),
    });
    (m, hash_hex)
}

#[test]
fn ir_xcheck_rejects_divergent_source_hash() {
    let tmp = tempdir().unwrap();
    let (m, _genuine) = manifest_with_ir(tmp.path());
    let err = verify_ir_matches_source(&m, Some("deadbeefdivergent")).unwrap_err();
    match err {
        VerifyError::IrSourceDivergence { bundle_hash, source_hash } => {
            assert_eq!(source_hash, "deadbeefdivergent");
            assert_eq!(bundle_hash, m.ir_payload.unwrap().canonical_ir_hash);
        }
        other => panic!("expected IrSourceDivergence, got {other:?}"),
    }
}

#[test]
fn ir_xcheck_accepts_matching_source_hash() {
    let tmp = tempdir().unwrap();
    let (m, genuine) = manifest_with_ir(tmp.path());
    verify_ir_matches_source(&m, Some(&genuine)).expect("matching hash must pass");
}

#[test]
fn ir_xcheck_fails_closed_when_source_unlowerable() {
    let tmp = tempdir().unwrap();
    let (m, _genuine) = manifest_with_ir(tmp.path());
    let err = verify_ir_matches_source(&m, None).unwrap_err();
    assert!(matches!(err, VerifyError::IrSourceUnverifiable), "got {err:?}");
}

#[test]
fn ir_xcheck_noop_for_v1_bundle() {
    let tmp = tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path()); // v1 — no ir_payload
    let s = std::fs::read_to_string(&path).unwrap();
    let m = BundleManifest::parse_str(&s).unwrap();
    assert!(m.ir_payload.is_none(), "fixture must be v1 for this test");
    // No IR to diverge: passes regardless of the recomputed-hash arg.
    verify_ir_matches_source(&m, None).expect("v1 + None must pass");
    verify_ir_matches_source(&m, Some("anything")).expect("v1 + Some must pass");
}
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg ir_xcheck`
Expected: compile error — `verify_ir_matches_source` not found, `VerifyError::IrSourceDivergence` / `IrSourceUnverifiable` not found.

- [ ] **Step 3: Add the two `VerifyError` variants**

In `crates/tau-pkg/src/bundle/verify_error.rs`, append inside the `enum VerifyError { ... }` (after `IrPayloadDrift`):

```rust
    /// The IR embedded in the bundle does not match what the local
    /// (verified) tau.toml lowers to — a capability/workflow divergence
    /// between the source the user inspected and the IR that would run.
    #[error("bundle IR diverges from the local tau.toml — bundle IR hash {bundle_hash}, but the source lowers to {source_hash}; the executed workflow does not match the inspected source")]
    IrSourceDivergence {
        /// Canonical IR hash recorded in the bundle's `ir_payload`.
        bundle_hash: String,
        /// Canonical IR hash recomputed by re-lowering the cwd source.
        source_hash: String,
    },

    /// The bundle carries an IR payload, but the local source could not be
    /// re-lowered to authenticate it; the run is refused (fail-closed).
    #[error("bundle carries an IR payload but the local tau.toml could not be re-lowered to authenticate it; refusing to run an unverifiable bundle")]
    IrSourceUnverifiable,
```

- [ ] **Step 4: Add the `recomputed_ir_hash` field to `VerifyOptions`**

In `crates/tau-pkg/src/bundle/verify.rs`, add the field to the `VerifyOptions` struct (after `project_root`):

```rust
    /// The canonical IR hash the caller recomputed by re-lowering the
    /// cwd source (tau-cli owns lowering — see the design doc's layering
    /// note). `None` means the caller could not lower the source; for a
    /// v2 bundle that is a fail-closed refusal (`IrSourceUnverifiable`).
    pub recomputed_ir_hash: Option<String>,
```

- [ ] **Step 5: Add the step fn and wire it in as step 10**

In `crates/tau-pkg/src/bundle/verify.rs`, add this function (place it next to `verify_ir_payload`):

```rust
/// Step 10: cross-check the bundle's embedded IR against the verified
/// source. `recomputed_ir_hash` is the canonical IR hash the caller
/// produced by re-lowering the cwd `tau.toml` (already proven byte-clean
/// by step 6).
///
/// This is the edge that turns the pipeline's *integrity* guarantee into
/// a *source-correspondence* guarantee: combined with step 9 (stored
/// hash == embedded IR bytes), a match here proves the executed IR is
/// exactly what the inspected `tau.toml` lowers to.
///
/// - v1 bundle (`ir_payload` is `None`): no IR to diverge — `Ok`.
/// - v2 bundle + `recomputed_ir_hash` `Some`: the hashes must be equal,
///   else [`VerifyError::IrSourceDivergence`].
/// - v2 bundle + `recomputed_ir_hash` `None`: the source could not be
///   re-lowered to authenticate the IR — fail closed with
///   [`VerifyError::IrSourceUnverifiable`].
fn verify_ir_matches_source(
    m: &BundleManifest,
    recomputed_ir_hash: Option<&str>,
) -> Result<(), VerifyError> {
    let Some(ir) = &m.ir_payload else {
        return Ok(()); // v1 bundle — nothing to cross-check.
    };
    match recomputed_ir_hash {
        Some(source_hash) => {
            if source_hash != ir.canonical_ir_hash {
                return Err(VerifyError::IrSourceDivergence {
                    bundle_hash: ir.canonical_ir_hash.clone(),
                    source_hash: source_hash.to_string(),
                });
            }
            Ok(())
        }
        None => Err(VerifyError::IrSourceUnverifiable),
    }
}
```

Then call it as the final step inside `verify_bundle`, immediately after step 9 (`verify_ir_payload(&manifest)?;`) and before `Ok(VerifyReport { ... })`:

```rust
    // Step 10: cross-check the embedded IR against the verified source.
    // Steps 6 + 9 prove the tau.toml and the IR bytes individually; this
    // ties them together so the executed IR cannot diverge from the
    // inspected source. See the fn doc for the v1 / fail-closed cases.
    verify_ir_matches_source(&manifest, opts.recomputed_ir_hash.as_deref())?;
```

- [ ] **Step 6: Update the in-crate `vopts` test helper**

In `crates/tau-pkg/src/bundle/verify.rs`, the `vopts` helper constructs `VerifyOptions`; add the new field so the existing tests (which exercise other steps) keep the cross-check inert:

```rust
    fn vopts(bundle_path: std::path::PathBuf, root: &std::path::Path) -> VerifyOptions {
        VerifyOptions {
            bundle_path,
            project_root: root.to_path_buf(),
            // Existing tests target steps 1–9; leave step 10 inert. The
            // happy-path fixture is v1 (no ir_payload), so None is a
            // no-op here, not a fail-closed refusal.
            recomputed_ir_hash: None,
        }
    }
```

- [ ] **Step 7: Run the tau-pkg unit tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg`
Expected: PASS, including the 4 new `ir_xcheck_*` tests and all pre-existing `verify_*` tests.

- [ ] **Step 8: Fix the integration-test call sites**

In `crates/tau-pkg/tests/bundle_verify_e2e.rs`, both `VerifyOptions { .. }` literals (around lines 119 and 151) need the new field. Add to each:

```rust
        recomputed_ir_hash: None,
```

(These e2e fixtures build v1 bundles — no `ir_payload` — so `None` is a no-op, matching their intent of exercising steps 6–8.)

- [ ] **Step 9: Run the tau-pkg e2e tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg --test bundle_verify_e2e`
Expected: PASS.

- [ ] **Step 10: Wire the real re-lowering into tau-cli `run.rs`**

In `crates/tau-cli/src/cmd/run.rs`, add this helper (place it near `bundle_verify_exit_code`):

```rust
/// Re-lower the cwd source and verify `bundle_path` against it.
///
/// Computes the canonical IR hash of the local `tau.toml` and hands it to
/// [`tau_pkg::bundle::verify_bundle`] so the bundle's embedded IR can be
/// cross-checked against the source it claims to come from (verify step
/// 10). Bundles are host-sealed (verify step 5 rejects a foreign target),
/// so lowering for the host target is correct: a foreign-target bundle is
/// rejected by step 5 before the divergence check is reached.
///
/// `lower_ir` returning `None` (the source no longer lowers) flows through
/// as `recomputed_ir_hash: None`, which verify_bundle turns into a
/// fail-closed `IrSourceUnverifiable` for a v2 bundle.
fn verify_bundle_against_source(
    cwd: &std::path::Path,
    bundle_path: &std::path::Path,
) -> Result<tau_pkg::bundle::VerifyReport, tau_pkg::bundle::VerifyError> {
    let empty_mcp_cache = std::collections::BTreeMap::new();
    let recomputed_ir_hash = crate::cmd::build::lower_ir(
        cwd,
        &tau_ports::target::TargetTriple::host(),
        &empty_mcp_cache,
        None,
    )
    .map(|p| p.canonical_ir_hash);

    tau_pkg::bundle::verify_bundle(tau_pkg::bundle::VerifyOptions {
        bundle_path: bundle_path.to_path_buf(),
        project_root: cwd.to_path_buf(),
        recomputed_ir_hash,
    })
}
```

Then, in the `--bundle` block, replace the inline call:

```rust
        match tau_pkg::bundle::verify_bundle(tau_pkg::bundle::VerifyOptions {
            bundle_path: bundle_path.clone(),
            project_root: cwd.clone(),
        }) {
```

with:

```rust
        match verify_bundle_against_source(&cwd, bundle_path) {
```

(The rest of the `match` arms — `Err(e) => { eprintln!(...); std::process::exit(bundle_verify_exit_code(&e)); }` and the `Ok(report) => { ... }` IR-dispatch block — are unchanged.)

- [ ] **Step 11: Extend the exit-code mapping**

In `crates/tau-cli/src/cmd/run.rs`, `bundle_verify_exit_code` exhaustively matches `VerifyError`; add the two new variants to the integrity/install-state `=> 3` arm (next to `IrPayloadDrift`):

```rust
        | V::IrPayloadDrift { .. }
        | V::IrSourceDivergence { .. }
        | V::IrSourceUnverifiable => 3,
```

- [ ] **Step 12: Run the tau-cli bundle tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-cli --test cmd_run_bundle --test cmd_verify_bundle`
Expected: PASS — all four `run_bundle_*` tests stay green (step 10 runs last; the clean fixture re-lowers to the same hash with the same binary).

- [ ] **Step 13: Commit**

```bash
git add crates/tau-pkg/src/bundle/verify_error.rs \
        crates/tau-pkg/src/bundle/verify.rs \
        crates/tau-pkg/tests/bundle_verify_e2e.rs \
        crates/tau-cli/src/cmd/run.rs
git commit -m "feat(tau-pkg): cross-check bundle IR against verified source (S3)

verify_bundle gains a final step that compares the bundle's recorded
canonical IR hash against a caller-supplied hash recomputed by
re-lowering the cwd tau.toml. tau-cli re-lowers (it owns lower_ir) and
passes the hash in; tau-pkg owns the comparison + typed errors
(IrSourceDivergence / IrSourceUnverifiable, both exit 3). Fail-closed:
a v2 bundle whose source cannot be re-lowered is refused.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 2: faithful real-lowering divergence test (tau-cli)

This proves the *actual* vulnerability shape from the finding: a bundle whose `tau.toml` hash matches the cwd but whose IR was lowered from a *different* source (extra capability) is rejected, and a genuine bundle passes. `tau_pkg::bundle::build` accepts an injected `ir_payload`, so we build an "A-source" bundle carrying "B-source" IR with no hash hand-editing.

**Files:**
- Modify: `crates/tau-cli/src/cmd/run.rs` (add a `#[cfg(test)] mod` at the end of the file)

- [ ] **Step 1: Write the failing test**

Append to `crates/tau-cli/src/cmd/run.rs`:

```rust
#[cfg(test)]
mod bundle_source_xcheck_tests {
    use super::verify_bundle_against_source;
    use std::collections::BTreeMap;
    use tau_pkg::bundle::{build, BuildOptions, IrPayload, VerifyError};
    use tau_ports::target::TargetTriple;

    /// Write a native-tool project whose single tool declares `caps`
    /// (a TOML capabilities array, e.g. `[]` or `[{ kind = "net.http" }]`).
    /// The capability set is what makes two otherwise-identical projects
    /// lower to different IR hashes.
    fn write_project(root: &std::path::Path, name: &str, caps: &str) {
        std::fs::write(
            root.join("tau.toml"),
            format!(
                r#"
[project]
name = "{name}"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "{name}@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"

[tools.read_temp]
native = "ReadTemp"
description = "reads the temperature"
capabilities = {caps}
"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
    }

    fn lower(root: &std::path::Path) -> IrPayload {
        let empty = BTreeMap::new();
        crate::cmd::build::lower_ir(root, &TargetTriple::host(), &empty, None)
            .expect("native-tool project must lower to Some(IrPayload)")
    }

    /// Build a bundle in `src_root` but embed `ir_payload`. When
    /// `ir_payload` comes from a *different* source tree, the bundle's
    /// recorded tau.toml hash still matches `src_root` (so verify steps
    /// 6/8 pass) while its IR diverges — exactly the S3 attack shape.
    fn build_bundle(src_root: &std::path::Path, ir_payload: Option<IrPayload>) -> std::path::PathBuf {
        build(BuildOptions {
            project_root: src_root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: None,
            ir_payload,
        })
        .expect("build must succeed")
        .path
    }

    #[test]
    fn run_bundle_rejects_ir_lowered_from_a_different_source() {
        // A: the source the user ships + inspects (no extra caps).
        let a = tempfile::tempdir().unwrap();
        write_project(a.path(), "proj", "[]");
        // B: a divergent source the attacker lowered the IR from.
        let b = tempfile::tempdir().unwrap();
        write_project(b.path(), "proj", "[{ kind = \"net.http\" }]");

        let ir_b = lower(b.path());
        // Bundle records A's tau.toml hash, but carries B's IR.
        let bundle = build_bundle(a.path(), Some(ir_b.clone()));

        let err = verify_bundle_against_source(a.path(), &bundle).unwrap_err();
        match err {
            VerifyError::IrSourceDivergence { bundle_hash, source_hash } => {
                assert_eq!(bundle_hash, ir_b.canonical_ir_hash, "bundle carries B's IR");
                assert_ne!(source_hash, bundle_hash, "A's re-lowered hash must differ");
            }
            other => panic!("expected IrSourceDivergence, got {other:?}"),
        }
    }

    #[test]
    fn run_bundle_accepts_genuine_ir() {
        let a = tempfile::tempdir().unwrap();
        write_project(a.path(), "proj", "[]");
        let ir_a = lower(a.path());
        let bundle = build_bundle(a.path(), Some(ir_a));
        verify_bundle_against_source(a.path(), &bundle).expect("genuine bundle must verify");
    }

    #[test]
    fn run_bundle_v1_unaffected() {
        // A v1 bundle (no ir_payload) verifies regardless of re-lowering.
        let a = tempfile::tempdir().unwrap();
        write_project(a.path(), "proj", "[]");
        let bundle = build_bundle(a.path(), None);
        verify_bundle_against_source(a.path(), &bundle).expect("v1 bundle must verify");
    }
}
```

- [ ] **Step 2: Run the test to verify the behavior**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-cli bundle_source_xcheck_tests`
Expected: PASS. (If implemented before Task 1, these would fail; Task 1 supplies `verify_bundle_against_source` + the error variants. Run against the post-Task-1 tree.)

Note: `verify_bundle_against_source` is a private fn in `run.rs`; this test lives in the same module tree (`super::`), so no visibility change is needed. `lower_ir` is `pub(crate)` and reachable as `crate::cmd::build::lower_ir`.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/src/cmd/run.rs
git commit -m "test(tau-cli): bundle with IR lowered from a divergent source is rejected

Builds an A-source bundle carrying B-source IR (extra net.http
capability) and asserts the run-path verify gate returns
IrSourceDivergence; a genuine bundle and a v1 bundle both pass.
Encodes the S3 finding's attack shape end-to-end through real lowering.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Task 3: integrity-vs-authenticity language + S2 trust-boundary docs

No behavior change — doc-comments and the security policy page only. Closes S3's second half (clarify "verified" ≠ "authentic / signed") and ships S2's documented trust boundary (`tau install` runs author code; code mitigation deferred).

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify.rs` (module doc-comment)
- Modify: `crates/tau-pkg/src/bundle/hash.rs` (`verify_self_hash` doc-comment)
- Modify: `crates/tau-pkg/src/install.rs` (module doc-comment)
- Modify: `SECURITY.md`

- [ ] **Step 1: Clarify the three guarantees in the verifier module doc**

In `crates/tau-pkg/src/bundle/verify.rs`, extend the top-of-file `//!` doc block with:

```rust
//!
//! # What "verified" means here
//!
//! This pipeline provides two guarantees and deliberately *not* a third:
//!
//! - **Integrity** (step 3, self-hash): the bundle's bytes have not been
//!   corrupted or altered since its builder sealed it. This is a checksum
//!   the builder computed over its own output — **not** a signature.
//! - **Source correspondence** (steps 6, 9, 10): the cwd `tau.toml`, the
//!   embedded IR bytes, and the IR the source lowers to all agree, so the
//!   executed workflow matches the source the user inspected.
//! - **Authenticity is *not* provided.** Nothing here proves *who* built
//!   the bundle or that its author is trustworthy; there is no signature.
//!   Trusting a bundle still means trusting whoever produced its source
//!   (see the `tau install` trust boundary in `SECURITY.md`).
```

- [ ] **Step 2: Clarify the self-hash doc-comment**

In `crates/tau-pkg/src/bundle/hash.rs`, on `pub fn verify_self_hash`, ensure the doc-comment states it is an integrity checksum, not authenticity. Add/replace its doc-comment with:

```rust
/// Verify the bundle's recorded self-hash against its canonical content.
///
/// This is an **integrity** check — it detects corruption or tampering of
/// the sealed bytes. It is **not** a signature and proves nothing about
/// *who* built the bundle or whether its source is trustworthy (see the
/// module doc on `bundle::verify` for the integrity / correspondence /
/// authenticity distinction).
```

- [ ] **Step 3: Add the S2 trust-boundary note to the install module doc**

In `crates/tau-pkg/src/install.rs`, append a section to the top-of-file `//!` module doc (after the existing 10-step description):

```rust
//!
//! # Trust boundary (security)
//!
//! `tau install <source>` clones an arbitrary repository and, for a
//! buildable package, compiles it (`cargo build`, which executes the
//! package's `build.rs` and proc macros) and may spawn the freshly built
//! binary for a capability cross-check — all **before** any Layer-4
//! sandbox or capability enforcement applies. **Installing a package
//! therefore executes its author's code on your machine: it is equivalent
//! to trusting that author.** Only install sources you trust. Running the
//! install-time build under a sandbox tier is tracked as a follow-up
//! (finding S2); the bundle-vs-source authenticity check (finding S3) is
//! implemented in `bundle::verify`.
```

- [ ] **Step 4: Add a "Trust boundaries" section to SECURITY.md**

In `SECURITY.md`, append a new `## Trust boundaries` section at the end of the file (the file's existing sections are `## Reporting a vulnerability`, `## Response`, `## Scope`, `## Supply chain`, all at `##` level — match that):

```markdown
## Trust boundaries

**`tau install <source>` runs the author's code.** Installing a package
clones an arbitrary source and builds it — running its `build.rs` and proc
macros, and possibly the freshly built binary — before any sandbox or
capability enforcement applies. Installing a package is equivalent to
trusting its author; only install sources you trust. (Sandboxing the
install-time build is a tracked follow-up.)

**A verified `.tau` bundle is integrity-checked, not signed.** `tau run
--bundle` proves a bundle's bytes are intact (integrity) and that its
embedded IR matches what the local `tau.toml` lowers to (source
correspondence), so the executed workflow cannot silently diverge from the
source you inspected. It does **not** prove *who* built the bundle —
there is no cryptographic signature. Trusting a bundle means trusting
whoever produced its source.
```

- [ ] **Step 5: Build the docs to confirm no linkcheck/build breakage**

`SECURITY.md` is in the book (`docs/SUMMARY.md` → `[Security policy](../SECURITY.md)`). Build per the DOCS RULES:

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines, no warnings/errors. Then `rm -rf docs/book`.

(If `mdbook`/`mdbook-linkcheck` are missing, note it and skip — the docs change is additive prose with no new links; CI's `docs-deploy` is the gate.)

- [ ] **Step 6: Confirm the crates still compile (doc-comment edits only)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo check -p tau-pkg`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg/src/bundle/verify.rs \
        crates/tau-pkg/src/bundle/hash.rs \
        crates/tau-pkg/src/install.rs \
        SECURITY.md
git commit -m "docs(security): clarify bundle integrity vs authenticity; document install trust boundary

Distinguish the three guarantees the bundle verifier provides
(integrity / source-correspondence / NOT authenticity) so 'verified'
no longer overstates the self-hash. Document the tau install trust
boundary (install == executing the author's code; S2 sandbox deferred).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] **Run the full affected-crate suites**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-cli
```
Expected: all green.

- [ ] **Clippy + fmt on the touched crates**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-pkg -p tau-cli --all-targets
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo fmt --check
```
Expected: no warnings; fmt clean.

- [ ] **requesting-code-review** (scope check before PR), then push + open PR citing S3 (full) and S2 (documented; sandbox deferred). STOP — no merge.

---

## Notes for the implementer

- **Do not touch `install.rs` beyond the doc-comment** — finding S2's code fix is deferred, and the install-path diagnostics session (45) may be editing that file.
- **Step ordering is load-bearing:** keep the new check as the *last* step in `verify_bundle`. Moving it earlier would change which error existing `cmd_run_bundle.rs` tests see.
- **Never pass an interim `recomputed_ir_hash: None` from `run.rs`** for a real run — that would fail-close every v2 bundle. The real `lower_ir` wiring (Task 1, Step 10) must land in the same commit as the field.
- **`to_hex_lower`** lives at `crate::tree_hash::to_hex_lower` (used by existing tests in `verify.rs`).
- If `cargo nextest` is unavailable, substitute `cargo test` with the same `-p` scope and timeouts (per CARGO RULES Rule 6).
