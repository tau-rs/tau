# `tau verify --bundle` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the MVP `tau verify --bundle <path>` reproducibility checker per spec `2026-05-28-tau-verify-bundle-design.md`: rebuild a fresh bundle from the local tree, compare self-hashes, and report a field-level diff on mismatch.

**Architecture:** New `tau_pkg::bundle::reproduce` module: `verify_reproducible` parses the shipped bundle → rebuilds via §C.2's `build` (using the shipped bundle's target, to a temp path) → compares self-hashes → `diff_manifests` produces field-level divergences on mismatch. `tau-cli::cmd::verify` gains a `--bundle` branch with human/JSON renderers. Logic in tau-pkg; tau-runtime untouched.

**Tech Stack:** Rust 2021, `serde`/`toml`, `sha2` (all workspace deps). `tempfile` is already a tau-pkg production dependency (verified) — the runtime rebuild temp dir is fine.

**Cargo rules (CLAUDE.md):** Every cargo invocation uses `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <verb> -p <crate>`. Commits use `--no-verify` + `-c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com"`. NEVER `git stash` in this worktree.

**Pre-existing state (verified):**
- `tau_pkg::bundle::{build, BuildOptions, BundleArtifact}` re-exported (§C.2, merged). `build(BuildOptions { project_root, target, output_path }) -> Result<BundleArtifact, BuildError>`.
- `BundleManifest::parse_str(&str) -> Result<Self, BundleParseError>`; `bundle::hash::verify_self_hash(&BundleManifest) -> Result<(), BundleIntegrityError>`.
- `compute_self_hash` zeros `bundle.sha256` AND `bundle.created_at` (so a clean rebuild is bit-stable). `bundle.tau_version` IS in the hash (option a — reproducibility is same-source-AND-same-tau-version).
- `BundleMeta { sha256: String, created_at: String, tau_version: String, target: TargetTriple }`.
- `ProjectInfo { name: String, version: semver::Version, tau_toml_sha256: String }`.
- `BundlePackage { name: String, version: semver::Version, source: PackageSource, tree_sha256: String, binary_sha256: Option<String>, required_shapes: Vec<CapabilityShape> }`.
- `BundleAgent { id: AgentId, backend: BackendRef, system_prompt_sha256: String, required_tools: Vec<String>, effective_capabilities: BundleEffectiveCapabilities }`.
- `tempfile` is in tau-pkg `[dependencies]` (production), not just dev.
- `tau-cli::cmd::verify::run(args: &VerifyArgs, output: &mut Output) -> anyhow::Result<()>` (verify.rs). It resolves scope, calls `tau_pkg::verify`/`verify_all`, renders human or `--json`. NOTE: `VerifyArgs` has NO `--json` field of its own — `--json` is a GLOBAL flag on the top-level `Cli` (confirm by grepping `pub json` in cli.rs; the verify renderer reads it via the `Output`). The plan's Task 5 confirms the json plumbing path.
- `VerifyArgs { package: Option<String>, version: Option<String>, global: bool, anthropic_strict: bool }` (cli.rs ~417).
- Stale doc-comment: `BundleMeta::created_at`'s `///` says "Informational; in the hash" — WRONG (it's excluded). Fix in Task 7.

---

## File Structure

**Created:**
- `crates/tau-pkg/src/bundle/reproduce.rs` — `verify_reproducible`, `ReproOptions`, `ReproReport`, `ManifestDiff`, `Side`, `diff_manifests` + unit tests
- `crates/tau-pkg/src/bundle/reproduce_error.rs` — `ReproError` enum
- `crates/tau-pkg/tests/bundle_reproduce_e2e.rs` — build→reproduce e2e
- `crates/tau-cli/tests/cmd_verify_bundle.rs` — CLI integration tests

**Modified:**
- `crates/tau-pkg/src/bundle/mod.rs` — re-export reproduce surface
- `crates/tau-pkg/src/bundle/manifest.rs` — fix the stale `created_at` doc-comment (Task 7)
- `crates/tau-cli/src/cli.rs` — add `bundle: Option<PathBuf>` to `VerifyArgs`
- `crates/tau-cli/src/cmd/verify.rs` — `--bundle` branch + `render_repro_human`/`render_repro_json` + exit-code mapping

---

## Task 1: `ReproError` + reproduce module skeleton

**Files:**
- Create: `crates/tau-pkg/src/bundle/reproduce_error.rs`
- Create: `crates/tau-pkg/src/bundle/reproduce.rs`
- Modify: `crates/tau-pkg/src/bundle/mod.rs`

- [ ] **Step 1: `reproduce_error.rs`**

