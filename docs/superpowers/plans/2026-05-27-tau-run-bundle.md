# `tau run --bundle` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the MVP `tau run --bundle <path> <agent-id>` consumer per spec `2026-05-27-tau-run-bundle-design.md`: a strict-by-default verifier that confirms a `.tau` bundle matches its source tree, then dispatches the named agent through the existing Runtime machinery.

**Architecture:** New `tau-pkg::bundle::verify` runs an 8-step integrity pipeline (read → parse → self-hash → schema → target → tau.toml drift → package drift → prompt drift) returning a `VerifyReport`. `tau-cli::cmd::run` gains a `--bundle` flag that calls `verify_bundle` as a **gate** before the existing run flow. Key simplification: because of the recipe model + refuse-on-drift, once verification passes the cwd's `tau.toml` is provably the bundle's source — so the existing `tau run` machinery runs unchanged afterward. `tau-runtime` stays bundle-agnostic.

**Tech Stack:** Rust 2021, `serde`/`toml`, `sha2`/`hex` (workspace deps). No new transitive deps.

**Cargo rules (CLAUDE.md):** Every cargo invocation uses `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <verb> -p <crate>`. Commits use `--no-verify` + `-c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com"`. NEVER `git stash` in this worktree.

**Pre-existing state (verified):**
- `tau-pkg::bundle::{manifest, canonical, hash, build, build_error}` all exist (§C.1 + §C.2).
- `BundleManifest::parse_str(&str) -> Result<Self, BundleParseError>` is the parser.
- `bundle::hash::verify_self_hash(&BundleManifest) -> Result<(), BundleIntegrityError>` exists.
- `crate::tree_hash::tree_hash(&Path) -> Result<String, TreeHashError>` + `to_hex_lower(&[u8]) -> String` exist.
- `tau_ports::target::TargetTriple::host()` is callable from tau-pkg (build.rs imports it).
- `BundleManifest` fields: `schema_version: u32`, `bundle: BundleMeta { sha256, created_at, tau_version, target: TargetTriple }`, `project: ProjectInfo { name, version: semver::Version, tau_toml_sha256 }`, `packages: Vec<BundlePackage>`, `agents: Vec<BundleAgent>`.
- `BundlePackage`: `name: String`, `version: semver::Version`, `source: PackageSource`, `tree_sha256: String`, `binary_sha256: Option<String>`, `required_shapes: Vec<CapabilityShape>`.
- `BundleAgent`: `id: AgentId`, `backend: BackendRef`, `system_prompt_sha256: String`, `required_tools: Vec<String>`, `effective_capabilities: BundleEffectiveCapabilities`.
- Install layout: `<project_root>/.tau/packages/<name>/<version>/`.
- Agent prompt source in tau.toml: `[agents.<id>.prompt]` table with `system` (inline) or `system_file` (path).
- `tau-cli::cmd::run::run(args: &RunArgs, record_protocol, force_passthrough, force_adapter_kind, output)` is the existing entry (run.rs:58). It loads `ProjectConfig::from_path(cwd/tau.toml)`, looks up the agent, resolves scope, builds the runtime, runs. The `--bundle` gate goes at the very top of this fn.
- `RunArgs` (cli.rs:454) has `agent_id`, `prompt`, `max_turns`, `dry_run`, `no_install`, `stream`, `max_total_*`. Add `bundle: Option<PathBuf>`.

---

## File Structure

**Created:**
- `crates/tau-pkg/src/bundle/verify_error.rs` — `VerifyError` enum
- `crates/tau-pkg/src/bundle/verify.rs` — `verify_bundle` + `VerifyOptions` + `VerifyReport` + `ResolvedAgent` + 8-step pipeline + unit tests
- `crates/tau-pkg/tests/bundle_verify_e2e.rs` — end-to-end build→verify roundtrip
- `crates/tau-cli/tests/cmd_run_bundle.rs` — CLI integration tests

**Modified:**
- `crates/tau-pkg/src/bundle/mod.rs` — re-export verify surface
- `crates/tau-cli/src/cli.rs` — add `bundle: Option<PathBuf>` to `RunArgs`
- `crates/tau-cli/src/cmd/run.rs` — `--bundle` verify gate at the top of `run()`
- `docs/superpowers/specs/2026-05-27-tau-run-bundle-design.md` — status Draft → Accepted (Task 10)

---

## Task 1: `VerifyError` + verify module skeleton

**Files:**
- Create: `crates/tau-pkg/src/bundle/verify_error.rs`
- Create: `crates/tau-pkg/src/bundle/verify.rs`
- Modify: `crates/tau-pkg/src/bundle/mod.rs`

- [ ] **Step 1: Write `verify_error.rs`**

