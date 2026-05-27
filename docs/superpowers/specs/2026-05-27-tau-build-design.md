# `tau build` — Phase 2 §C.2 design

**Status:** Draft
**Date:** 2026-05-27
**Authors:** titouanlebocq
**Depends on:** ADR-0035 (bundle format, §C.1, merged 2026-05-19), ADR-0034 (target triple registry)
**Successor:** §C.3 (`tau run --bundle`) — out of scope here

## 1. Goal

Ship the **MVP `tau build` producer**: a single CLI command that gathers a fully-installed tau project's resolved state and emits a `.tau` bundle file matching the §C.1 schema. The bundle is reference-only (no embedded plugin binaries) and byte-stable across builds of identical sources.

Non-goals (deferred):

- `--target <triple>` multi-target builds → §C.2.1
- `-o <path>` custom output → §C.2.1
- `--agent <id>` per-agent slicing → §C.2.1
- `--json` machine-readable output → §C.2.1
- `tau run --bundle` consumer → §C.3
- Bundle signing / authenticity → Phase 3+
- Cross-machine reproducibility verify → §E
- Self-contained bundles (embedded plugin binaries) → indefinitely deferred per ADR-0035

## 2. Headline decisions

- **MVP surface.** `tau build` with no flags. Target defaults to host triple. Output path is `<project-root>/<project-name>-<project-version>.tau` (no `-o`).
- **Strict install state required.** If `tau.lock` is missing or any locked package isn't installed on disk, build fails with a remediation hint (`run `tau install` first`). Mirrors `cargo package` semantics. Build remains a pure-read operation; no implicit install side-effects.
- **Stable bundle hashes.** `bundle.created_at` is informational only. `bundle.sha256` is computed over the canonical TOML with **both** `bundle.sha256` AND `bundle.created_at` zeroed. Two builds of identical sources produce byte-identical `sha256`. This requires a one-line amendment to §C.1's `compute_self_hash`.
- **Producer code lives in `tau-pkg`**, next to `tree_hash`, `verify`, etc. Thin CLI shim in `tau-cli::cmd::build`. No new crate.

## 3. Module structure

```
crates/tau-pkg/src/bundle/
├── mod.rs          (re-exports + public surface)
├── manifest.rs     (§C.1 — BundleManifest + sub-structs)        [exists]
├── canonical.rs    (§C.1 — to_canonical_toml)                   [exists]
├── hash.rs         (§C.1 — compute_self_hash + verify)          [exists, amend §3.1]
├── build.rs        (§C.2 — gather + assemble + write)           [new]
└── build_error.rs  (§C.2 — BuildError enum)                     [new]

crates/tau-cli/src/cmd/build.rs                                   [new]
```

### 3.1 §C.1 amendment — `hash.rs`

`compute_self_hash` currently zeros only `bundle.sha256` before canonicalizing. Update to also zero `bundle.created_at` (set to `chrono::DateTime::<Utc>::UNIX_EPOCH` or equivalent sentinel — must be a stable, valid RFC3339 value so the canonical emitter accepts it). One additional unit test asserts that two manifests differing only in `created_at` produce identical hashes.

This change is **backwards-incompatible** for any bundle written by §C.1's hash impl in the small window between #206 merging (2026-05-19) and §C.2 shipping. Since no bundles are produced by users yet (no `tau build` exists until §C.2), this is a paper risk only. §C.2 ships the amendment in the same PR.

## 4. Build pipeline

Public API:

```rust
pub fn build(opts: BuildOptions) -> Result<BundleArtifact, BuildError>;

pub struct BuildOptions {
    pub project_root: PathBuf,
    pub target: TargetTriple, // ADR-0034 triple
    pub output_path: Option<PathBuf>, // defaults to <project_root>/<name>-<version>.tau
}

pub struct BundleArtifact {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}
```

Pipeline (7 steps):

1. **Load `tau.toml`** — reuse `tau_pkg::config::load_project_toml`. Failure → `BuildError::ProjectConfig`.
2. **Load `tau.lock`** — reuse `tau_pkg::lockfile::LockFile::load`. Missing → `BuildError::MissingLockfile`. Schema version mismatch propagates.
3. **Verify install** — for each `LockedPlugin` + `LockedSkill`, check the package directory exists at the expected scope path. Missing any → `BuildError::PackageNotInstalled { name, path }`.
4. **Gather package facts** — for each locked package, compute `tree_sha256` via `tau_pkg::tree_hash::tree_hash` (priority 7). Pull `binary_sha256` from the lockfile if present. Pull `required_shapes` from the lockfile. Produce a `Vec<BundlePackage>` sorted by name.
5. **Gather agent facts** — for each `[[agents.<id>]]` in tau.toml:
   - Resolve the system prompt to bytes (inline string OR file path → read file). Hash → `system_prompt_sha256`.
   - List required tools (already typed as `[[agents.<id>.requires.tools]]` in tau.toml).
   - Compute effective capabilities via `compute_effective` (priority 4). See §4.1 for the layering.
   - Produce `Vec<BundleAgent>` sorted by id.
6. **Assemble** — populate `BundleManifest` with `schema_version=1`, gathered data, `target = opts.target`, `bundle.sha256 = ""` (placeholder), `created_at = Utc::now()` (excluded from hash per §2).
7. **Write** — canonical-TOML emit → compute self-hash → fill `bundle.sha256` → re-emit canonical TOML → atomic `std::fs::write` (bundle is ~few KB; no streaming needed). Return `BundleArtifact`.