```rust
//! `ReproError` — failures that prevent producing a reproducibility
//! comparison (Phase 2 §E). A successful *non-reproducible* result is
//! NOT an error — it's `Ok(ReproReport { reproducible: false, .. })`.

use std::path::PathBuf;

/// Errors from [`crate::bundle::verify_reproducible`].
#[derive(Debug, thiserror::Error)]
pub enum ReproError {
    /// The shipped bundle file could not be read.
    #[error("failed to read bundle at {path:?}: {source}")]
    BundleRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The shipped bundle failed to parse.
    #[error("bundle parse failed: {source}")]
    BundleParse {
        /// Underlying parse error.
        #[source]
        source: crate::bundle::error::BundleParseError,
    },

    /// The shipped bundle's own self-hash is invalid — it is corrupt
    /// and cannot serve as a reproducibility reference.
    #[error("shipped bundle self-hash is invalid ({detail}); it is corrupt — cannot use it as a reproducibility reference")]
    ShippedSelfHashInvalid {
        /// Human-readable detail from the integrity check.
        detail: String,
    },

    /// Could not create the temp dir for the rebuild.
    #[error("could not create temp dir for rebuild: {source}")]
    TempDir {
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The rebuild from the local tree failed.
    #[error("rebuild failed: {source}")]
    Rebuild {
        /// Underlying build error.
        #[source]
        source: crate::bundle::build_error::BuildError,
    },

    /// The rebuilt bundle could not be read back.
    #[error("failed to read rebuilt bundle at {path:?}: {source}")]
    RebuiltRead {
        /// Path attempted.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// The rebuilt bundle failed to parse.
    #[error("rebuilt bundle parse failed: {source}")]
    RebuiltParse {
        /// Underlying parse error.
        #[source]
        source: crate::bundle::error::BundleParseError,
    },
}
```

- [ ] **Step 2: `reproduce.rs` skeleton**

```rust
//! `tau verify --bundle` reproducibility checker (Phase 2 §E).
//!
//! Rebuilds a fresh bundle from the local source tree and compares its
//! self-hash to a shipped bundle. See spec
//! `2026-05-28-tau-verify-bundle-design.md`.

use std::path::PathBuf;

use crate::bundle::manifest::BundleManifest;
use crate::bundle::reproduce_error::ReproError;

/// Inputs to [`verify_reproducible`].
#[derive(Debug, Clone)]
pub struct ReproOptions {
    /// Path to the shipped `.tau` bundle to reproduce.
    pub bundle_path: PathBuf,
    /// Local source tree to rebuild from (typically cwd).
    pub project_root: PathBuf,
}

/// Result of a reproducibility check.
#[derive(Debug, Clone)]
pub struct ReproReport {
    /// True when the rebuilt bundle's self-hash equals the shipped one's.
    pub reproducible: bool,
    /// The shipped bundle's self-hash.
    pub shipped_sha256: String,
    /// The rebuilt bundle's self-hash.
    pub rebuilt_sha256: String,
    /// Field-level divergences. Empty when `reproducible`.
    pub diffs: Vec<ManifestDiff>,
}

/// Which side of a comparison a one-sided item appears on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// Present only in the shipped bundle.
    ShippedOnly,
    /// Present only in the rebuilt bundle.
    RebuiltOnly,
}

/// A single field-level divergence between two manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestDiff {
    /// A `[project]` field differs.
    ProjectField { field: String, shipped: String, rebuilt: String },
    /// A package is present on only one side.
    PackageMissing { name: String, side: Side },
    /// A package field differs.
    PackageField { name: String, field: String, shipped: String, rebuilt: String },
    /// An agent is present on only one side.
    AgentMissing { id: String, side: Side },
    /// An agent field differs.
    AgentField { id: String, field: String, shipped: String, rebuilt: String },
    /// A `[bundle]` metadata field differs (target, tau_version).
    BundleMetaField { field: String, shipped: String, rebuilt: String },
    /// schema_version differs.
    SchemaVersionMismatch { shipped: u32, rebuilt: u32 },
}

/// Rebuild from `opts.project_root` and compare to the shipped bundle.
pub fn verify_reproducible(_opts: ReproOptions) -> Result<ReproReport, ReproError> {
    unimplemented!("Task 3")
}

/// Field-level diff between two manifests (Task 2).
pub(crate) fn diff_manifests(_shipped: &BundleManifest, _rebuilt: &BundleManifest) -> Vec<ManifestDiff> {
    unimplemented!("Task 2")
}
```

- [ ] **Step 3: Wire `mod.rs`**

Add (alphabetical):
```rust
pub mod reproduce;
pub mod reproduce_error;
```
Re-exports:
```rust
pub use reproduce::{verify_reproducible, ManifestDiff, ReproOptions, ReproReport, Side};
pub use reproduce_error::ReproError;
```

- [ ] **Step 4: Build**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-pkg
```

Clean (unused-stub warnings OK).

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/reproduce_error.rs crates/tau-pkg/src/bundle/reproduce.rs crates/tau-pkg/src/bundle/mod.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle): ReproError + reproduce skeleton"
```

**Concern:** confirm `crate::bundle::error::BundleParseError` + `crate::bundle::build_error::BuildError` paths (they're the same ones §C.3's verify_error.rs used — correct).

---

## Task 2: `diff_manifests` (pure function, fully unit-tested)

**Files:**
- Modify: `crates/tau-pkg/src/bundle/reproduce.rs`

This is the field-diff logic. It's pure (two manifests in, Vec out) so it's tested in isolation without I/O.

- [ ] **Step 1: Failing tests**