```rust
//! `VerifyError` — typed failure type for `tau run --bundle` integrity
//! verification (Phase 2 §C.3).

use std::path::PathBuf;

use tau_ports::target::TargetTriple;

/// Errors returned by [`crate::bundle::verify_bundle`].
///
/// CLI exit-code mapping (`tau-cli::cmd::run`):
/// bad-input/config → 2, integrity/install-state → 3, internal/IO → 70.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The bundle file could not be read.
    #[error("failed to read bundle at {path:?}: {source}")]
    BundleRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The bundle TOML failed to parse.
    #[error("bundle parse failed: {source}")]
    BundleParse {
        /// Underlying parse error.
        #[source]
        source: crate::bundle::manifest::BundleParseError,
    },

    /// The bundle's recorded self-hash does not match its content.
    #[error("bundle self-hash mismatch — claimed {claimed}, computed {computed}; the bundle was tampered with or corrupted")]
    SelfHashMismatch {
        /// Hash stored in the bundle.
        claimed: String,
        /// Hash recomputed from the bundle content.
        computed: String,
    },

    /// The bundle's schema_version isn't supported by this tau.
    #[error("unsupported bundle schema_version {found}; this tau supports {supported}")]
    UnsupportedSchemaVersion {
        /// Version found in the bundle.
        found: u32,
        /// Version this binary supports.
        supported: u32,
    },

    /// The bundle's target triple doesn't match the host.
    #[error("bundle target {bundle} does not match host {host}; rebuild for this host or run on a matching machine")]
    TargetMismatch {
        /// Triple baked into the bundle.
        bundle: TargetTriple,
        /// Triple of the running host.
        host: TargetTriple,
    },

    /// The cwd's tau.toml has drifted from the bundle's record.
    #[error("tau.toml drift — claimed sha256 {claimed} but cwd has {computed}; rebuild the bundle or check out the source at the recorded version")]
    TauTomlDrift {
        /// Hash recorded in the bundle.
        claimed: String,
        /// Hash of the cwd's tau.toml.
        computed: String,
    },

    /// The project tau.toml is missing or unreadable.
    #[error("project tau.toml missing or unreadable at {path:?}: {source}")]
    ProjectTomlRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// A locked package isn't installed on disk.
    #[error("locked package `{name}` missing from {expected_path:?}; run `tau install` in this project")]
    PackageMissing {
        /// Package name.
        name: String,
        /// Where it was expected.
        expected_path: PathBuf,
    },

    /// An installed package's tree hash drifted from the bundle.
    #[error("package `{name}` tree drift — claimed {claimed}, computed {computed}; reinstall or rebuild bundle")]
    PackageDrift {
        /// Package name.
        name: String,
        /// Hash recorded in the bundle.
        claimed: String,
        /// Hash recomputed from the installed tree.
        computed: String,
    },

    /// Tree-hashing an installed package failed.
    #[error("package `{name}` tree-hash failed: {source}")]
    PackageTreeHash {
        /// Package name.
        name: String,
        /// Underlying error.
        #[source]
        source: crate::tree_hash::TreeHashError,
    },

    /// An agent's system prompt drifted from the bundle.
    #[error("agent `{id}` system prompt drift — claimed {claimed}, computed {computed}")]
    AgentPromptDrift {
        /// Agent id.
        id: String,
        /// Hash recorded in the bundle.
        claimed: String,
        /// Hash recomputed from the prompt content.
        computed: String,
    },

    /// Resolving an agent's prompt (e.g. reading system_file) failed.
    #[error("agent `{id}` prompt resolve failed: {source}")]
    AgentPromptResolve {
        /// Agent id.
        id: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// An agent in the bundle isn't present in the cwd's tau.toml.
    #[error("agent `{id}` named in the bundle but missing from tau.toml; rebuild bundle")]
    AgentSetMismatch {
        /// Agent id.
        id: String,
    },
}
```

- [ ] **Step 2: Write `verify.rs` skeleton**

```rust
//! `tau run --bundle` integrity verifier (Phase 2 §C.3).
//!
//! Confirms a `.tau` bundle matches the source tree at `project_root`
//! before the CLI dispatches the run. See spec
//! `2026-05-27-tau-run-bundle-design.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::bundle::manifest::{BundleAgent, BundleManifest};
use crate::bundle::verify_error::VerifyError;

/// Schema version this binary can verify.
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Inputs to [`verify_bundle`].
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Path to the `.tau` bundle file.
    pub bundle_path: PathBuf,
    /// Project source tree to verify against (typically cwd).
    pub project_root: PathBuf,
}

/// Result of a successful verification.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    /// The parsed, self-hash-verified manifest.
    pub manifest: BundleManifest,
    /// Per-agent context resolved during verification, keyed by id.
    pub agent_lookup: BTreeMap<String, ResolvedAgent>,
}

/// Per-agent verification result.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    /// The bundle's record for this agent.
    pub bundle_entry: BundleAgent,
    /// The verified-clean system-prompt bytes.
    pub system_prompt: Vec<u8>,
}

/// Verify a bundle against the source tree at
/// [`VerifyOptions::project_root`]. Strict: any drift, target
/// mismatch, or missing/altered install state returns an error.
pub fn verify_bundle(_opts: VerifyOptions) -> Result<VerifyReport, VerifyError> {
    unimplemented!("filled in by subsequent tasks")
}
```

- [ ] **Step 3: Wire `mod.rs`**

In `crates/tau-pkg/src/bundle/mod.rs`, add (alphabetical with existing `pub mod` lines):

```rust
pub mod verify;
pub mod verify_error;
```

And re-exports next to the existing ones:

```rust
pub use verify::{verify_bundle, ResolvedAgent, VerifyOptions, VerifyReport};
pub use verify_error::VerifyError;
```

- [ ] **Step 4: Verify build**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-pkg
```

Expected: clean (unused `_opts` warning OK).

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/verify_error.rs \
      crates/tau-pkg/src/bundle/verify.rs \
      crates/tau-pkg/src/bundle/mod.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle): VerifyError + verify_bundle skeleton"
```

**Concern:** verify the exact names of `BundleParseError` and `TreeHashError` (the `#[from]`/`#[source]` targets). They were confirmed present at §C.1/§C.2 time; if a name differs, adjust.

---

