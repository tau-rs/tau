# Tau bundle format — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Phase 2 §C.1 — a `tau-pkg::bundle` data module + canonical TOML emitter + self-hash verification. Pure-data crate; no CLI surface.

**Architecture:** Single `.tau` TOML file. Schema is `BundleManifest` (4 sub-structs). Canonical TOML emitter writes a deterministic byte stream; the self-hash is SHA-256 of that stream with `bundle.sha256` zeroed during hashing. Verification reads → zeroes → re-emits canonically → SHA-256s → compares.

**Tech Stack:** Rust 2024, `serde` + `toml` + `sha2` + `semver` + `thiserror` (all already workspace deps in tau-pkg). One added Cargo.toml feature: enable `serde` on the `tau-ports` dep so `TargetTriple` serializes.

**Spec:** `docs/superpowers/specs/2026-05-19-bundle-format-design.md` (commit `7765ff0`).

**Cargo rules (CLAUDE.md):** every cargo invocation uses `CARGO_TARGET_DIR=target/main`, `CARGO_INCREMENTAL=0`, `-p tau-pkg`, wrapped with `timeout`. Template:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-pkg
```

**Worktree:** `~/code/tau-worktrees/bundle-format`, branch `feat/bundle-format`, based on `origin/main` at `a57c07d`.

---

### Task 1: `BundleManifest` structs + parser + module scaffold (TDD)

**Files:**
- Modify: `crates/tau-pkg/Cargo.toml` (add `"serde"` to tau-ports features)
- Modify: `crates/tau-pkg/src/lib.rs` (add `pub mod bundle;` + a `pub use` block)
- Create: `crates/tau-pkg/src/bundle/mod.rs`
- Create: `crates/tau-pkg/src/bundle/manifest.rs`
- Create: `crates/tau-pkg/src/bundle/error.rs`

- [ ] **Step 1: Enable serde on tau-ports dep**

Find the existing `tau-ports = { workspace = true, features = ["test-fixtures"] }` line in `crates/tau-pkg/Cargo.toml`. Change to:

```toml
tau-ports = { workspace = true, features = ["test-fixtures", "serde"] }
```

- [ ] **Step 2: Write failing tests in `manifest.rs`**

Create `crates/tau-pkg/src/bundle/manifest.rs` (full content; tests at bottom will fail to compile until impls land in Step 3):

```rust
//! `BundleManifest` and its sub-structs. See spec §4 + §6.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tau_domain::{AgentId, CapabilityShape, PackageSource};
use tau_ports::target::TargetTriple;

use crate::bundle::error::BundleParseError;

/// Top-level bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// Major schema version. v1.x is the current line; v2+ would be a
    /// breaking change (consumer rejects loudly).
    pub schema_version: u32,
    /// Bundle-level metadata (sha + timestamp + tau version + target).
    pub bundle: BundleMeta,
    /// Project identity (name, version, source tau.toml hash).
    pub project: ProjectInfo,
    /// Resolved packages (lockfile-equivalent set).
    #[serde(default)]
    pub packages: Vec<BundlePackage>,
    /// Per-agent compiled grant set + system prompt hash + tool list.
    #[serde(default)]
    pub agents: Vec<BundleAgent>,
}

/// Bundle-level metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMeta {
    /// Self-hash. SHA-256 of the canonical TOML serialization with
    /// this field set to the empty string. See spec §5.
    pub sha256: String,
    /// RFC 3339 UTC timestamp. Informational; in the hash. Reproducibility
    /// is §E's problem.
    pub created_at: String,
    /// tau binary version that produced this bundle.
    pub tau_version: String,
    /// Deployment target.
    pub target: TargetTriple,
}

/// Project identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Project name from `[project]` table in source tau.toml.
    pub name: String,
    /// Project version (semver).
    pub version: semver::Version,
    /// SHA-256 of the source tau.toml bytes (hex).
    pub tau_toml_sha256: String,
}

/// One resolved package in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundlePackage {
    /// Package name.
    pub name: String,
    /// Resolved version (semver).
    pub version: semver::Version,
    /// Source location (git URL + ref, local path, etc.).
    pub source: PackageSource,
    /// SHA-256 of the package tree (output of `tau-pkg::tree_hash`).
    pub tree_sha256: String,
    /// SHA-256 of the plugin binary, if this is a plugin package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha256: Option<String>,
    /// Capability shapes this package's plugin needs the host to enforce.
    #[serde(default)]
    pub required_shapes: Vec<CapabilityShape>,
}

/// One agent's compiled deployment record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleAgent {
    /// Agent identifier from project tau.toml.
    pub id: AgentId,
    /// LLM backend selection (kind + model + arbitrary backend-specific extras).
    pub backend: BackendRef,
    /// SHA-256 of the agent's system prompt text (hex).
    pub system_prompt_sha256: String,
    /// Plugin names this agent depends on.
    #[serde(default)]
    pub required_tools: Vec<String>,
    /// Per-shape allow/deny lists from `compute_effective`. Omitted
    /// entirely when the agent's grant set is empty.
    #[serde(default, skip_serializing_if = "BundleEffectiveCapabilities::is_empty")]
    pub effective_capabilities: BundleEffectiveCapabilities,
}

/// LLM backend reference carried in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRef {
    /// Backend kind (e.g. "ollama", "anthropic", "openai", "stub").
    pub kind: String,
    /// Model identifier, if the backend requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Forward-compat catch-all for backend-specific keys.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// Serialized form of `compute_effective`'s output for one agent.
/// All ten lists hold glob patterns; empty lists are omitted from the
/// TOML output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BundleEffectiveCapabilities {
    /// fs.read allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_fs_read: Vec<String>,
    /// fs.read deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_fs_read: Vec<String>,
    /// fs.write allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_fs_write: Vec<String>,
    /// fs.write deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_fs_write: Vec<String>,
    /// exec allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_exec: Vec<String>,
    /// exec deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_exec: Vec<String>,
    /// net.http allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_net_http: Vec<String>,
    /// net.http deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_net_http: Vec<String>,
    /// agent.spawn allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_agent_spawn: Vec<String>,
    /// agent.spawn deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_agent_spawn: Vec<String>,
}

impl BundleEffectiveCapabilities {
    /// True when every list is empty (the table can be omitted entirely).
    pub fn is_empty(&self) -> bool {
        self.allow_fs_read.is_empty()
            && self.deny_fs_read.is_empty()
            && self.allow_fs_write.is_empty()
            && self.deny_fs_write.is_empty()
            && self.allow_exec.is_empty()
            && self.deny_exec.is_empty()
            && self.allow_net_http.is_empty()
            && self.deny_net_http.is_empty()
            && self.allow_agent_spawn.is_empty()
            && self.deny_agent_spawn.is_empty()
    }
}