Append a `#[cfg(test)] mod tests` block. Build manifests by parsing a known-good bundle (produced via `build` in a tempdir) then mutating clones — avoids hand-constructing every field.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::build::{build, BuildOptions};
    use tau_ports::target::TargetTriple;
    use tempfile::tempdir;

    /// Build a minimal bundle and return its parsed manifest.
    fn sample_manifest() -> BundleManifest {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "diff-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"
"#,
        ).unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        ).unwrap();
        let artifact = build(BuildOptions {
            project_root: tmp.path().to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
        }).unwrap();
        let s = std::fs::read_to_string(&artifact.path).unwrap();
        BundleManifest::parse_str(&s).unwrap()
    }

    #[test]
    fn diff_ignores_sha256_and_created_at() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.bundle.sha256 = "different".into();
        b.bundle.created_at = "2099-01-01T00:00:00Z".into();
        assert!(diff_manifests(&a, &b).is_empty(), "sha256 + created_at must be excluded");
    }

    #[test]
    fn diff_reports_tau_version_skew() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.bundle.tau_version = "9.9.9".into();
        let diffs = diff_manifests(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert!(matches!(&diffs[0], ManifestDiff::BundleMetaField { field, .. } if field == "tau_version"), "got {diffs:?}");
    }

    #[test]
    fn diff_reports_project_tau_toml_sha256() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.project.tau_toml_sha256 = "ffff".into();
        let diffs = diff_manifests(&a, &b);
        assert!(
            diffs.iter().any(|d| matches!(d, ManifestDiff::ProjectField { field, .. } if field == "tau_toml_sha256")),
            "got {diffs:?}",
        );
    }

    #[test]
    fn diff_detects_added_package() {
        let a = sample_manifest();
        let mut b = a.clone();
        // Clone an existing package entry if any; else synthesize the
        // smallest valid BundlePackage. sample_manifest has zero
        // packages, so push one onto `b`.
        let pkg = crate::bundle::manifest::BundlePackage {
            name: "newpkg".into(),
            version: semver::Version::new(0, 1, 0),
            source: a.packages.first().map(|p| p.source.clone())
                .unwrap_or_else(|| "https://example.com/x.git".parse().expect("PackageSource parse")),
            tree_sha256: "0".repeat(64),
            binary_sha256: None,
            required_shapes: vec![],
        };
        b.packages.push(pkg);
        let diffs = diff_manifests(&a, &b);
        assert!(
            diffs.iter().any(|d| matches!(d, ManifestDiff::PackageMissing { name, side } if name == "newpkg" && *side == Side::RebuiltOnly)),
            "got {diffs:?}",
        );
    }

    #[test]
    fn diff_detects_removed_agent() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.agents.clear(); // shipped (a) has `solo`, rebuilt (b) has none
        let diffs = diff_manifests(&a, &b);
        assert!(
            diffs.iter().any(|d| matches!(d, ManifestDiff::AgentMissing { id, side } if id == "solo" && *side == Side::ShippedOnly)),
            "got {diffs:?}",
        );
    }
}
```

> **Implementer:** `BundlePackage.source` is a `PackageSource`. The `"…".parse()` shape above may not match `PackageSource`'s `FromStr` — read its parse impl and construct a valid value (or clone from an existing package). `BundleManifest`, `BundlePackage` field names confirmed in the plan header. `BundleAgent.id` is `AgentId` — compare via `.as_str()`.

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce::tests
```

Expected: panic at `unimplemented!("Task 2")`.

- [ ] **Step 3: Implement `diff_manifests`**