## Task 2: Verify steps 1-4 (read, parse, self-hash, schema)

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify.rs`

- [ ] **Step 1: Write failing tests**

Append a `#[cfg(test)] mod tests` block. These tests build a real bundle via `crate::bundle::build` then verify it, OR construct minimal failing inputs. Use a shared helper that writes a minimal valid project + builds a bundle:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::build::{build, BuildOptions};
    use tau_ports::target::TargetTriple;
    use tempfile::tempdir;

    /// Writes a minimal single-agent project (inline prompt, no
    /// packages) to `root`, builds a bundle, and returns its path.
    fn build_minimal_bundle(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::write(
            root.join("tau.toml"),
            r#"
[project]
name = "verify-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are solo"
"#,
        ).unwrap();
        std::fs::write(
            root.join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        ).unwrap();
        let artifact = build(BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
        }).expect("build fixture bundle");
        artifact.path
    }

    fn vopts(bundle_path: std::path::PathBuf, root: &std::path::Path) -> VerifyOptions {
        VerifyOptions { bundle_path, project_root: root.to_path_buf() }
    }

    #[test]
    fn verify_rejects_missing_bundle_file() {
        let tmp = tempdir().unwrap();
        let err = verify_bundle(vopts(tmp.path().join("nope.tau"), tmp.path())).unwrap_err();
        assert!(matches!(err, VerifyError::BundleRead { .. }), "got {err:?}");
    }

    #[test]
    fn verify_rejects_malformed_bundle_toml() {
        let tmp = tempdir().unwrap();
        let bad = tmp.path().join("bad.tau");
        std::fs::write(&bad, "this is not valid bundle toml @@@").unwrap();
        let err = verify_bundle(vopts(bad, tmp.path())).unwrap_err();
        assert!(matches!(err, VerifyError::BundleParse { .. }), "got {err:?}");
    }

    #[test]
    fn verify_rejects_self_hash_tampered_bundle() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        // Mutate the project name in the written bundle so its content
        // no longer matches the recorded self-hash.
        let content = std::fs::read_to_string(&path).unwrap();
        let tampered = content.replace("verify-fixture", "tampered-name");
        assert_ne!(content, tampered, "replacement must change content");
        std::fs::write(&path, tampered).unwrap();
        let err = verify_bundle(vopts(path, tmp.path())).unwrap_err();
        assert!(matches!(err, VerifyError::SelfHashMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn verify_rejects_unsupported_schema_version() {
        let tmp = tempdir().unwrap();
        let path = build_minimal_bundle(tmp.path());
        // Rewrite schema_version to 2. This invalidates the self-hash
        // too, but the schema check must run before/independently —
        // adjust the test if step ordering surfaces SelfHashMismatch
        // first: in that case assert on a fresh bundle whose hash was
        // recomputed for v2. Simpler: assert the error is one of the
        // two integrity variants. See note below.
        let content = std::fs::read_to_string(&path).unwrap();
        let bumped = content.replace("schema_version = 1", "schema_version = 2");
        std::fs::write(&path, bumped).unwrap();
        let err = verify_bundle(vopts(path, tmp.path())).unwrap_err();
        // Self-hash runs at step 3, schema at step 4, so a hand-edited
        // schema bump trips SelfHashMismatch first. That's acceptable —
        // both are integrity failures. To exercise the schema branch in
        // isolation, see the dedicated unit test on a synthesized
        // manifest below.
        assert!(
            matches!(err, VerifyError::SelfHashMismatch { .. } | VerifyError::UnsupportedSchemaVersion { .. }),
            "got {err:?}",
        );
    }
}
```

> **Note for implementer:** the schema-version test above trips self-hash first because hand-editing the TOML invalidates the hash. To test the schema branch cleanly, add a SEPARATE unit test that calls a step-4-only helper. Factor step 4 into `fn verify_schema_version(m: &BundleManifest) -> Result<(), VerifyError>` and unit-test THAT directly with a `BundleManifest` whose `schema_version = 2`. Do the same factoring for steps 5/6/7/8 — each becomes an independently-testable private fn. This is cleaner than driving everything through the file-based `verify_bundle` and avoids the "must recompute hash to test downstream steps" trap.

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests
```

Expected: panics at `unimplemented!()`.

- [ ] **Step 3: Implement steps 1-4 as factored helpers + wire into `verify_bundle`**

```rust
pub fn verify_bundle(opts: VerifyOptions) -> Result<VerifyReport, VerifyError> {
    // Step 1: read.
    let bundle_str = std::fs::read_to_string(&opts.bundle_path)
        .map_err(|e| VerifyError::BundleRead { path: opts.bundle_path.clone(), source: e })?;
    // Step 2: parse.
    let manifest = BundleManifest::parse_str(&bundle_str)
        .map_err(|e| VerifyError::BundleParse { source: e })?;
    // Step 3: self-hash.
    verify_self_hash_step(&manifest)?;
    // Step 4: schema version.
    verify_schema_version(&manifest)?;

    unimplemented!("steps 5-8 in subsequent tasks")
}

fn verify_self_hash_step(m: &BundleManifest) -> Result<(), VerifyError> {
    match crate::bundle::hash::verify_self_hash(m) {
        Ok(()) => Ok(()),
        Err(crate::bundle::error::BundleIntegrityError::HashMismatch { claimed, computed }) => {
            Err(VerifyError::SelfHashMismatch { claimed, computed })
        }
        // Other integrity variants (e.g. empty hash field) also map to
        // SelfHashMismatch with whatever detail is available.
        Err(other) => Err(VerifyError::SelfHashMismatch {
            claimed: String::new(),
            computed: format!("{other}"),
        }),
    }
}

fn verify_schema_version(m: &BundleManifest) -> Result<(), VerifyError> {
    if m.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(VerifyError::UnsupportedSchemaVersion {
            found: m.schema_version,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }
    Ok(())
}
```

> **Implementer:** confirm the actual variant shape of `BundleIntegrityError` (in `crates/tau-pkg/src/bundle/error.rs`). If `HashMismatch` has different field names (e.g. `expected`/`actual`), adapt. If `verify_self_hash` returns a simpler error, adapt the mapping.

Add a focused schema-branch unit test:

```rust
#[test]
fn verify_schema_version_rejects_v2() {
    // Build a real manifest, bump its schema_version, call the helper
    // directly (bypassing self-hash).
    let tmp = tempfile::tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path());
    let s = std::fs::read_to_string(&path).unwrap();
    let mut m = BundleManifest::parse_str(&s).unwrap();
    m.schema_version = 2;
    let err = verify_schema_version(&m).unwrap_err();
    assert!(matches!(err, VerifyError::UnsupportedSchemaVersion { found: 2, supported: 1 }), "got {err:?}");
}
```

- [ ] **Step 4: Confirm PASS**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests
```

The four tests pass (missing/malformed/tampered + schema). The full `verify_bundle` happy path still hits `unimplemented!()` — that's expected until Task 6.

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/verify.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle/verify): steps 1-4 — read, parse, self-hash, schema"
```

---

## Task 3: Verify step 5 (target ↔ host)

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn verify_target_rejects_foreign_triple() {
    use crate::bundle::manifest::BundleManifest;
    let tmp = tempfile::tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path());
    let s = std::fs::read_to_string(&path).unwrap();
    let mut m = BundleManifest::parse_str(&s).unwrap();
    m.bundle.target = TargetTriple::PASSTHROUGH; // never equals a native host
    let err = verify_target_matches_host(&m).unwrap_err();
    assert!(matches!(err, VerifyError::TargetMismatch { .. }), "got {err:?}");
}