impl BundleManifest {
    /// Parse a bundle manifest from a TOML string.
    pub fn parse_str(s: &str) -> Result<Self, BundleParseError> {
        let manifest: BundleManifest = toml::from_str(s)?;
        if manifest.schema_version != 1 {
            return Err(BundleParseError::UnsupportedSchemaVersion {
                found: manifest.schema_version,
            });
        }
        Ok(manifest)
    }

    /// Read and parse a bundle manifest from a file.
    pub fn from_path(p: &std::path::Path) -> Result<Self, crate::bundle::error::BundleIoError> {
        let bytes = std::fs::read_to_string(p).map_err(|source| {
            crate::bundle::error::BundleIoError::Read {
                path: p.to_path_buf(),
                source,
            }
        })?;
        Ok(Self::parse_str(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> BundleManifest {
        BundleManifest {
            schema_version: 1,
            bundle: BundleMeta {
                sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
                created_at: "2026-05-19T13:42:11Z".into(),
                tau_version: "0.1.0".into(),
                target: "linux-native-strict".parse().unwrap(),
            },
            project: ProjectInfo {
                name: "support-bot".into(),
                version: semver::Version::parse("0.3.2").unwrap(),
                tau_toml_sha256: "a".repeat(64),
            },
            packages: vec![BundlePackage {
                name: "tau-plugin-fs-read".into(),
                version: semver::Version::parse("0.2.1").unwrap(),
                source: PackageSource::Git {
                    url: "https://github.com/example/fs-read.git".parse().unwrap(),
                    reference: tau_domain::GitReference::Tag("v0.2.1".into()),
                },
                tree_sha256: "1".repeat(64),
                binary_sha256: Some("2".repeat(64)),
                required_shapes: vec![CapabilityShape::FilesystemRead],
            }],
            agents: vec![BundleAgent {
                id: "researcher".parse().unwrap(),
                backend: BackendRef {
                    kind: "ollama".into(),
                    model: Some("llama3.1:8b".into()),
                    extra: BTreeMap::new(),
                },
                system_prompt_sha256: "7".repeat(64),
                required_tools: vec!["tau-plugin-fs-read".into()],
                effective_capabilities: BundleEffectiveCapabilities {
                    allow_fs_read: vec!["/data/**".into(), "/etc/agent/**".into()],
                    deny_fs_read: vec!["/data/secrets/**".into()],
                    ..Default::default()
                },
            }],
        }
    }

    #[test]
    fn manifest_round_trips_through_toml() {
        let original = sample_manifest();
        let toml_str = toml::to_string(&original).expect("serialize");
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_str_rejects_schema_version_2() {
        let toml_str = r#"
schema_version = 2

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-05-19T13:42:11Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;
        let err = BundleManifest::parse_str(toml_str).expect_err("should reject v2");
        match err {
            BundleParseError::UnsupportedSchemaVersion { found } => assert_eq!(found, 2),
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn binary_sha256_is_optional() {
        let mut m = sample_manifest();
        m.packages[0].binary_sha256 = None;
        let toml_str = toml::to_string(&m).expect("serialize");
        assert!(
            !toml_str.contains("binary_sha256"),
            "binary_sha256 should be omitted when None: {toml_str}"
        );
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed.packages[0].binary_sha256, None);
    }

    #[test]
    fn effective_capabilities_omitted_when_empty() {
        let mut m = sample_manifest();
        m.agents[0].effective_capabilities = BundleEffectiveCapabilities::default();
        let toml_str = toml::to_string(&m).expect("serialize");
        assert!(
            !toml_str.contains("effective_capabilities"),
            "table should be omitted entirely when empty: {toml_str}"
        );
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(
            parsed.agents[0].effective_capabilities,
            BundleEffectiveCapabilities::default()
        );
    }

    #[test]
    fn backend_extra_captures_unknown_keys() {
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-05-19T13:42:11Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[agents]]
id = "demo"
backend = { kind = "anthropic", model = "claude-sonnet-4-6", api_base_url = "https://custom.example/" }
system_prompt_sha256 = "7777777777777777777777777777777777777777777777777777777777777777"
"#;
        let m = BundleManifest::parse_str(toml_str).expect("parse");
        let backend = &m.agents[0].backend;
        assert_eq!(backend.kind, "anthropic");
        assert_eq!(backend.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(
            backend.extra.get("api_base_url").map(|v| v.as_str()),
            Some(Some("https://custom.example/")),
        );
    }

    #[test]
    fn unknown_top_level_field_is_accepted() {
        // Forward-compat: a future schema may add a [binaries] table.
        // v1 consumers ignore unknown top-level keys gracefully.
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-05-19T13:42:11Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[binaries]
some_future_field = "value"
"#;
        BundleManifest::parse_str(toml_str).expect("future tables ignored");
    }
}
```

- [ ] **Step 3: Implement the error types**

Create `crates/tau-pkg/src/bundle/error.rs`:

```rust
//! Error types for bundle parsing, IO, and integrity checks.

/// Errors raised when parsing bundle TOML.
#[derive(Debug, thiserror::Error)]
pub enum BundleParseError {
    /// Underlying TOML syntax/schema error.
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    /// Bundle declares a `schema_version` this binary does not support.
    #[error("unsupported schema_version {found}; this tau binary supports v1.x only")]
    UnsupportedSchemaVersion {
        /// The schema_version found in the manifest.
        found: u32,
    },
}

/// Errors raised when reading + parsing a bundle from disk.
#[derive(Debug, thiserror::Error)]
pub enum BundleIoError {
    /// Could not read the bundle file.
    #[error("could not read bundle at {path}: {source}")]
    Read {
        /// Path attempted.
        path: std::path::PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// Parsing the bundle contents failed.
    #[error(transparent)]
    Parse(#[from] BundleParseError),
}

/// Errors raised when verifying a bundle's self-hash. Used by Task 3.
#[derive(Debug, thiserror::Error)]
pub enum BundleIntegrityError {
    /// `bundle.sha256` does not match the recomputed canonical-TOML SHA-256.
    #[error("bundle self-hash mismatch: claimed {claimed}, computed {computed}")]
    HashMismatch {
        /// Hash claimed by the bundle's `bundle.sha256` field.
        claimed: String,
        /// Hash computed from the bundle's canonical-TOML form.
        computed: String,
    },
    /// `bundle.sha256` field is empty (zero-length string).
    #[error("bundle.sha256 field is empty")]
    HashFieldEmpty,
}
```

- [ ] **Step 4: Implement the module entry point**

Create `crates/tau-pkg/src/bundle/mod.rs`:

```rust
//! Tau bundle format (Phase 2 §C.1).
//!
//! See spec `docs/superpowers/specs/2026-05-19-bundle-format-design.md`
//! and ADR-0035.
//!
//! Public surface:
//! - [`BundleManifest`] — the top-level struct + sub-structs (manifest module).
//! - [`BundleParseError`] / [`BundleIoError`] / [`BundleIntegrityError`] (error module).
//! - Canonical TOML serialization (canonical module, Task 2).
//! - Self-hash compute + verify (hash module, Task 3).

pub mod error;
pub mod manifest;

pub use error::{BundleIntegrityError, BundleIoError, BundleParseError};
pub use manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundleMeta,
    BundlePackage, ProjectInfo,
};
```

- [ ] **Step 5: Add `pub mod bundle;` to `lib.rs`**

In `crates/tau-pkg/src/lib.rs`, find the alphabetic position for `bundle` (after `pub mod capability_override;` would be wrong because `b` < `c`; place it between `pub mod` lines so that `bundle` lands before `capability_override`). Read the file first:

```bash
sed -n '20,40p' /Users/titouanlebocq/code/tau-worktrees/bundle-format/crates/tau-pkg/src/lib.rs
```

The existing first `pub mod` is `pub mod capability_override;` at line 21. Insert ABOVE it:

```rust
pub mod bundle;
```

Then add a corresponding `pub use` block. Find the `pub use error::{` line (around line 40). Insert ABOVE the existing `pub use` block:

```rust
pub use bundle::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleIntegrityError, BundleIoError,
    BundleManifest, BundleMeta, BundlePackage, BundleParseError, ProjectInfo,
};
```

- [ ] **Step 6: Run tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-pkg --lib bundle 2>&1 | tail -30
```

Expected: 6 tests pass.

If `tau_domain::GitReference` has a different path or constructor than used in the `sample_manifest` helper, fix the sample helper to match. Verify with:

```bash
grep -n 'pub enum GitReference\|impl GitReference' /Users/titouanlebocq/code/tau-worktrees/bundle-format/crates/tau-domain/src/package/source.rs | head -5
```

- [ ] **Step 7: clippy**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-pkg --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 8: Commit**

Run `git status --short` first. Expected:
```
 M crates/tau-pkg/Cargo.toml
 M crates/tau-pkg/src/lib.rs
?? crates/tau-pkg/src/bundle/
```

If anything else appears, REPORT BLOCKED.

Note: `.gitignore` matches `**/target` so files inside `crates/tau-pkg/src/bundle/` may need `git add -f` — same pattern as the `tau-ports::target` module from PR #190.

```bash
git add crates/tau-pkg/Cargo.toml crates/tau-pkg/src/lib.rs
git add -f crates/tau-pkg/src/bundle/mod.rs crates/tau-pkg/src/bundle/manifest.rs crates/tau-pkg/src/bundle/error.rs
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg): BundleManifest + parser + error types (§C.1 part 1)

Adds tau-pkg::bundle module with BundleManifest, 5 sub-structs, and 3
error enums. Enables serde feature on tau-ports dep so TargetTriple
round-trips through TOML. 6 unit tests cover round-trip parse,
schema_version rejection, optional fields, and forward-compat catch-all
for backend extras and unknown top-level tables."
```

---

### Task 2: Canonical TOML emitter (TDD)

**Files:**
- Create: `crates/tau-pkg/src/bundle/canonical.rs`
- Modify: `crates/tau-pkg/src/bundle/mod.rs` (re-export `to_canonical_toml`)
- Modify: `crates/tau-pkg/src/bundle/manifest.rs` (add `to_canonical_toml` method on `BundleManifest`)

- [ ] **Step 1: Write failing tests in `canonical.rs`**

Create `crates/tau-pkg/src/bundle/canonical.rs`:

```rust
//! Canonical TOML serialization for `BundleManifest`.
//!
//! `serde` + `toml`'s default emitter does not guarantee deterministic
//! field ordering across versions; we hand-roll a small emitter that
//! writes fields in a fixed order so the self-hash is reproducible.

use std::fmt::Write;

use crate::bundle::manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundlePackage,
};

/// Emit the canonical TOML serialization of a `BundleManifest`.
///
/// Field order is fixed; arrays of tables (`[[packages]]`, `[[agents]]`)
/// are emitted in the order they appear in the input struct. Empty
/// optional values are omitted per the spec.
pub fn to_canonical_toml(manifest: &BundleManifest) -> String {
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "schema_version = {}", manifest.schema_version);

    // [bundle]
    out.push('\n');
    out.push_str("[bundle]\n");
    write_str_kv(&mut out, "sha256", &manifest.bundle.sha256);
    write_str_kv(&mut out, "created_at", &manifest.bundle.created_at);
    write_str_kv(&mut out, "tau_version", &manifest.bundle.tau_version);
    write_str_kv(&mut out, "target", &manifest.bundle.target.to_string());

    // [project]
    out.push('\n');
    out.push_str("[project]\n");
    write_str_kv(&mut out, "name", &manifest.project.name);
    write_str_kv(&mut out, "version", &manifest.project.version.to_string());
    write_str_kv(&mut out, "tau_toml_sha256", &manifest.project.tau_toml_sha256);

    // [[packages]]
    for pkg in &manifest.packages {
        out.push('\n');
        out.push_str("[[packages]]\n");
        write_package(&mut out, pkg);
    }

    // [[agents]]
    for agent in &manifest.agents {
        out.push('\n');
        out.push_str("[[agents]]\n");
        write_agent(&mut out, agent);
    }

    out
}

fn write_package(out: &mut String, pkg: &BundlePackage) {
    write_str_kv(out, "name", &pkg.name);
    write_str_kv(out, "version", &pkg.version.to_string());
    // PackageSource serializes to a string form via its own serde impl;
    // we use that for consistency.
    let source_str = match toml::Value::try_from(&pkg.source) {
        Ok(toml::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(e) => panic!("PackageSource serialization must succeed for canonical TOML: {e}"),
    };
    write_str_kv(out, "source", &source_str);
    write_str_kv(out, "tree_sha256", &pkg.tree_sha256);
    if let Some(bin) = &pkg.binary_sha256 {
        write_str_kv(out, "binary_sha256", bin);
    }
    if !pkg.required_shapes.is_empty() {
        write_string_array(
            out,
            "required_shapes",
            pkg.required_shapes
                .iter()
                .map(capability_shape_to_str)
                .collect::<Vec<_>>(),
        );
    }
}

fn write_agent(out: &mut String, agent: &BundleAgent) {
    write_str_kv(out, "id", agent.id.as_str());
    write_backend_inline(out, &agent.backend);
    write_str_kv(out, "system_prompt_sha256", &agent.system_prompt_sha256);
    if !agent.required_tools.is_empty() {
        write_string_array(
            out,
            "required_tools",
            agent.required_tools.iter().cloned().collect(),
        );
    }
    if !agent.effective_capabilities.is_empty() {
        out.push_str("\n[agents.effective_capabilities]\n");
        write_effective_capabilities(out, &agent.effective_capabilities);
    }
}

fn write_backend_inline(out: &mut String, backend: &BackendRef) {
    out.push_str("backend = { ");
    write!(out, "kind = {}", toml_string(&backend.kind)).unwrap();
    if let Some(model) = &backend.model {
        write!(out, ", model = {}", toml_string(model)).unwrap();
    }
    for (k, v) in &backend.extra {
        // Use toml's value-string emitter for the value side; it handles
        // strings, integers, bools, arrays consistently.
        let v_toml = v.to_string();
        write!(out, ", {} = {}", toml_bare_key(k), v_toml).unwrap();
    }
    out.push_str(" }\n");
}

fn write_effective_capabilities(out: &mut String, caps: &BundleEffectiveCapabilities) {
    // Fixed field order matching the struct declaration in manifest.rs.
    if !caps.allow_fs_read.is_empty() {
        write_string_array(out, "allow_fs_read", caps.allow_fs_read.clone());
    }
    if !caps.deny_fs_read.is_empty() {
        write_string_array(out, "deny_fs_read", caps.deny_fs_read.clone());
    }
    if !caps.allow_fs_write.is_empty() {
        write_string_array(out, "allow_fs_write", caps.allow_fs_write.clone());
    }
    if !caps.deny_fs_write.is_empty() {
        write_string_array(out, "deny_fs_write", caps.deny_fs_write.clone());
    }
    if !caps.allow_exec.is_empty() {
        write_string_array(out, "allow_exec", caps.allow_exec.clone());
    }
    if !caps.deny_exec.is_empty() {
        write_string_array(out, "deny_exec", caps.deny_exec.clone());
    }
    if !caps.allow_net_http.is_empty() {
        write_string_array(out, "allow_net_http", caps.allow_net_http.clone());
    }
    if !caps.deny_net_http.is_empty() {
        write_string_array(out, "deny_net_http", caps.deny_net_http.clone());
    }
    if !caps.allow_agent_spawn.is_empty() {
        write_string_array(out, "allow_agent_spawn", caps.allow_agent_spawn.clone());
    }
    if !caps.deny_agent_spawn.is_empty() {
        write_string_array(out, "deny_agent_spawn", caps.deny_agent_spawn.clone());
    }
}

fn write_str_kv(out: &mut String, key: &str, value: &str) {
    writeln!(out, "{} = {}", key, toml_string(value)).unwrap();
}

fn write_string_array(out: &mut String, key: &str, items: Vec<String>) {
    out.push_str(key);
    out.push_str(" = [");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&toml_string(s));
    }
    out.push_str("]\n");
}

/// Emit a TOML basic-string literal (escapes per the TOML spec).
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                write!(&mut out, "\\u{:04X}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Emit a TOML bare key if `k` is ASCII alphanumeric/dash/underscore;
/// otherwise quote it.
fn toml_bare_key(k: &str) -> String {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        k.to_string()
    } else {
        toml_string(k)
    }
}

fn capability_shape_to_str(s: &tau_domain::CapabilityShape) -> String {
    match s {
        tau_domain::CapabilityShape::FilesystemRead => "FilesystemRead".into(),
        tau_domain::CapabilityShape::FilesystemWrite => "FilesystemWrite".into(),
        tau_domain::CapabilityShape::ProcessExec => "ProcessExec".into(),
        tau_domain::CapabilityShape::NetworkHttp => "NetworkHttp".into(),
        tau_domain::CapabilityShape::AgentSpawn => "AgentSpawn".into(),
        tau_domain::CapabilityShape::Custom { name } => format!("Custom({name})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::manifest::tests_helpers::sample_manifest;
    use crate::bundle::BundleManifest;

    #[test]
    fn canonical_serialization_is_byte_identical_on_repeat() {
        let m = sample_manifest();
        let a = to_canonical_toml(&m);
        let b = to_canonical_toml(&m);
        assert_eq!(a, b);
    }

    #[test]
    fn canonical_round_trip_parses_back_to_equal_manifest() {
        let m = sample_manifest();
        let toml_str = to_canonical_toml(&m);
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed, m);
    }

    #[test]
    fn omits_empty_effective_capabilities() {
        let mut m = sample_manifest();
        m.agents[0].effective_capabilities = BundleEffectiveCapabilities::default();
        let toml_str = to_canonical_toml(&m);
        assert!(
            !toml_str.contains("effective_capabilities"),
            "empty table should be omitted: {toml_str}"
        );
    }

    #[test]
    fn omits_missing_binary_sha256() {
        let mut m = sample_manifest();
        m.packages[0].binary_sha256 = None;
        let toml_str = to_canonical_toml(&m);
        assert!(
            !toml_str.contains("binary_sha256"),
            "None binary_sha256 should be omitted: {toml_str}"
        );
    }

    #[test]
    fn fixed_field_order_in_bundle_table() {
        let m = sample_manifest();
        let toml_str = to_canonical_toml(&m);
        let pos_sha = toml_str.find("sha256 =").expect("sha256 present");
        let pos_created = toml_str.find("created_at =").expect("created_at present");
        let pos_version = toml_str.find("tau_version =").expect("tau_version present");
        let pos_target = toml_str.find("target =").expect("target present");
        assert!(
            pos_sha < pos_created && pos_created < pos_version && pos_version < pos_target,
            "fields out of order: {toml_str}"
        );
    }
}
```

- [ ] **Step 2: Expose `sample_manifest` for cross-module test reuse**

In `crates/tau-pkg/src/bundle/manifest.rs`, the test helper `sample_manifest()` is defined inside `#[cfg(test)] mod tests`. The Task 2 + Task 3 tests need access to it. Refactor by moving the helper into a separate `tests_helpers` module that is exposed to other test modules within the bundle module.

Edit `crates/tau-pkg/src/bundle/manifest.rs`. Replace the existing `#[cfg(test)] mod tests {` block opening with:

```rust
#[cfg(test)]
pub(crate) mod tests_helpers {
    use super::*;
    use std::collections::BTreeMap;

    pub fn sample_manifest() -> BundleManifest {
        BundleManifest {
            schema_version: 1,
            bundle: BundleMeta {
                sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
                created_at: "2026-05-19T13:42:11Z".into(),
                tau_version: "0.1.0".into(),
                target: "linux-native-strict".parse().unwrap(),
            },
            project: ProjectInfo {
                name: "support-bot".into(),
                version: semver::Version::parse("0.3.2").unwrap(),
                tau_toml_sha256: "a".repeat(64),
            },
            packages: vec![BundlePackage {
                name: "tau-plugin-fs-read".into(),
                version: semver::Version::parse("0.2.1").unwrap(),
                source: PackageSource::Git {
                    url: "https://github.com/example/fs-read.git".parse().unwrap(),
                    reference: tau_domain::GitReference::Tag("v0.2.1".into()),
                },
                tree_sha256: "1".repeat(64),
                binary_sha256: Some("2".repeat(64)),
                required_shapes: vec![CapabilityShape::FilesystemRead],
            }],
            agents: vec![BundleAgent {
                id: "researcher".parse().unwrap(),
                backend: BackendRef {
                    kind: "ollama".into(),
                    model: Some("llama3.1:8b".into()),
                    extra: BTreeMap::new(),
                },
                system_prompt_sha256: "7".repeat(64),
                required_tools: vec!["tau-plugin-fs-read".into()],
                effective_capabilities: BundleEffectiveCapabilities {
                    allow_fs_read: vec!["/data/**".into(), "/etc/agent/**".into()],
                    deny_fs_read: vec!["/data/secrets/**".into()],
                    ..Default::default()
                },
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::tests_helpers::sample_manifest;
```

Delete the original `fn sample_manifest() -> BundleManifest { ... }` body from inside `mod tests` (its new home is `tests_helpers`).

The 6 existing tests inside `mod tests` continue to work; they import `sample_manifest` via `use super::tests_helpers::sample_manifest;` at the top of `mod tests`.

- [ ] **Step 3: Wire `canonical` into `mod.rs`**

Add to `crates/tau-pkg/src/bundle/mod.rs` (after the existing `pub mod manifest;` line):

```rust
pub mod canonical;

pub use canonical::to_canonical_toml;
```

Also add `to_canonical_toml` to the `pub use bundle::{...}` block in `crates/tau-pkg/src/lib.rs`.

- [ ] **Step 4: Add `to_canonical_toml` method on `BundleManifest`**

At the bottom of `BundleManifest`'s impl block in `crates/tau-pkg/src/bundle/manifest.rs`, add:

```rust
    /// Emit the canonical-TOML serialization of this manifest. See
    /// `crate::bundle::canonical::to_canonical_toml` for the format
    /// specification.
    pub fn to_canonical_toml(&self) -> String {
        crate::bundle::canonical::to_canonical_toml(self)
    }
```

- [ ] **Step 5: Run tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-pkg --lib bundle 2>&1 | tail -30
```

Expected: 6 (T1) + 5 (T2 new) = 11 tests pass.

- [ ] **Step 6: clippy**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-pkg --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 7: Commit**

`git status --short` expected:
```
 M crates/tau-pkg/src/bundle/manifest.rs
 M crates/tau-pkg/src/bundle/mod.rs
 M crates/tau-pkg/src/lib.rs
?? crates/tau-pkg/src/bundle/canonical.rs
```

```bash
git add crates/tau-pkg/src/bundle/manifest.rs crates/tau-pkg/src/bundle/mod.rs crates/tau-pkg/src/lib.rs
git add -f crates/tau-pkg/src/bundle/canonical.rs
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg): canonical TOML emitter for BundleManifest (§C.1 part 2)

Hand-rolled emitter writes fields in a fixed order so the self-hash
in Task 3 is reproducible. 5 unit tests cover round-trip equality,
byte-identical idempotency, fixed field ordering, and omit-when-empty
semantics for binary_sha256 + effective_capabilities."
```

---

### Task 3: Self-hash compute + verify (TDD)

**Files:**
- Create: `crates/tau-pkg/src/bundle/hash.rs`
- Modify: `crates/tau-pkg/src/bundle/mod.rs` (re-export hash entries)
- Modify: `crates/tau-pkg/src/bundle/manifest.rs` (add `compute_self_hash` + `verify_self_hash` methods on `BundleManifest`)
- Modify: `crates/tau-pkg/src/lib.rs` (extend `pub use bundle::{...}`)

- [ ] **Step 1: Write failing tests in `hash.rs`**

Create `crates/tau-pkg/src/bundle/hash.rs`:

```rust
//! Self-hash compute + verify for `BundleManifest`. See spec §5.

use sha2::{Digest, Sha256};

use crate::bundle::canonical::to_canonical_toml;
use crate::bundle::error::BundleIntegrityError;
use crate::bundle::manifest::BundleManifest;

/// Compute the canonical SHA-256 of a manifest with the `bundle.sha256`
/// field forced to the empty string. Does NOT mutate the input.
pub fn compute_self_hash(manifest: &BundleManifest) -> String {
    let mut clone = manifest.clone();
    clone.bundle.sha256 = String::new();
    let canonical = to_canonical_toml(&clone);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Verify that the manifest's `bundle.sha256` field equals the
/// recomputed self-hash.
pub fn verify_self_hash(manifest: &BundleManifest) -> Result<(), BundleIntegrityError> {
    if manifest.bundle.sha256.is_empty() {
        return Err(BundleIntegrityError::HashFieldEmpty);
    }
    let computed = compute_self_hash(manifest);
    if computed == manifest.bundle.sha256 {
        Ok(())
    } else {
        Err(BundleIntegrityError::HashMismatch {
            claimed: manifest.bundle.sha256.clone(),
            computed,
        })
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::manifest::tests_helpers::sample_manifest;

    #[test]
    fn compute_self_hash_is_deterministic() {
        let m = sample_manifest();
        let a = compute_self_hash(&m);
        let b = compute_self_hash(&m);
        assert_eq!(a, b);
    }

    #[test]
    fn compute_self_hash_does_not_mutate_input() {
        let m = sample_manifest();
        let original_sha = m.bundle.sha256.clone();
        let _ = compute_self_hash(&m);
        assert_eq!(m.bundle.sha256, original_sha);
    }

    #[test]
    fn compute_self_hash_returns_64_hex_chars() {
        let m = sample_manifest();
        let h = compute_self_hash(&m);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn compute_self_hash_ignores_existing_sha_value() {
        let mut m = sample_manifest();
        m.bundle.sha256 = "9".repeat(64);
        let h1 = compute_self_hash(&m);
        m.bundle.sha256 = "f".repeat(64);
        let h2 = compute_self_hash(&m);
        assert_eq!(h1, h2, "existing sha value must not affect the computed hash");
    }

    #[test]
    fn verify_self_hash_ok_when_hash_matches() {
        let mut m = sample_manifest();
        m.bundle.sha256 = compute_self_hash(&m);
        verify_self_hash(&m).expect("ok");
    }

    #[test]
    fn verify_self_hash_detects_tampered_package_version() {
        let mut m = sample_manifest();
        m.bundle.sha256 = compute_self_hash(&m);
        // Tamper after hash is set.
        m.packages[0].version = semver::Version::parse("0.2.2").unwrap();
        match verify_self_hash(&m) {
            Err(BundleIntegrityError::HashMismatch { claimed, computed }) => {
                assert_ne!(claimed, computed);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_self_hash_errors_when_field_is_empty() {
        let mut m = sample_manifest();
        m.bundle.sha256.clear();
        match verify_self_hash(&m) {
            Err(BundleIntegrityError::HashFieldEmpty) => {}
            other => panic!("expected HashFieldEmpty, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Wire `hash` into `mod.rs`**

Add to `crates/tau-pkg/src/bundle/mod.rs`:

```rust
pub mod hash;

pub use hash::{compute_self_hash, verify_self_hash};
```

Also extend `crates/tau-pkg/src/lib.rs`'s `pub use bundle::{...}` to include `compute_self_hash` and `verify_self_hash`.

- [ ] **Step 3: Add method-style wrappers on `BundleManifest`**

In `crates/tau-pkg/src/bundle/manifest.rs`, append to `BundleManifest`'s impl block:

```rust
    /// Compute the canonical self-hash of this manifest. Does not mutate.
    /// See `crate::bundle::hash::compute_self_hash`.
    pub fn compute_self_hash(&self) -> String {
        crate::bundle::hash::compute_self_hash(self)
    }

    /// Verify that this manifest's `bundle.sha256` field equals the
    /// recomputed canonical self-hash. See
    /// `crate::bundle::hash::verify_self_hash`.
    pub fn verify_self_hash(&self) -> Result<(), crate::bundle::error::BundleIntegrityError> {
        crate::bundle::hash::verify_self_hash(self)
    }
```

- [ ] **Step 4: Run tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-pkg --lib bundle 2>&1 | tail -30
```

Expected: 6 (T1) + 5 (T2) + 7 (T3 new) = 18 tests pass.

- [ ] **Step 5: clippy**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-pkg --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 6: Commit**

`git status --short` expected:
```
 M crates/tau-pkg/src/bundle/manifest.rs
 M crates/tau-pkg/src/bundle/mod.rs
 M crates/tau-pkg/src/lib.rs
?? crates/tau-pkg/src/bundle/hash.rs
```

```bash
git add crates/tau-pkg/src/bundle/manifest.rs crates/tau-pkg/src/bundle/mod.rs crates/tau-pkg/src/lib.rs
git add -f crates/tau-pkg/src/bundle/hash.rs
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-pkg): self-hash compute + verify (§C.1 part 3)

SHA-256 of canonical TOML with bundle.sha256 zeroed. compute_self_hash
is pure (no mutation). verify_self_hash returns HashMismatch on tamper
or HashFieldEmpty when the field is missing. 7 unit tests cover
determinism, non-mutation, hex format, tamper detection, and the
empty-field error path."
```

---

### Task 4: ADR-0035 + integration fixture + mdbook + final verification + PR

**Files:**
- Create: `docs/decisions/0035-bundle-format.md`
- Create: `crates/tau-pkg/tests/fixtures/bundle-support-bot.tau`
- Create: `crates/tau-pkg/tests/bundle_fixture.rs`
- Modify: `docs/SUMMARY.md` (register ADR-0035 in the "Decisions" section)

- [ ] **Step 1: Write ADR-0035**

Create `docs/decisions/0035-bundle-format.md`:

```markdown
# ADR-0035: Tau bundle format (§C.1)

**Status:** Accepted
**Date:** 2026-05-19
**Deciders:** titouanlebocq

## Context

Phase 2 §C produces the deployment artifact for tau workflows. The
scope estimate (~6 weeks) decomposes into §C.1 (this ADR — bundle
format), §C.2 (`tau build` producer), and §C.3 (`tau run --bundle`
consumer). C.1 lands first because C.2 and C.3 both depend on a
stable format.

## Decision

A bundle is a **single `.tau` TOML file**, reference-only (no
embedded plugin binaries). Schema v1 holds:

- `schema_version` (u32, currently 1)
- `[bundle]` — self-hash + created_at + tau_version + target
- `[project]` — name + version + tau_toml_sha256
- `[[packages]]` — one per resolved package (name, version, source,
  tree_sha256, optional binary_sha256, required_shapes)
- `[[agents]]` — one per agent (id, backend, system_prompt_sha256,
  required_tools, optional effective_capabilities table)

See spec §4 for the full schema and §6 for code surface.

### Self-hash

`bundle.sha256` is the SHA-256 of the canonical-TOML serialization
of the manifest with `bundle.sha256` itself set to the empty string.
A hand-written `to_canonical_toml` emitter guarantees byte-stable
output across `toml` crate versions; the same emitter is used at
both producer and consumer ends.

### Stability discipline

v1.x is **additive**:
- New optional fields land with `#[serde(default)]` defaults.
- New top-level tables are reserved (e.g., `[binaries]` for a future
  self-contained mode); v1 producers MUST NOT emit reserved tables.
  v1 consumers MUST ignore unknown top-level tables.

v2 is a **breaking change**. Consumers fail loudly
(`BundleParseError::UnsupportedSchemaVersion`) when they meet a
schema_version they don't support.

### Reference-only deferral

Self-contained bundles (with embedded plugin binaries) are deferred
indefinitely per the §C brainstorm. The reservations in v1's schema
preserve forward-compat: a future `[binaries]` table can be added
without breaking existing v1 bundles. The decision to embed binaries
is gated on a concrete air-gap or remote-runner use case.

## Out of scope

- `tau build --target` — Phase 2 §C.2.
- `tau run --bundle` — Phase 2 §C.3.
- Bundle signing / authenticity — Phase 3+.
- Cross-machine reproducibility verification — Phase 2 §E.
- Embedded plugin binaries — deferred per §C brainstorm.

## Spec

See `docs/superpowers/specs/2026-05-19-bundle-format-design.md`.
```

- [ ] **Step 2: Write a realistic bundle fixture + integration test**

Create the fixture directory:

```bash
mkdir -p /Users/titouanlebocq/code/tau-worktrees/bundle-format/crates/tau-pkg/tests/fixtures
```

The fixture's `bundle.sha256` field cannot be hard-coded here — it depends on the exact canonical-TOML output of T2's emitter. Instead, the integration test will **compute** the expected hash from a known manifest, write the bundle to a tempdir, read it back, and verify. That way the fixture is constructive and self-verifying without static hash maintenance.

Create `crates/tau-pkg/tests/bundle_fixture.rs`:

```rust
//! Integration test: realistic bundle round-trips through file IO.
//!
//! This is a Layer 3 check on top of the unit tests in `tau-pkg::bundle`.
//! It builds a plausible bundle, writes it to disk, reads it back, and
//! verifies the self-hash matches.

use tau_pkg::bundle::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundleMeta,
    BundlePackage, ProjectInfo,
};

#[test]
fn realistic_bundle_round_trips_through_disk() {
    let mut manifest = BundleManifest {
        schema_version: 1,
        bundle: BundleMeta {
            sha256: String::new(),
            created_at: "2026-05-19T13:42:11Z".into(),
            tau_version: "0.1.0".into(),
            target: "linux-native-strict".parse().unwrap(),
        },
        project: ProjectInfo {
            name: "support-bot".into(),
            version: semver::Version::parse("0.3.2").unwrap(),
            tau_toml_sha256: "a".repeat(64),
        },
        packages: vec![
            BundlePackage {
                name: "tau-plugin-fs-read".into(),
                version: semver::Version::parse("0.2.1").unwrap(),
                source: tau_domain::PackageSource::Git {
                    url: "https://github.com/example/fs-read.git".parse().unwrap(),
                    reference: tau_domain::GitReference::Tag("v0.2.1".into()),
                },
                tree_sha256: "1".repeat(64),
                binary_sha256: Some("2".repeat(64)),
                required_shapes: vec![tau_domain::CapabilityShape::FilesystemRead],
            },
            BundlePackage {
                name: "tau-plugin-shell".into(),
                version: semver::Version::parse("0.4.0").unwrap(),
                source: tau_domain::PackageSource::Git {
                    url: "https://github.com/example/shell.git".parse().unwrap(),
                    reference: tau_domain::GitReference::Tag("v0.4.0".into()),
                },
                tree_sha256: "3".repeat(64),
                binary_sha256: Some("4".repeat(64)),
                required_shapes: vec![tau_domain::CapabilityShape::ProcessExec],
            },
        ],
        agents: vec![BundleAgent {
            id: "researcher".parse().unwrap(),
            backend: BackendRef {
                kind: "ollama".into(),
                model: Some("llama3.1:8b".into()),
                extra: std::collections::BTreeMap::new(),
            },
            system_prompt_sha256: "7".repeat(64),
            required_tools: vec!["tau-plugin-fs-read".into()],
            effective_capabilities: BundleEffectiveCapabilities {
                allow_fs_read: vec!["/data/**".into()],
                deny_fs_read: vec!["/data/secrets/**".into()],
                ..Default::default()
            },
        }],
    };

    // Compute the self-hash and store it.
    manifest.bundle.sha256 = manifest.compute_self_hash();

    // Write canonical TOML to a tempdir.
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("support-bot.tau");
    std::fs::write(&path, manifest.to_canonical_toml()).expect("write");

    // Read back from disk and verify.
    let parsed = BundleManifest::from_path(&path).expect("from_path");
    parsed.verify_self_hash().expect("verify");

    // Spot-check a few fields survived the round trip.
    assert_eq!(parsed.project.name, "support-bot");
    assert_eq!(parsed.packages.len(), 2);
    assert_eq!(parsed.agents.len(), 1);
    assert_eq!(parsed.agents[0].id.as_str(), "researcher");
}

#[test]
fn tampered_bundle_on_disk_fails_verification() {
    let mut manifest = BundleManifest {
        schema_version: 1,
        bundle: BundleMeta {
            sha256: String::new(),
            created_at: "2026-05-19T13:42:11Z".into(),
            tau_version: "0.1.0".into(),
            target: "passthrough".parse().unwrap(),
        },
        project: ProjectInfo {
            name: "tiny".into(),
            version: semver::Version::parse("0.1.0").unwrap(),
            tau_toml_sha256: "a".repeat(64),
        },
        packages: vec![],
        agents: vec![],
    };
    manifest.bundle.sha256 = manifest.compute_self_hash();

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("tiny.tau");

    // Tamper: change project name AFTER hashing.
    let mut tampered = manifest.clone();
    tampered.project.name = "huge".into();
    // Keep the original (now-stale) hash; write tampered body.
    tampered.bundle.sha256 = manifest.bundle.sha256.clone();
    std::fs::write(&path, tampered.to_canonical_toml()).expect("write");

    let parsed = BundleManifest::from_path(&path).expect("from_path");
    let err = parsed.verify_self_hash().expect_err("should detect tamper");
    let msg = err.to_string();
    assert!(msg.contains("mismatch"), "unexpected error: {msg}");
}
```

Confirm `tempfile` is a dev-dep of `tau-pkg`:

```bash
grep tempfile /Users/titouanlebocq/code/tau-worktrees/bundle-format/crates/tau-pkg/Cargo.toml
```

If absent, add `tempfile = { workspace = true }` to `[dev-dependencies]`. (It is already a workspace dep.)

The fixture file under `tests/fixtures/` referenced in the spec is materialised here as a programmatic construction; that's the closest we get to a static-file fixture without committing a sha256 that drifts on every emitter tweak. The spec mentions a `fixtures/support-bot.tau` — that path is reserved for §C.2 (the producer) to write a real fixture file once `tau build` exists. For C.1, the integration test above is the canonical fixture.

- [ ] **Step 3: Register ADR-0035 in mdbook**

Read the existing `docs/SUMMARY.md`:

```bash
grep -n "ADR-0033\|ADR-0034" /Users/titouanlebocq/code/tau-worktrees/bundle-format/docs/SUMMARY.md
```

Find the line `- [ADR-0034 — Target triple registry](decisions/0034-target-triple-registry.md)`. Insert AFTER it:

```markdown
- [ADR-0035 — Bundle format](decisions/0035-bundle-format.md)
```

- [ ] **Step 4: Final fmt, clippy, nextest, doctest**

```bash
cargo fmt
timeout 30 cargo fmt --check 2>&1 | tail -3
```

Expected: empty output.

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-pkg --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean.

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg 2>&1 | tail -10
```

Expected: all tests pass (existing tau-pkg tests + 18 bundle unit tests + 2 integration tests).

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test --doc -p tau-pkg 2>&1 | tail -10
```

Expected: doctests pass (the new module's doc comments don't include `cargo test`-able examples, so this should pass trivially or report 0 tests).

- [ ] **Step 5: Commit docs + fixture + any fmt drift**

Run `git status --short`. Expected (give or take ordering):

```
 M docs/SUMMARY.md
?? docs/decisions/0035-bundle-format.md
?? crates/tau-pkg/tests/bundle_fixture.rs
```

Possibly also `M crates/tau-pkg/src/bundle/...` from cargo fmt; include those if present.

```bash
git add docs/SUMMARY.md docs/decisions/0035-bundle-format.md
git add crates/tau-pkg/tests/bundle_fixture.rs
# If cargo fmt produced changes:
git add -A crates/tau-pkg/
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "docs(adr): ADR-0035 + bundle integration tests (§C.1 part 4)

Closes Phase 2 §C.1. ADR-0035 codifies the bundle format decision
(single TOML file, reference-only, self-hash via zero-the-field
trick). 2 integration tests in tests/bundle_fixture.rs round-trip a
realistic bundle through disk and verify tamper detection."
```

- [ ] **Step 6: Push + open PR**

```bash
scripts/agent-push.sh -u origin feat/bundle-format 2>&1 | tail -10
```

If podman SSH is still broken on this host (per [[project-check-sandbox-extraction-2026-05-19]]), fall back to `git push --no-verify` — the user has pre-authorized this exception for the current session.

```bash
gh pr create --base main --head feat/bundle-format \
  --title "feat(tau-pkg): bundle format (Phase 2 §C.1)" \
  --body "$(cat <<'EOF'
## Summary

Phase 2 §C.1: pure-data bundle format crate. Single `.tau` TOML file. Reference-only (no embedded plugin binaries; deferred per §C brainstorm). New module `tau-pkg::bundle` with `BundleManifest` + 5 sub-structs, canonical TOML emitter, and self-hash compute/verify.

## Schema (v1)

| Section | Purpose |
|---|---|
| `schema_version = 1` | Forward-compat hook |
| `[bundle]` | Self-hash + created_at + tau_version + target |
| `[project]` | Name + version + tau_toml_sha256 |
| `[[packages]]` | Per-package: name + version + source + tree_sha256 + optional binary_sha256 + required_shapes |
| `[[agents]]` | Per-agent: id + backend + system_prompt_sha256 + required_tools + optional effective_capabilities table |

## Self-hash

SHA-256 of the canonical-TOML serialization with `bundle.sha256` zeroed. Hand-written `to_canonical_toml` emitter guarantees byte-stable output across `toml` crate versions.

## Forward-compat

- New optional fields land with `#[serde(default)]`.
- Unknown top-level tables are accepted by v1 consumers (forward-compat for the eventual `[binaries]` table when self-contained mode lands).
- v2+ schema breaks fail loudly with `UnsupportedSchemaVersion`.

## Test plan

- [x] 6 unit tests for parser + manifest round-trip + schema_version rejection + optional fields + forward-compat
- [x] 5 unit tests for canonical TOML emitter (byte-identity, field order, omit-when-empty)
- [x] 7 unit tests for self-hash compute + verify (determinism, non-mutation, hex format, tamper detection, empty-field error)
- [x] 2 integration tests round-tripping a realistic bundle through disk + verifying tamper detection
- [x] `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest -p tau-pkg`, doctest all green locally
- [ ] CI gate authoritative (pushed `--no-verify` due to local podman SSH issue per [[project-check-sandbox-extraction-2026-05-19]])

## Decomposition note

§C ships in three sub-PRs:
- **§C.1 (this PR)**: bundle format (pure data).
- §C.2: `tau build --target <triple>` producer — future PR.
- §C.3: `tau run --bundle <file>` consumer — future PR.

§C.4 (self-contained bundles with embedded plugin binaries) is deferred indefinitely.

## References

- Spec: \`docs/superpowers/specs/2026-05-19-bundle-format-design.md\`
- Plan: \`docs/superpowers/plans/2026-05-19-bundle-format.md\`
- ADR:  \`docs/decisions/0035-bundle-format.md\`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Capture the PR URL.

---

## Self-review checklist (applied)

- **Spec §3 strategic decisions** (reference-only, single TOML, schema_version=1, forward-compat): T1 + ADR (T4). ✓
- **Spec §4 schema**: T1 ships every field with serde annotations. ✓
- **Spec §5 self-hash methodology**: T3. ✓
- **Spec §5.1 canonical TOML**: T2. ✓
- **Spec §6 code surface**: T1 (structs + parse), T2 (canonical), T3 (hash + method wrappers). ✓
- **Spec §6.4 errors**: T1's error.rs. ✓
- **Spec §7 testing strategy**: T1 has 6 tests (parse + round-trip + schema_version + optional + forward-compat). T2 has 5 (byte-identity + ordering + omission). T3 has 7 (hash + verify + tamper + empty). T4 has 2 integration tests. 20 tests total vs. spec's "10-15"; a little over but that's fine. ✓
- **Spec §8 dependency additions** (no new ones beyond enabling `serde` on tau-ports dep): T1 Step 1. ✓
- **Spec §9 ADR**: T4. ✓
- **No placeholders**: every step has actual code or commands. ✓
- **Type consistency**: `BundleManifest`'s 5 sub-struct names match across T1, T2, T3, T4. `compute_self_hash` / `verify_self_hash` signatures match across hash.rs and the method wrappers. The `tests_helpers::sample_manifest` helper is shared by T1, T2, T3 — declared in T1, refactored to `pub(crate)` in T2. ✓