```rust
pub(crate) fn diff_manifests(shipped: &BundleManifest, rebuilt: &BundleManifest) -> Vec<ManifestDiff> {
    let mut diffs = Vec::new();

    // schema_version
    if shipped.schema_version != rebuilt.schema_version {
        diffs.push(ManifestDiff::SchemaVersionMismatch {
            shipped: shipped.schema_version,
            rebuilt: rebuilt.schema_version,
        });
    }

    // bundle meta — target + tau_version (NOT sha256, NOT created_at).
    if shipped.bundle.target != rebuilt.bundle.target {
        diffs.push(ManifestDiff::BundleMetaField {
            field: "target".into(),
            shipped: shipped.bundle.target.to_string(),
            rebuilt: rebuilt.bundle.target.to_string(),
        });
    }
    if shipped.bundle.tau_version != rebuilt.bundle.tau_version {
        diffs.push(ManifestDiff::BundleMetaField {
            field: "tau_version".into(),
            shipped: shipped.bundle.tau_version.clone(),
            rebuilt: rebuilt.bundle.tau_version.clone(),
        });
    }

    // project
    if shipped.project.name != rebuilt.project.name {
        diffs.push(ManifestDiff::ProjectField { field: "name".into(), shipped: shipped.project.name.clone(), rebuilt: rebuilt.project.name.clone() });
    }
    if shipped.project.version != rebuilt.project.version {
        diffs.push(ManifestDiff::ProjectField { field: "version".into(), shipped: shipped.project.version.to_string(), rebuilt: rebuilt.project.version.to_string() });
    }
    if shipped.project.tau_toml_sha256 != rebuilt.project.tau_toml_sha256 {
        diffs.push(ManifestDiff::ProjectField { field: "tau_toml_sha256".into(), shipped: shipped.project.tau_toml_sha256.clone(), rebuilt: rebuilt.project.tau_toml_sha256.clone() });
    }

    // packages — index by name, stable order.
    use std::collections::BTreeMap;
    let ship_pkgs: BTreeMap<&str, &_> = shipped.packages.iter().map(|p| (p.name.as_str(), p)).collect();
    let reb_pkgs: BTreeMap<&str, &_> = rebuilt.packages.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut pkg_names: Vec<&str> = ship_pkgs.keys().chain(reb_pkgs.keys()).copied().collect();
    pkg_names.sort_unstable();
    pkg_names.dedup();
    for name in pkg_names {
        match (ship_pkgs.get(name), reb_pkgs.get(name)) {
            (Some(_), None) => diffs.push(ManifestDiff::PackageMissing { name: name.to_string(), side: Side::ShippedOnly }),
            (None, Some(_)) => diffs.push(ManifestDiff::PackageMissing { name: name.to_string(), side: Side::RebuiltOnly }),
            (Some(s), Some(r)) => {
                if s.version != r.version {
                    diffs.push(ManifestDiff::PackageField { name: name.to_string(), field: "version".into(), shipped: s.version.to_string(), rebuilt: r.version.to_string() });
                }
                if s.tree_sha256 != r.tree_sha256 {
                    diffs.push(ManifestDiff::PackageField { name: name.to_string(), field: "tree_sha256".into(), shipped: s.tree_sha256.clone(), rebuilt: r.tree_sha256.clone() });
                }
                if s.source != r.source {
                    diffs.push(ManifestDiff::PackageField { name: name.to_string(), field: "source".into(), shipped: format!("{:?}", s.source), rebuilt: format!("{:?}", r.source) });
                }
                if s.binary_sha256 != r.binary_sha256 {
                    diffs.push(ManifestDiff::PackageField { name: name.to_string(), field: "binary_sha256".into(), shipped: format!("{:?}", s.binary_sha256), rebuilt: format!("{:?}", r.binary_sha256) });
                }
                if s.required_shapes != r.required_shapes {
                    diffs.push(ManifestDiff::PackageField { name: name.to_string(), field: "required_shapes".into(), shipped: format!("{:?}", s.required_shapes), rebuilt: format!("{:?}", r.required_shapes) });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    // agents — index by id, stable order.
    let ship_agents: BTreeMap<String, &_> = shipped.agents.iter().map(|a| (a.id.as_str().to_string(), a)).collect();
    let reb_agents: BTreeMap<String, &_> = rebuilt.agents.iter().map(|a| (a.id.as_str().to_string(), a)).collect();
    let mut agent_ids: Vec<String> = ship_agents.keys().chain(reb_agents.keys()).cloned().collect();
    agent_ids.sort_unstable();
    agent_ids.dedup();
    for id in agent_ids {
        match (ship_agents.get(&id), reb_agents.get(&id)) {
            (Some(_), None) => diffs.push(ManifestDiff::AgentMissing { id: id.clone(), side: Side::ShippedOnly }),
            (None, Some(_)) => diffs.push(ManifestDiff::AgentMissing { id: id.clone(), side: Side::RebuiltOnly }),
            (Some(s), Some(r)) => {
                if s.system_prompt_sha256 != r.system_prompt_sha256 {
                    diffs.push(ManifestDiff::AgentField { id: id.clone(), field: "system_prompt_sha256".into(), shipped: s.system_prompt_sha256.clone(), rebuilt: r.system_prompt_sha256.clone() });
                }
                if s.backend != r.backend {
                    diffs.push(ManifestDiff::AgentField { id: id.clone(), field: "backend".into(), shipped: format!("{:?}", s.backend), rebuilt: format!("{:?}", r.backend) });
                }
                if s.required_tools != r.required_tools {
                    diffs.push(ManifestDiff::AgentField { id: id.clone(), field: "required_tools".into(), shipped: format!("{:?}", s.required_tools), rebuilt: format!("{:?}", r.required_tools) });
                }
                if s.effective_capabilities != r.effective_capabilities {
                    diffs.push(ManifestDiff::AgentField { id: id.clone(), field: "effective_capabilities".into(), shipped: format!("{:?}", s.effective_capabilities), rebuilt: format!("{:?}", r.effective_capabilities) });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    diffs
}
```

> **Implementer:** the field types must impl `PartialEq` for the `!=` compares (they derive it per §C.1/§C.2 — `BundleManifest` itself derives `PartialEq, Eq`). If `effective_capabilities` or `backend` don't impl `Eq` cleanly, the `!=` still works via `PartialEq`. Use `{:?}` (Debug) for the shipped/rebuilt string rendering of non-String fields.

- [ ] **Step 4: Confirm PASS**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce::tests
```

All 5 diff tests pass.

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/reproduce.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle/reproduce): diff_manifests field-level comparison"
```

---

## Task 3: `verify_reproducible` pipeline

**Files:**
- Modify: `crates/tau-pkg/src/bundle/reproduce.rs`

- [ ] **Step 1: Failing tests**

Append to the `tests` module:

```rust
fn build_minimal_bundle(root: &std::path::Path) -> std::path::PathBuf {
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "repro-fixture"
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
    }).unwrap();
    artifact.path
}

fn ropts(bundle: std::path::PathBuf, root: &std::path::Path) -> ReproOptions {
    ReproOptions { bundle_path: bundle, project_root: root.to_path_buf() }
}

#[test]
fn reproducible_when_tree_unchanged() {
    let tmp = tempdir().unwrap();
    let bundle = build_minimal_bundle(tmp.path());
    let report = verify_reproducible(ropts(bundle, tmp.path())).expect("repro check ran");
    assert!(report.reproducible, "clean rebuild must reproduce; diffs={:?}", report.diffs);
    assert_eq!(report.shipped_sha256, report.rebuilt_sha256);
    assert!(report.diffs.is_empty());
}

#[test]
fn not_reproducible_when_tau_toml_changes() {
    let tmp = tempdir().unwrap();
    let bundle = build_minimal_bundle(tmp.path());
    // Edit tau.toml after build so the rebuild's tau_toml_sha256 differs.
    std::fs::write(
        tmp.path().join("tau.toml"),
        r#"
[project]
name = "repro-fixture"
version = "0.2.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are solo"
"#,
    ).unwrap();
    let report = verify_reproducible(ropts(bundle, tmp.path())).expect("repro check ran");
    assert!(!report.reproducible);
    assert!(
        report.diffs.iter().any(|d| matches!(d, ManifestDiff::ProjectField { field, .. } if field == "tau_toml_sha256")),
        "expected tau_toml_sha256 diff; got {:?}", report.diffs,
    );
}

#[test]
fn repro_error_when_bundle_missing() {
    let tmp = tempdir().unwrap();
    let err = verify_reproducible(ropts(tmp.path().join("nope.tau"), tmp.path())).unwrap_err();
    assert!(matches!(err, ReproError::BundleRead { .. }), "got {err:?}");
}

#[test]
fn repro_error_when_shipped_bundle_corrupt() {
    let tmp = tempdir().unwrap();
    let bundle = build_minimal_bundle(tmp.path());
    let body = std::fs::read_to_string(&bundle).unwrap();
    // Flip the project name without recomputing sha256 → self-hash stale.
    let tampered = body.replacen("repro-fixture", "tampered-name", 1);
    assert_ne!(body, tampered);
    std::fs::write(&bundle, tampered).unwrap();
    let err = verify_reproducible(ropts(bundle, tmp.path())).unwrap_err();
    assert!(matches!(err, ReproError::ShippedSelfHashInvalid { .. }), "got {err:?}");
}
```

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce::tests
```

Expected: the 4 new tests panic at `unimplemented!("Task 3")`.

- [ ] **Step 3: Implement `verify_reproducible`**

```rust
pub fn verify_reproducible(opts: ReproOptions) -> Result<ReproReport, ReproError> {
    // 1. Read + parse the shipped bundle.
    let shipped_str = std::fs::read_to_string(&opts.bundle_path)
        .map_err(|e| ReproError::BundleRead { path: opts.bundle_path.clone(), source: e })?;
    let shipped = BundleManifest::parse_str(&shipped_str)
        .map_err(|e| ReproError::BundleParse { source: e })?;

    // 2. The shipped bundle must be valid before we compare against it.
    crate::bundle::hash::verify_self_hash(&shipped)
        .map_err(|e| ReproError::ShippedSelfHashInvalid { detail: e.to_string() })?;

    // 3. Rebuild from the local tree using the shipped target, to a temp path.
    let tmp = tempfile::TempDir::new().map_err(|e| ReproError::TempDir { source: e })?;
    let rebuilt_path = tmp.path().join("rebuilt.tau");
    let artifact = crate::bundle::build(crate::bundle::BuildOptions {
        project_root: opts.project_root.clone(),
        target: shipped.bundle.target,
        output_path: Some(rebuilt_path.clone()),
    })
    .map_err(|e| ReproError::Rebuild { source: e })?;

    // 4. Parse the rebuilt bundle.
    let rebuilt_str = std::fs::read_to_string(&artifact.path)
        .map_err(|e| ReproError::RebuiltRead { path: artifact.path.clone(), source: e })?;
    let rebuilt = BundleManifest::parse_str(&rebuilt_str)
        .map_err(|e| ReproError::RebuiltParse { source: e })?;

    // 5. Verdict + diff.
    let reproducible = shipped.bundle.sha256 == rebuilt.bundle.sha256;
    let diffs = if reproducible { Vec::new() } else { diff_manifests(&shipped, &rebuilt) };

    Ok(ReproReport {
        reproducible,
        shipped_sha256: shipped.bundle.sha256.clone(),
        rebuilt_sha256: rebuilt.bundle.sha256.clone(),
        diffs,
    })
}
```

> **Implementer:** `crate::bundle::build` + `crate::bundle::BuildOptions` are the re-exported paths (confirm in mod.rs — they're `pub use build::{build, BuildOptions, BundleArtifact}`). `shipped.bundle.target` is `Copy` (`TargetTriple`).

- [ ] **Step 4: Confirm PASS**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
```

All pass — especially `reproducible_when_tree_unchanged` (proves clean rebuild is bit-stable).

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/reproduce.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg/bundle/reproduce): verify_reproducible rebuild-and-compare"
```

---

## Task 4: build→reproduce e2e integration test

**Files:**
- Create: `crates/tau-pkg/tests/bundle_reproduce_e2e.rs`

- [ ] **Step 1: Write the test**

Reuse the §C.2/§C.3 e2e fixture (read `crates/tau-pkg/tests/bundle_build_e2e.rs` for the verbatim `write_fixture` — 2 packages + 2 agents, `fs-read`/`critic`, `researcher`/`writer`).

```rust
//! End-to-end: build a bundle, then reproducibility-check it against
//! the same tree (reproducible) and a mutated tree (not reproducible).