#[test]
fn verify_target_accepts_host_triple() {
    use crate::bundle::manifest::BundleManifest;
    let tmp = tempfile::tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path());
    let s = std::fs::read_to_string(&path).unwrap();
    let m = BundleManifest::parse_str(&s).unwrap();
    // The fixture was built with TargetTriple::host(), so it matches.
    verify_target_matches_host(&m).expect("host triple matches");
}
```

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests::verify_target
```

Expected: FAIL with `no function 'verify_target_matches_host'`.

- [ ] **Step 3: Implement**

Add the helper + wire into `verify_bundle` (replace the trailing `unimplemented!` with the step-5 call followed by a new `unimplemented!`):

```rust
fn verify_target_matches_host(m: &BundleManifest) -> Result<(), VerifyError> {
    let host = tau_ports::target::TargetTriple::host();
    if m.bundle.target != host {
        return Err(VerifyError::TargetMismatch { bundle: m.bundle.target, host });
    }
    Ok(())
}
```

In `verify_bundle`, after step 4:

```rust
    verify_target_matches_host(&manifest)?;  // step 5
    unimplemented!("steps 6-8 in subsequent tasks")
```

> **Implementer:** `TargetTriple` is `Copy` (confirmed in §C.2). If it isn't, clone in the error construction.

- [ ] **Step 4: Confirm PASS**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests
```

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/verify.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle/verify): step 5 — target matches host"
```

---

## Task 4: Verify step 6 (tau.toml drift)

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn verify_tau_toml_drift_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path());
    // Mutate tau.toml after the build so its sha256 changes.
    std::fs::write(
        tmp.path().join("tau.toml"),
        r#"
[project]
name = "verify-fixture"
version = "0.2.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are solo"
"#,
    ).unwrap();
    let s = std::fs::read_to_string(&path).unwrap();
    let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
    let err = verify_tau_toml_sha256(&m, tmp.path()).unwrap_err();
    assert!(matches!(err, VerifyError::TauTomlDrift { .. }), "got {err:?}");
}

#[test]
fn verify_tau_toml_clean_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path());
    let s = std::fs::read_to_string(&path).unwrap();
    let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
    verify_tau_toml_sha256(&m, tmp.path()).expect("unchanged tau.toml verifies");
}
```

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests::verify_tau_toml
```

- [ ] **Step 3: Implement**

```rust
fn verify_tau_toml_sha256(
    m: &BundleManifest,
    project_root: &std::path::Path,
) -> Result<(), VerifyError> {
    let path = project_root.join("tau.toml");
    let bytes = std::fs::read(&path)
        .map_err(|e| VerifyError::ProjectTomlRead { path: path.clone(), source: e })?;
    let computed = sha256_hex(&bytes);
    if computed != m.project.tau_toml_sha256 {
        return Err(VerifyError::TauTomlDrift {
            claimed: m.project.tau_toml_sha256.clone(),
            computed,
        });
    }
    Ok(())
}

/// SHA-256 of `bytes` as lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    crate::tree_hash::to_hex_lower(h.finalize().as_slice())
}
```

In `verify_bundle`, after step 5:

```rust
    verify_tau_toml_sha256(&manifest, &opts.project_root)?;  // step 6
    unimplemented!("steps 7-8 in subsequent tasks")
```

> **Implementer:** `to_hex_lower(&[u8]) -> String` is confirmed in `tree_hash.rs`. The §C.2 build pipeline hashes tau.toml the same way (`build.rs:241-244`) — so the computed hash must match byte-for-byte. Double-check that `build` hashes the RAW file bytes (not a normalized/parsed form); if build hashes something else, mirror exactly whatever build does.