### 4.1 `compute_effective` layering — known plan-time decision

`compute_effective` and its `CapabilityOverrideError` currently live in `tau-runtime::capability_override`. Adding `tau-runtime` as a `tau-pkg` dep would invert the existing layering (tau-runtime → tau-pkg today). Resolution path at plan time, in order of preference:

- **(a)** Lift `compute_effective` to `tau-domain`. It's pure logic over `Capability` (which is already in `tau-domain`), so no behavioral change.
- **(b)** Lift to `tau-ports`. Less natural (it's not a port).
- **(c)** Add `tau-pkg::capability_override` as a re-export shim that takes the inputs `compute_effective` needs without pulling tau-runtime. More indirection.

(a) is the right answer; the planner confirms there's no hidden dep before writing the task.

### 4.2 Determinism guarantees

- Sorted iteration in steps 4 and 5 (by name / by id).
- `tree_hash` is content-addressed (already deterministic per priority 7).
- `bundle.created_at` excluded from `bundle.sha256` per §3.1.
- Canonical TOML emitter is byte-stable per §C.1.

These four properties together mean: two builds of identical input trees on different machines, at different times, produce identical `bundle.sha256` strings. §E (cross-machine reproducibility verify) extends this with a verify command.

## 5. CLI

```
$ tau build
```

No flags in MVP.

**Stderr (human):**

```
Gathering 5 packages…
Hashing trees…
Resolving agents (3)…
Writing bundle: my-project-0.1.0.tau (sha256: a3b2…f1d4, 2.4 KB)
```

**Stdout:** the absolute path to the written bundle. This lets `tau run --bundle "$(tau build)"` work once §C.3 lands.

**Exit codes:**

| Code | Meaning |
|---|---|
| 0 | Success |
| 2 | Bad config / parse error |
| 3 | Install state incomplete (missing lockfile, uninstalled package) |
| 70 | Internal / IO error |

Mirrors `tau check`.

## 6. Error types

```rust
// crates/tau-pkg/src/bundle/build_error.rs
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("failed to load project tau.toml: {0}")]
    ProjectConfig(#[from] tau_pkg::config::ProjectConfigError),

    #[error("missing lockfile; run `tau install` first")]
    MissingLockfile,

    #[error("failed to load lockfile: {0}")]
    LockfileLoad(#[from] tau_pkg::lockfile::LockfileError),

    #[error("package `{name}` is locked but not installed at {path:?}; run `tau install`")]
    PackageNotInstalled { name: String, path: PathBuf },

    #[error("tree hash failed for `{name}`: {source}")]
    TreeHashFailed { name: String, #[source] source: tau_pkg::tree_hash::Error },

    #[error("system prompt resolution failed for agent `{id}`: {source}")]
    PromptResolveFailed { id: String, #[source] source: std::io::Error },

    #[error("capability override compute failed for agent `{id}`: {source}")]
    CapabilityOverrideFailed { id: String, #[source] source: /* CapabilityOverrideError, post §4.1 lift */ },

    #[error("manifest assembly failed: {0}")]
    ManifestInvalid(String),

    #[error("bundle write failed at {path:?}: {source}")]
    WriteFailed { path: PathBuf, #[source] source: std::io::Error },
}
```

Exit-code mapping in `tau-cli::cmd::build`:

- `MissingLockfile`, `PackageNotInstalled` → 3
- `ProjectConfig`, `LockfileLoad`, `ManifestInvalid` → 2 (config/parse errors)
- `TreeHashFailed`, `PromptResolveFailed`, `CapabilityOverrideFailed`, `WriteFailed` → 70 (internal/IO)

## 7. Test plan

**Unit tests in `tau-pkg::bundle::build`:**

- `build_strict_rejects_missing_lockfile`
- `build_strict_rejects_missing_install`
- `build_emits_sorted_packages`
- `build_emits_sorted_agents`
- `build_excludes_created_at_from_self_hash`
- `build_self_hash_round_trips_through_verify`
- `build_writes_to_expected_default_path`
- `build_overwrites_existing_bundle`

**Unit test in `tau-pkg::bundle::hash` (amendment):**

- `compute_self_hash_zeros_created_at`

**Integration test in `tau-pkg/tests/bundle_build_e2e.rs`:**

- Realistic fixture: tmpdir with a 2-package + 2-agent `tau.toml` + lockfile + installed package trees. Build → assert file exists, parses, self-hash verifies, all expected packages + agents with correct hashes.

**CLI integration tests in `tau-cli/tests/cmd_build.rs`:**

- `tau build` on a clean fixture → stdout is the absolute bundle path, stderr matches the "Gathering N packages…" shape, exit 0
- `tau build` with no lockfile → exit 3, stderr contains "run `tau install` first"
- `tau build` with locked-but-not-installed package → exit 3, stderr names the missing package

## 8. Out of scope

See §1. All deferred items are clean follow-ups that don't require revisiting this design.

## 9. References

- ADR-0035 — bundle format (§C.1)
- ADR-0034 — target triple registry
- Priority 4 — capability override + `compute_effective`
- Priority 7 — `tree_hash`
- Phase 2 §E — cross-machine reproducibility verify (consumer of the stable-hash property in §2)