use tau_pkg::bundle::{build, verify_reproducible, BuildOptions, ManifestDiff, ReproOptions};
use tau_ports::target::TargetTriple;

fn write_fixture(root: &std::path::Path) {
    // Paste write_fixture() from bundle_build_e2e.rs verbatim.
}

#[test]
fn e2e_clean_rebuild_is_reproducible() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
    }).unwrap();
    let report = verify_reproducible(ReproOptions {
        bundle_path: artifact.path,
        project_root: tmp.path().to_path_buf(),
    }).unwrap();
    assert!(report.reproducible, "diffs={:?}", report.diffs);
}

#[test]
fn e2e_mutated_package_breaks_reproducibility() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
    }).unwrap();
    // Mutate a file inside an installed package (adapt path to whatever
    // write_fixture creates, e.g. fs-read/0.1.0/src/lib.rs).
    let f = tmp.path().join(".tau/packages/fs-read/0.1.0/src/lib.rs");
    std::fs::write(&f, "// mutated after build\n").unwrap();
    let report = verify_reproducible(ReproOptions {
        bundle_path: artifact.path,
        project_root: tmp.path().to_path_buf(),
    }).unwrap();
    assert!(!report.reproducible);
    assert!(
        report.diffs.iter().any(|d| matches!(d, ManifestDiff::PackageField { name, field, .. } if name == "fs-read" && field == "tree_sha256")),
        "expected fs-read tree_sha256 diff; got {:?}", report.diffs,
    );
}
```

- [ ] **Step 2: Run + Step 3: Commit**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg --test bundle_reproduce_e2e
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/tests/bundle_reproduce_e2e.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(tau-pkg/bundle/reproduce): build-then-reproduce e2e"
```

---

## Task 5: CLI `--bundle` flag + reproducibility branch

**Files:**
- Modify: `crates/tau-cli/src/cli.rs`
- Modify: `crates/tau-cli/src/cmd/verify.rs`

- [ ] **Step 1: Add `--bundle` to `VerifyArgs`**

In `cli.rs`, inside `pub struct VerifyArgs`:

```rust
    /// Reproducibility check: rebuild a bundle from the local tree and
    /// compare to this `.tau` file. Mutually exclusive with the package
    /// positional. Exit 0 reproducible / 2 not / 3 not-installed
    /// (Phase 2 §E).
    #[arg(long, value_name = "PATH", conflicts_with = "package")]
    pub bundle: Option<std::path::PathBuf>,
```

(`conflicts_with = "package"` makes clap enforce mutual exclusion. Confirm the field name of the package positional is `package` — it is.)

- [ ] **Step 2: Branch in `cmd::verify::run`**

At the top of `run()`, before the existing scope/package logic, add:

```rust
    if let Some(bundle_path) = &args.bundle {
        return run_reproducibility_check(bundle_path, output);
    }
```

Implement `run_reproducibility_check`. Read how the existing `run` uses `output` for human vs JSON (grep for `output.json` / `Output` methods in verify.rs to learn the API — it likely has `output.is_json()` or the global `--json` is checked through `output`). Match that pattern.

```rust
fn run_reproducibility_check(bundle_path: &std::path::Path, output: &mut Output) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let report = match tau_pkg::bundle::verify_reproducible(tau_pkg::bundle::ReproOptions {
        bundle_path: bundle_path.to_path_buf(),
        project_root: cwd,
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(repro_error_exit_code(&e));
        }
    };

    if output.is_json() {  // adapt to the real Output API
        render_repro_json(&report, output)?;
    } else {
        render_repro_human(&report, output)?;
    }

    if report.reproducible {
        Ok(())
    } else {
        std::process::exit(2);
    }
}

fn repro_error_exit_code(e: &tau_pkg::bundle::ReproError) -> i32 {
    use tau_pkg::bundle::ReproError as E;
    use tau_pkg::BuildError;
    match e {
        E::BundleRead { .. } | E::BundleParse { .. } | E::ShippedSelfHashInvalid { .. } => 2,
        E::Rebuild { source: BuildError::MissingLockfile }
        | E::Rebuild { source: BuildError::PackageNotInstalled { .. } } => 3,
        E::TempDir { .. } | E::RebuiltRead { .. } | E::RebuiltParse { .. } | E::Rebuild { .. } => 70,
    }
}
```

> **Implementer:** confirm `BuildError`'s variant names (`MissingLockfile`, `PackageNotInstalled`) — they're from §C.2. The `E::Rebuild { source: BuildError::X }` nested match needs `BuildError` to be matchable; if the variants don't line up, fall back to matching `E::Rebuild { source }` then inspecting `source`. Confirm the `Output` JSON-detection API (`is_json()` is a guess — read verify.rs's existing renderer to see how it branches human vs json; mirror exactly). Exit mechanism `std::process::exit` matches `cmd::build`/`cmd::run --bundle` (§C.2/§C.3).

- [ ] **Step 3: Implement the renderers**