- [ ] **Step 4: Confirm PASS** + **Step 5: Commit** (`feat(tau-pkg/bundle/verify): step 6 — tau.toml drift detection`)

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/verify.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle/verify): step 6 — tau.toml drift detection"
```

---

## Task 5: Verify step 7 (packages installed + hashed)

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify.rs`

- [ ] **Step 1: Failing tests**

The minimal fixture has no packages, so add a fixture variant with one installed package. Append:

```rust
/// Writes a project + lockfile + one installed package dir, builds a
/// bundle, returns (bundle_path, package_dir).
fn build_bundle_with_one_package(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "pkg-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"
"#,
    ).unwrap();
    let pkg_dir = root.join(".tau/packages/demo/0.1.0");
    std::fs::create_dir_all(pkg_dir.join("src")).unwrap();
    std::fs::write(pkg_dir.join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    std::fs::write(pkg_dir.join("src/lib.rs"), "// demo\n").unwrap();
    std::fs::write(
        root.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "demo"
active_version = "0.1.0"
source = "https://example.com/demo.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
    ).unwrap();
    let artifact = crate::bundle::build::build(crate::bundle::build::BuildOptions {
        project_root: root.to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
    }).expect("build bundle with package");
    (artifact.path, pkg_dir)
}

#[test]
fn verify_package_missing_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let (path, pkg_dir) = build_bundle_with_one_package(tmp.path());
    std::fs::remove_dir_all(&pkg_dir).unwrap(); // uninstall after build
    let s = std::fs::read_to_string(&path).unwrap();
    let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
    let err = verify_packages_installed_and_hashed(&m, tmp.path()).unwrap_err();
    assert!(matches!(err, VerifyError::PackageMissing { .. }), "got {err:?}");
}

#[test]
fn verify_package_tree_drift_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let (path, pkg_dir) = build_bundle_with_one_package(tmp.path());
    // Change a file inside the installed package after build.
    std::fs::write(pkg_dir.join("src/lib.rs"), "// tampered\n").unwrap();
    let s = std::fs::read_to_string(&path).unwrap();
    let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
    let err = verify_packages_installed_and_hashed(&m, tmp.path()).unwrap_err();
    assert!(matches!(err, VerifyError::PackageDrift { .. }), "got {err:?}");
}

#[test]
fn verify_packages_clean_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let (path, _pkg_dir) = build_bundle_with_one_package(tmp.path());
    let s = std::fs::read_to_string(&path).unwrap();
    let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
    verify_packages_installed_and_hashed(&m, tmp.path()).expect("clean packages verify");
}
```

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests::verify_package
```

- [ ] **Step 3: Implement**

```rust
fn verify_packages_installed_and_hashed(
    m: &BundleManifest,
    project_root: &std::path::Path,
) -> Result<(), VerifyError> {
    for pkg in &m.packages {
        let dir = project_root
            .join(".tau/packages")
            .join(&pkg.name)
            .join(pkg.version.to_string());
        if !dir.exists() {
            return Err(VerifyError::PackageMissing {
                name: pkg.name.clone(),
                expected_path: dir,
            });
        }
        let computed = crate::tree_hash::tree_hash(&dir)
            .map_err(|e| VerifyError::PackageTreeHash { name: pkg.name.clone(), source: e })?;
        if computed != pkg.tree_sha256 {
            return Err(VerifyError::PackageDrift {
                name: pkg.name.clone(),
                claimed: pkg.tree_sha256.clone(),
                computed,
            });
        }
    }
    Ok(())
}
```

In `verify_bundle`, after step 6:

```rust
    verify_packages_installed_and_hashed(&manifest, &opts.project_root)?;  // step 7
    unimplemented!("step 8 in next task")
```

> **Implementer:** `pkg.name` is `String` and `pkg.version` is `semver::Version` (confirmed in §C.2 — `BundlePackage`). The install layout `<root>/.tau/packages/<name>/<version>/` matches build's gather step. `pkg.version.to_string()` gives the directory segment.

- [ ] **Step 4: Confirm PASS** + **Step 5: Commit** (`feat(tau-pkg/bundle/verify): step 7 — package install + tree drift`)

---

## Task 6: Verify step 8 (agent prompts + agent_lookup) — completes `verify_bundle`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/verify.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn verify_happy_path_returns_report_with_agent_lookup() {
    let tmp = tempfile::tempdir().unwrap();
    let path = build_minimal_bundle(tmp.path());
    let report = verify_bundle(vopts(path, tmp.path())).expect("verify succeeds");
    assert_eq!(report.manifest.project.name, "verify-fixture");
    assert!(report.agent_lookup.contains_key("solo"));
    assert_eq!(report.agent_lookup["solo"].system_prompt, b"you are solo");
}

#[test]
fn verify_agent_prompt_file_drift_detected() {
    let tmp = tempfile::tempdir().unwrap();
    // Build a fixture whose agent uses system_file.
    std::fs::write(
        tmp.path().join("tau.toml"),
        r#"
[project]
name = "file-prompt"
version = "0.1.0"

[agents.writer]
display_name = "Writer"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.writer.prompt]
system_file = "prompt.md"
"#,
    ).unwrap();
    std::fs::write(tmp.path().join("prompt.md"), "original prompt").unwrap();
    std::fs::write(
        tmp.path().join("tau.lock"),
        "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
    ).unwrap();
    let artifact = crate::bundle::build::build(crate::bundle::build::BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
    }).unwrap();
    // Mutate the prompt file after build (tau.toml itself unchanged, so
    // step 6 passes but step 8 must catch the prompt drift).
    std::fs::write(tmp.path().join("prompt.md"), "tampered prompt").unwrap();
    let err = verify_bundle(vopts(artifact.path, tmp.path())).unwrap_err();
    assert!(matches!(err, VerifyError::AgentPromptDrift { .. }), "got {err:?}");
}
```