```rust
fn render_repro_human(report: &tau_pkg::bundle::ReproReport, _output: &mut Output) -> anyhow::Result<()> {
    fn abbrev(h: &str) -> String {
        if h.len() <= 12 { h.to_string() } else { format!("{}…{}", &h[..6], &h[h.len()-6..]) }
    }
    if report.reproducible {
        println!("✓ Reproducible — rebuilt bundle matches (sha256: {})", abbrev(&report.shipped_sha256));
    } else {
        eprintln!("✗ NOT reproducible");
        eprintln!("  shipped: {}", abbrev(&report.shipped_sha256));
        eprintln!("  rebuilt: {}", abbrev(&report.rebuilt_sha256));
        eprintln!("  divergences:");
        for d in &report.diffs {
            eprintln!("    - {}", format_diff(d));
        }
    }
    Ok(())
}

fn format_diff(d: &tau_pkg::bundle::ManifestDiff) -> String {
    use tau_pkg::bundle::ManifestDiff as D;
    match d {
        D::ProjectField { field, shipped, rebuilt } => format!("project {field}: {shipped} → {rebuilt}"),
        D::PackageMissing { name, side } => format!("package `{name}` present only in {side:?}"),
        D::PackageField { name, field, shipped, rebuilt } => format!("package `{name}` {field}: {shipped} → {rebuilt}"),
        D::AgentMissing { id, side } => format!("agent `{id}` present only in {side:?}"),
        D::AgentField { id, field, shipped, rebuilt } => format!("agent `{id}` {field}: {shipped} → {rebuilt}"),
        D::BundleMetaField { field, shipped, rebuilt } => format!("{field}: {shipped} → {rebuilt}"),
        D::SchemaVersionMismatch { shipped, rebuilt } => format!("schema_version: {shipped} → {rebuilt}"),
    }
}

fn render_repro_json(report: &tau_pkg::bundle::ReproReport, _output: &mut Output) -> anyhow::Result<()> {
    use tau_pkg::bundle::ManifestDiff as D;
    let diffs: Vec<serde_json::Value> = report.diffs.iter().map(|d| match d {
        D::ProjectField { field, shipped, rebuilt } => serde_json::json!({"kind":"project_field","field":field,"shipped":shipped,"rebuilt":rebuilt}),
        D::PackageMissing { name, side } => serde_json::json!({"kind":"package_missing","name":name,"side":format!("{side:?}")}),
        D::PackageField { name, field, shipped, rebuilt } => serde_json::json!({"kind":"package_field","name":name,"field":field,"shipped":shipped,"rebuilt":rebuilt}),
        D::AgentMissing { id, side } => serde_json::json!({"kind":"agent_missing","id":id,"side":format!("{side:?}")}),
        D::AgentField { id, field, shipped, rebuilt } => serde_json::json!({"kind":"agent_field","id":id,"field":field,"shipped":shipped,"rebuilt":rebuilt}),
        D::BundleMetaField { field, shipped, rebuilt } => serde_json::json!({"kind":"bundle_meta_field","field":field,"shipped":shipped,"rebuilt":rebuilt}),
        D::SchemaVersionMismatch { shipped, rebuilt } => serde_json::json!({"kind":"schema_version_mismatch","shipped":shipped,"rebuilt":rebuilt}),
    }).collect();
    let obj = serde_json::json!({
        "reproducible": report.reproducible,
        "shipped_sha256": report.shipped_sha256,
        "rebuilt_sha256": report.rebuilt_sha256,
        "diffs": diffs,
    });
    println!("{}", serde_json::to_string(&obj)?);
    Ok(())
}
```

> **Implementer:** the renderers use `println!`/`eprintln!` directly. If verify.rs routes ALL output through the `Output` struct (for capture in tests), use that API instead — read the existing renderers and match. Whether reproducible-success goes to stdout vs stderr: the verdict line on stdout is fine, but mirror how the existing verify renders its summary.

- [ ] **Step 4: Build + smoke**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-cli
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo run -p tau-cli -- verify --help 2>&1 | grep -A1 -i bundle
```

Expected: `--bundle <PATH>` in `verify --help`.

- [ ] **Step 5: Regenerate help snapshots**

```bash
cd crates/tau-cli && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/titouanlebocq/code/tau-worktrees/tau-verify-bundle/target/agent-impl cargo insta test --accept --test help_snapshots
cd /Users/titouanlebocq/code/tau-worktrees/tau-verify-bundle && timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test help_snapshots
```

`verify_help` snapshot updates; all pass.

- [ ] **Step 6: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/verify.rs crates/tau-cli/tests/snapshots/ && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-cli): tau verify --bundle reproducibility check"
```

---

## Task 6: CLI integration tests

**Files:**
- Create: `crates/tau-cli/tests/cmd_verify_bundle.rs`

- [ ] **Step 1: Write the tests**

Mirror `crates/tau-cli/tests/cmd_run_bundle.rs` (created in §C.3) for the build-via-binary + TAU_HOME pattern. Each test: `tau build` to make a bundle, then `tau verify --bundle <path>`.