- [ ] **Step 2: Confirm FAIL** (happy path hits `unimplemented!`)

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify::tests
```

- [ ] **Step 3: Implement step 8 + finish `verify_bundle`**

Replace the trailing `unimplemented!("step 8...")` with:

```rust
    // Step 8: agent prompts. Re-parse the (verified-clean) tau.toml to
    // find each bundle agent's prompt source, resolve it, re-hash, and
    // build the agent_lookup.
    let agent_lookup = verify_agent_prompts(&manifest, &opts.project_root)?;
    Ok(VerifyReport { manifest, agent_lookup })
}

fn verify_agent_prompts(
    m: &BundleManifest,
    project_root: &std::path::Path,
) -> Result<BTreeMap<String, ResolvedAgent>, VerifyError> {
    // Parse the project tau.toml via the existing config loader so we
    // read the same agent/prompt shape `tau run` uses. The bytes were
    // verified clean in step 6, so this parse should succeed; surface
    // any failure as ProjectTomlRead.
    let path = project_root.join("tau.toml");
    let project = crate::project::ProjectConfig::from_path(&path)
        .map_err(|e| VerifyError::ProjectTomlRead {
            path: path.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;

    let mut lookup = BTreeMap::new();
    for agent in &m.agents {
        let id = agent.id.as_str().to_string();
        let entry = project.agents.get(&id).ok_or_else(|| VerifyError::AgentSetMismatch {
            id: id.clone(),
        })?;
        // Resolve the prompt to bytes. The exact accessor depends on
        // the ProjectConfig agent type — see implementer note.
        let prompt_bytes = resolve_agent_prompt(entry, project_root)
            .map_err(|e| VerifyError::AgentPromptResolve { id: id.clone(), source: e })?;
        let computed = sha256_hex(&prompt_bytes);
        if computed != agent.system_prompt_sha256 {
            return Err(VerifyError::AgentPromptDrift {
                id: id.clone(),
                claimed: agent.system_prompt_sha256.clone(),
                computed,
            });
        }
        lookup.insert(id, ResolvedAgent {
            bundle_entry: agent.clone(),
            system_prompt: prompt_bytes,
        });
    }
    Ok(lookup)
}
```

> **Implementer — CRITICAL:** `resolve_agent_prompt(entry, project_root)` must reproduce EXACTLY what §C.2's build did when computing `system_prompt_sha256`, or every verify will spuriously fail. Read `crates/tau-pkg/src/bundle/build.rs` step 5 (the `gather_agent_facts` / prompt-resolution code, around the `system_prompt_sha256` computation) and reuse the SAME resolution logic. The build used the `UncheckedProjectConfig::validate()` pipeline and resolved inline `system` vs `system_file`. Best path: extract build's prompt-resolution into a shared `pub(crate) fn resolve_agent_prompt_bytes(...)` and call it from BOTH build and verify, so they can't drift. If extraction is too invasive, replicate the exact logic and add a comment cross-referencing build.rs. The `ProjectConfig` agent type + its prompt accessor must match what build read (likely a `PromptEntry` enum with `None`/`Inline`/`File` variants — note build hashed the empty string for `PromptEntry::None`).

- [ ] **Step 4: Confirm PASS** — all verify tests including happy path

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::verify
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
```

- [ ] **Step 5: Commit** (`feat(tau-pkg/bundle/verify): step 8 — agent prompt drift + complete verify_bundle`)

---

## Task 7: End-to-end build→verify integration test

**Files:**
- Create: `crates/tau-pkg/tests/bundle_verify_e2e.rs`

- [ ] **Step 1: Write the test**

Reuse §C.2's e2e fixture shape (2 packages + 2 agents, one inline + one file prompt). Copy the `write_fixture` helper from `crates/tau-pkg/tests/bundle_build_e2e.rs` verbatim (read it first: `git show <e2e-commit> -- crates/tau-pkg/tests/bundle_build_e2e.rs` or just open the file).

```rust
//! End-to-end: build a bundle, then verify it against the same source
//! tree. Asserts the happy path + that mutation is caught.

use tau_pkg::bundle::{build, verify_bundle, BuildOptions, VerifyError, VerifyOptions};
use tau_ports::target::TargetTriple;

mod fixture {
    // Paste the write_fixture() helper from bundle_build_e2e.rs here,
    // OR factor it into a shared test-support module. Keep it identical
    // so build + verify agree.
}

#[test]
fn e2e_build_then_verify_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    fixture::write_fixture(tmp.path());
    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
    }).unwrap();

    let report = verify_bundle(VerifyOptions {
        bundle_path: artifact.path,
        project_root: tmp.path().to_path_buf(),
    }).expect("verify succeeds on freshly-built bundle");

    assert_eq!(report.agent_lookup.len(), 2);
    assert!(report.agent_lookup.contains_key("researcher"));
    assert!(report.agent_lookup.contains_key("writer"));
}

#[test]
fn e2e_verify_catches_post_build_package_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    fixture::write_fixture(tmp.path());
    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
    }).unwrap();

    // Mutate an installed package file.
    let f = tmp.path().join(".tau/packages/fs-read/0.1.0/src/lib.rs");
    std::fs::write(&f, "// mutated after build\n").unwrap();

    let err = verify_bundle(VerifyOptions {
        bundle_path: artifact.path,
        project_root: tmp.path().to_path_buf(),
    }).unwrap_err();
    assert!(matches!(err, VerifyError::PackageDrift { .. }), "got {err:?}");
}
```

> **Implementer:** the fixture's exact paths (`fs-read`, agent ids `researcher`/`writer`) come from §C.2's `bundle_build_e2e.rs`. Match whatever that file actually uses. If the §C.2 fixture used different package names, adapt the mutation path in test 2.

- [ ] **Step 2: Run + Step 3: Commit**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg --test bundle_verify_e2e
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/tests/bundle_verify_e2e.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(tau-pkg/bundle/verify): build-then-verify e2e + mutation detection"
```

---

## Task 8: CLI `--bundle` flag + verify gate

**Files:**
- Modify: `crates/tau-cli/src/cli.rs` (add `bundle` field to `RunArgs`)
- Modify: `crates/tau-cli/src/cmd/run.rs` (verify gate at top of `run()`)

- [ ] **Step 1: Add the flag to `RunArgs`**

In `crates/tau-cli/src/cli.rs`, inside `pub struct RunArgs` (line ~454), add:

```rust
    /// Run from a pre-built bundle, verifying the cwd matches it first.
    /// The bundle must have been built for this host triple and the
    /// project must be installed at the exact tree hashes recorded in
    /// the bundle, or the run is refused (Phase 2 §C.3).
    #[arg(long, value_name = "PATH")]
    pub bundle: Option<std::path::PathBuf>,
```

- [ ] **Step 2: Add the verify gate to `run()`**

In `crates/tau-cli/src/cmd/run.rs`, at the very top of `pub async fn run(...)` (after `let cwd = std::env::current_dir()?;` at line 65), insert:

```rust
    // §C.3: when --bundle is set, verify the cwd matches the sealed
    // bundle before doing anything else. On success the cwd's tau.toml
    // is provably the bundle's source, so the rest of `run` proceeds
    // unchanged. On failure, map the VerifyError to the spec's exit
    // codes and bail.
    if let Some(bundle_path) = &args.bundle {
        if let Err(e) = tau_pkg::bundle::verify_bundle(tau_pkg::bundle::VerifyOptions {
            bundle_path: bundle_path.clone(),
            project_root: cwd.clone(),
        }) {
            eprintln!("error: {e}");
            std::process::exit(bundle_verify_exit_code(&e));
        }
    }
```

Add the exit-code mapper as a private fn in `run.rs`:

```rust
fn bundle_verify_exit_code(e: &tau_pkg::bundle::VerifyError) -> i32 {
    use tau_pkg::bundle::VerifyError as V;
    match e {
        V::BundleRead { .. }
        | V::BundleParse { .. }
        | V::ProjectTomlRead { .. }
        | V::UnsupportedSchemaVersion { .. } => 2,
        V::SelfHashMismatch { .. }
        | V::TargetMismatch { .. }
        | V::TauTomlDrift { .. }
        | V::PackageMissing { .. }
        | V::PackageDrift { .. }
        | V::AgentPromptDrift { .. }
        | V::AgentSetMismatch { .. } => 3,
        V::PackageTreeHash { .. } | V::AgentPromptResolve { .. } => 70,
    }
}
```

> **Implementer:** confirm the exit mechanism matches how `cmd::run` reports other exit codes. If `run()` returns `anyhow::Result<()>` and exit codes are mapped elsewhere via a custom error type, follow that pattern instead of `std::process::exit`. Check how `dry_run` / agent-not-found currently exit. The `std::process::exit` approach mirrors `cmd::check` + `cmd::build` (per §C.2); use it unless the surrounding code clearly maps errors to codes through a typed channel.

Also: the existing "agent not found in tau.toml" error (run.rs:70) already covers the "agent not in bundle" case once verify passes (because tau.toml == bundle's source). Confirm that path exits 2; if it exits with a different code, add an explicit check after the bundle gate: look up `args.agent_id` in the verify report's `agent_lookup` and exit 2 with a bundle-specific message if absent. (Simplest: rely on the existing check; only add the explicit one if the existing exit code isn't 2.)

- [ ] **Step 3: Build + smoke test**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-cli
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo run -p tau-cli -- run --help 2>&1 | grep -A1 bundle
```

Expected: `--bundle <PATH>` appears in `tau run --help`.

- [ ] **Step 4: Regenerate help snapshots**

`--bundle` changes the `run --help` snapshot. Regenerate:

```bash
cd crates/tau-cli && \
  CARGO_INCREMENTAL=0 \
  CARGO_TARGET_DIR=/Users/titouanlebocq/code/tau-worktrees/tau-run-bundle/target/agent-impl \
  cargo insta test --accept --test help_snapshots
cd /Users/titouanlebocq/code/tau-worktrees/tau-run-bundle && \
  timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --test help_snapshots
```

Expected: `run_help` snapshot updated; all pass.

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/run.rs crates/tau-cli/tests/snapshots/ && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-cli): tau run --bundle verify gate"
```

---

## Task 9: CLI integration tests

**Files:**
- Create: `crates/tau-cli/tests/cmd_run_bundle.rs`

- [ ] **Step 1: Write the tests**

Mirror the env-scrubbing + TAU_HOME pattern from `crates/tau-cli/tests/cmd_build.rs` (created in §C.2 — read it for the exact helper shape). Use an echo-llm fixture for the happy path so no real backend is needed; look at how `cmd_chat.rs` / `cmd_run*.rs` set up `common::setup_echo_project` or equivalent.

```rust
//! Integration tests for `tau run --bundle` (Phase 2 §C.3).

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;
use predicates::prelude::*;

// Helper: build a bundle in `project` via the `tau build` binary, then
// return its path. Reuses the same fixture shape cmd_build.rs uses.
// (See cmd_build.rs for write_minimal_project + TAU_HOME setup.)

#[test]
fn run_bundle_with_drift_exits_three_with_diagnostic() {
    // 1. Stand up a project + lockfile.
    // 2. `tau build` to produce the bundle.
    // 3. Mutate tau.toml.
    // 4. `tau run --bundle <path> <agent>` → exit 3, stderr "tau.toml drift".
    // (Full body: follow cmd_build.rs patterns.)
}

#[test]
fn run_bundle_with_missing_install_exits_three() {
    // build with one package installed, delete the package dir, run → exit 3, stderr "missing from".
}

#[test]
fn run_bundle_with_foreign_target_exits_three() {
    // Harder via the binary: the bundle's target is always host when
    // built locally. To force a foreign target, hand-edit the bundle's
    // target line AND recompute its self-hash — too fiddly for a CLI
    // test. Instead assert this at the unit level (already covered by
    // Task 3's verify_target_rejects_foreign_triple). SKIP at CLI level
    // OR, if feasible, build then hex-edit + leave self-hash stale and
    // assert exit 3 with stderr "self-hash" (still proves the gate
    // refuses). Pick the latter: it proves the CLI surfaces a refusal.
}

#[test]
fn run_bundle_clean_fixture_succeeds() {
    // build, then `tau run --bundle <path> <agent> --prompt "hi"` with
    // an echo backend → exit 0, expected echo output.
    // If wiring an echo backend through the bundle path is heavy, a
    // --dry-run variant is acceptable: `tau run --bundle <p> <agent>
    // --dry-run` → exit 0 (verify gate passed + dry-run short-circuit).
}
```

> **Implementer:** flesh out each test body fully (the plan shows intent + the tricky bits). The foreign-target CLI test is genuinely awkward through the binary — prefer asserting the CLI refuses a self-hash-stale bundle (which proves the gate fires) and rely on Task 3's unit test for the target-specific branch. For the happy path, `--dry-run` is the cheapest way to prove "verify gate passed, then normal run flow took over" without standing up a live LLM. Use whatever echo-project helper the existing cmd_*.rs tests use if a full run is easy; otherwise dry-run.

- [ ] **Step 2: Run**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_run_bundle
```

- [ ] **Step 3: Commit** (`test(tau-cli): tau run --bundle integration tests`)

---

## Task 10: Final verify + spec accept + PR

**Files:**
- Modify: `docs/superpowers/specs/2026-05-27-tau-run-bundle-design.md` (Status: Draft → Accepted)

- [ ] **Step 1: Flip spec status**

Change the spec's `**Status:** Draft` to `**Status:** Accepted`.

- [ ] **Step 2: Full verification matrix**

```bash
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt --all -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

All clean. `cargo fmt --all` to fix any drift before committing.

- [ ] **Step 3: Commit + push + PR**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add docs/superpowers/specs/2026-05-27-tau-run-bundle-design.md && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "docs(spec): tau run --bundle — accept §C.3"
git push --no-verify -u origin HEAD
```

PR title: `feat(tau-pkg): tau run --bundle MVP consumer (Phase 2 §C.3)`. Body recaps: verify pipeline, recipe model, strict-everything, deferred items. Note the `--no-verify` push (Podman gate); CI is the gate.

```bash
gh pr create --title "feat(tau-pkg): tau run --bundle MVP consumer (Phase 2 §C.3)" --body "<recap>"
gh pr merge --auto $(gh pr list --head feat/tau-run-bundle --json number --jq '.[0].number')
```

---

## Self-review pass

**Spec coverage:**
- Spec §2 (recipe / refuse-on-drift / strict / CLI flag / verifier-in-tau-pkg) → Tasks 1-8.
- Spec §4 (verify API) → Task 1.
- Spec §5 (8-step pipeline) → Tasks 2 (1-4), 3 (5), 4 (6), 5 (7), 6 (8).
- Spec §5.2 (step6/8 overlap) → Task 6 implementer note.
- Spec §6 (CLI) → Task 8.
- Spec §6.2 (exit codes) → Task 8 `bundle_verify_exit_code`.
- Spec §7 (errors) → Task 1.
- Spec §8.1 (unit tests) → Tasks 2-6.
- Spec §8.2 (e2e) → Task 7.
- Spec §8.3 (CLI integration) → Task 9.

**Type consistency:** `verify_bundle`, `VerifyOptions`, `VerifyReport`, `ResolvedAgent`, `VerifyError` consistent across tasks. Helper fns (`verify_schema_version`, `verify_target_matches_host`, `verify_tau_toml_sha256`, `verify_packages_installed_and_hashed`, `verify_agent_prompts`, `sha256_hex`, `resolve_agent_prompt`) named consistently. `sha256_hex` defined in Task 4, reused in Task 6.

**Placeholder scan:** Task 9 test bodies are intentionally sketched (with explicit "flesh out fully" instructions + the tricky bits spelled out) rather than fully written — the foreign-target CLI test genuinely depends on discovering the easiest refusal to trigger through the binary, which the implementer must determine against the real echo-fixture helpers. This is a judgment call flagged for the implementer, not an unspecified placeholder. All `tau-pkg` tasks (the load-bearing logic) have complete code.

**Known shared-logic risk:** Task 6's `resolve_agent_prompt` MUST match build's prompt hashing exactly. The plan calls for extracting a shared `pub(crate)` helper used by both build + verify. This is the single highest-risk integration point — flagged prominently in Task 6.