```rust
//! Integration tests for `tau verify --bundle` (Phase 2 §E).

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;
use predicates::prelude::*;

// Copy the write_minimal_project + TAU_HOME helpers from cmd_run_bundle.rs
// / cmd_build.rs. Build a bundle via the `tau` binary; return its path
// from `tau build`'s stdout.

#[test]
fn verify_bundle_reproducible_exits_zero() {
    // stand up project + empty-package lockfile, tau build, then
    // tau verify --bundle <path> on the UNCHANGED tree → exit 0,
    // stdout/stderr contains "Reproducible".
}

#[test]
fn verify_bundle_drift_exits_two() {
    // build with one installed package, mutate a file in it, verify →
    // exit 2, stderr "NOT reproducible" + the package name.
}

#[test]
fn verify_bundle_not_installed_exits_three() {
    // build with one package, delete the package dir, verify → exit 3.
}

#[test]
fn verify_bundle_json_emits_structured_result() {
    // build, mutate, tau verify --bundle <path> --json → parse stdout
    // as JSON, assert obj["reproducible"] == false and obj["diffs"] is
    // a non-empty array.
}
```

Flesh out each body fully using the cmd_run_bundle.rs / cmd_build.rs helpers. For the one-package fixture, mirror the lockfile + `.tau/packages/<name>/<version>/` shape from those files. `--json` is the global flag (`tau verify --bundle <path> --json`); confirm placement by checking how cmd_build.rs / other tests pass `--json`.

> **Implementer:** the reproducible-exits-zero test needs `tau build` to succeed on the fixture (zero-package lockfile is fine — build iterates lockfile.packages). The verify rebuild then also succeeds (no packages to install). This should give a true exit 0, unlike §C.3's clean test which needed a full run. Confirm.

- [ ] **Step 2: Run**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_verify_bundle
```

- [ ] **Step 3: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/tests/cmd_verify_bundle.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(tau-cli): tau verify --bundle integration tests"
```

---

## Task 7: stale-comment fix + final verify + spec accept + PR

**Files:**
- Modify: `crates/tau-pkg/src/bundle/manifest.rs` (fix `created_at` doc-comment)
- Modify: `docs/superpowers/specs/2026-05-28-tau-verify-bundle-design.md` (Status → Accepted)

- [ ] **Step 1: Fix the stale `created_at` doc-comment**

In `manifest.rs`, `BundleMeta::created_at`'s doc currently reads "Informational; in the hash. Reproducibility is §E's problem." That's wrong — `compute_self_hash` excludes it. Change to:

```rust
    /// RFC 3339 UTC timestamp. Informational; **excluded** from the
    /// self-hash (see `compute_self_hash`) so rebuilds at different
    /// times reproduce the same hash. §E relies on this.
    pub created_at: String,
```

- [ ] **Step 2: Flip spec status** to `Accepted`.

- [ ] **Step 3: Full verification matrix**

```bash
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt --all -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

All clean. `cargo fmt --all` to fix drift first.

- [ ] **Step 4: Commit + push + PR**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-pkg/src/bundle/manifest.rs docs/superpowers/specs/2026-05-28-tau-verify-bundle-design.md && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "docs: tau verify --bundle accept §E + fix stale created_at comment"
git push --no-verify -u origin HEAD
```

PR title: `feat(tau-pkg): tau verify --bundle reproducibility check (Phase 2 §E)`. Body recaps: rebuild-and-compare, field-diff, tau_version-in-hash decision, exit codes, deferred items. Note `--no-verify` push (Podman gate); CI is the gate.

```bash
gh pr create --title "feat(tau-pkg): tau verify --bundle reproducibility check (Phase 2 §E)" --body "<recap>"
gh pr merge --auto $(gh pr list --head feat/tau-verify-bundle --json number --jq '.[0].number')
```

---

## Self-review pass

**Spec coverage:**
- Spec §2 (rebuild-and-compare / field-diff / tau_version-in-hash / --bundle flag / logic-in-tau-pkg) → Tasks 1-6.
- Spec §3 (module + API) → Task 1.
- Spec §4 (pipeline) → Task 3; §4.1 (diff_manifests) → Task 2.
- Spec §5 (CLI + output) → Task 5; §5.1 (exit codes) → Task 5 `repro_error_exit_code` + the reproducible/not branches.
- Spec §6 (errors) → Task 1.
- Spec §7 (test plan) → Tasks 2 (diff unit), 3 (pipeline unit), 4 (e2e), 6 (CLI).

**Type consistency:** `verify_reproducible`, `ReproOptions`, `ReproReport`, `ManifestDiff`, `Side`, `diff_manifests`, `ReproError` consistent across tasks. `ManifestDiff` variants used in Task 2 (construction), Task 5 (`format_diff`/`render_repro_json`), Task 6 (assertions) all match the Task 1 definition.

**Placeholder scan:** Task 6 test bodies are sketched with full intent + the tricky bits (which exit code, which substring, json shape) spelled out — the implementer fleshes them using the cmd_run_bundle.rs helpers (an established, readable pattern). Tasks 1-5 (load-bearing logic) have complete code.

**Known integration risks flagged:** (1) the `Output` JSON-detection API in verify.rs is a guess (`is_json()`) — Task 5 instructs reading the existing renderer and matching. (2) `PackageSource::FromStr` shape for the synthesized test package in Task 2 — flagged to read the parse impl. (3) `BuildError` nested-match in `repro_error_exit_code` — flagged with a fallback.
