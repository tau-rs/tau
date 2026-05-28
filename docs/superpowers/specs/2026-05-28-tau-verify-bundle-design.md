# `tau verify --bundle` — Phase 2 §E design

**Status:** Accepted
**Date:** 2026-05-28
**Authors:** titouanlebocq
**Depends on:** §C.1 bundle format (ADR-0035, merged), §C.2 `tau build` producer (PR #242, merged)
**Independent of:** §C.3 `tau run --bundle` (PR #247) — §E uses `build` + `BundleManifest` + `compute_self_hash`, none from §C.3

## 1. Goal

Ship cross-machine **reproducibility verification**: `tau verify --bundle <path>` rebuilds a fresh bundle from the local source tree and compares its self-hash to the shipped bundle's. Match ⇒ the local source reproduces this exact bundle. On mismatch, report a field-level diff of what diverged.

This is the strong reproducibility guarantee: if a bundle built on machine A verifies on machine B, then B's source + installed packages are byte-identical (by content hash) to A's. Distinct from §C.3's `tau run --bundle` gate (which checks the tree against the bundle's *recorded* hashes); §E rebuilds and catches drift the recorded hashes alone can't (e.g. a build-logic change that produces different output from the same inputs).

Non-goals (deferred):
- Comparing two bundle files directly (`--against b.tau`, no local tree)
- `--allow`/`--ignore-tau-version` lists for expected divergences
- Remote fetch of the reference bundle (both bundle + source assumed local)

## 2. Headline decisions

- **Rebuild-and-compare**, not a re-expose of `verify_bundle`. `tau verify --bundle foo.tau` calls §C.2's `build` against the local tree (using the *shipped bundle's* target triple), then compares self-hashes.
- **Field-level diff on mismatch** — report exactly which package / agent / project field diverged, not just a binary verdict.
- **`tau_version` stays in the hash domain (option a).** Reproducibility is scoped to same-source-AND-same-tau-version. A tau upgrade alone reports not-reproducible, with the field-diff clearly showing the `tau_version` skew. No change to §C.2's hash module.
- **`--bundle` flag on existing `tau verify`** — switches to reproducibility mode (mutually exclusive with the package-positional path). Reuses `--json`. Exit codes 0/2/3/70.
- **Logic in `tau_pkg::bundle::reproduce`** next to `build`/`verify`; thin renderer in `tau-cli::cmd::verify`.

## 3. Module structure + core API

```
crates/tau-pkg/src/bundle/
├── ... build, build_error, verify, verify_error, manifest, canonical, hash [exist]
├── reproduce.rs        (§E — rebuild-and-compare + diff_manifests)   [new]
└── reproduce_error.rs  (§E — ReproError enum)                        [new]

crates/tau-cli/src/cmd/verify.rs   (add --bundle branch + renderers)  [modify]
crates/tau-cli/src/cli.rs          (add --bundle to VerifyArgs)       [modify]
```

```rust
pub fn verify_reproducible(opts: ReproOptions) -> Result<ReproReport, ReproError>;

pub struct ReproOptions {
    pub bundle_path: PathBuf,
    pub project_root: PathBuf,   // local source tree (cwd)
}

pub struct ReproReport {
    pub reproducible: bool,
    pub shipped_sha256: String,
    pub rebuilt_sha256: String,
    /// Empty when reproducible; otherwise the field-level divergences.
    pub diffs: Vec<ManifestDiff>,
}

pub enum ManifestDiff {
    ProjectField { field: String, shipped: String, rebuilt: String },
    PackageMissing { name: String, side: Side },
    PackageField { name: String, field: String, shipped: String, rebuilt: String },
    AgentMissing { id: String, side: Side },
    AgentField { id: String, field: String, shipped: String, rebuilt: String },
    BundleMetaField { field: String, shipped: String, rebuilt: String }, // e.g. tau_version, target
    SchemaVersionMismatch { shipped: u32, rebuilt: u32 },
}

pub enum Side { ShippedOnly, RebuiltOnly }
```

`ReproError` is for "couldn't produce a comparison." A successful *non-reproducible* result is `Ok(ReproReport { reproducible: false, diffs })`, not an error.

## 4. Reproduce pipeline

```rust
pub fn verify_reproducible(opts: ReproOptions) -> Result<ReproReport, ReproError> {
    // 1. Read + parse the shipped bundle.
    let shipped_str = std::fs::read_to_string(&opts.bundle_path)
        .map_err(|e| ReproError::BundleRead { path: opts.bundle_path.clone(), source: e })?;
    let shipped = BundleManifest::parse_str(&shipped_str)
        .map_err(|e| ReproError::BundleParse { source: e })?;

    // 2. The shipped bundle must itself be valid before we compare to it.
    crate::bundle::hash::verify_self_hash(&shipped)
        .map_err(|e| ReproError::ShippedSelfHashInvalid { detail: e.to_string() })?;

    // 3. Rebuild from the local tree using the SHIPPED bundle's target
    //    (apples-to-apples), to a temp path so we never clobber anything.
    let tmp = tempfile::TempDir::new().map_err(|e| ReproError::TempDir { source: e })?;
    let rebuilt_path = tmp.path().join("rebuilt.tau");
    let artifact = crate::bundle::build(crate::bundle::BuildOptions {
        project_root: opts.project_root.clone(),
        target: shipped.bundle.target,
        output_path: Some(rebuilt_path.clone()),
    }).map_err(|e| ReproError::Rebuild { source: e })?;

    // 4. Parse the rebuilt bundle.
    let rebuilt_str = std::fs::read_to_string(&artifact.path)
        .map_err(|e| ReproError::RebuiltRead { path: artifact.path.clone(), source: e })?;
    let rebuilt = BundleManifest::parse_str(&rebuilt_str)
        .map_err(|e| ReproError::RebuiltParse { source: e })?;

    // 5. Verdict by self-hash; diff for detail.
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

### 4.1 `diff_manifests(shipped, rebuilt) -> Vec<ManifestDiff>`

Compares everything in the **hash domain**, excluding only `bundle.sha256` (the comparison key) and `bundle.created_at` (not hashed per §C.2):

- `schema_version` → `SchemaVersionMismatch` (defensive; should match).
- `bundle.target`, `bundle.tau_version` → `BundleMetaField` (target should match since we rebuilt with it; tau_version legitimately diffs across tool versions — option a).
- `project.name`, `project.version`, `project.tau_toml_sha256` → `ProjectField`.
- `packages` — index both by name. Name in one side only → `PackageMissing { side }`. Otherwise per-field compare `tree_sha256`, `source`, `binary_sha256`, `required_shapes` → `PackageField`.
- `agents` — index both by id. Id in one side only → `AgentMissing { side }`. Otherwise per-field compare `system_prompt_sha256`, `backend`, `required_tools`, `effective_capabilities` → `AgentField`.

Deterministic ordering: emit diffs in a stable order (schema, bundle-meta, project, packages sorted by name, agents sorted by id) so output + JSON are reproducible across runs.

## 5. CLI surface + output

```
tau verify --bundle <path>
tau verify --bundle <path> --json
```

`--bundle <path>` on `VerifyArgs`. When set, switches to reproducibility mode — mutually exclusive with the package positional (clap `conflicts_with`, or explicit exit-2 if both given). `--global` / `--version` / `--anthropic-strict` are package-verify options, ignored in bundle mode (documented).

**Human — reproducible:**
```
$ tau verify --bundle my-project-0.1.0.tau
Rebuilding from local tree (target: linux-native-strict)…
✓ Reproducible — rebuilt bundle matches my-project-0.1.0.tau (sha256: a3b2…f1d4)
```

**Human — NOT reproducible:**
```
$ tau verify --bundle my-project-0.1.0.tau
Rebuilding from local tree (target: linux-native-strict)…
✗ NOT reproducible
  shipped: a3b2…f1d4
  rebuilt: 9c8e…20ab
  divergences:
    - package `fs-read` tree_sha256: 1111… → 2222…
    - agent `writer` system_prompt_sha256: aaaa… → bbbb…
    - tau_version: 0.1.0 → 0.2.0
```

**JSON (`--json`)** — one object mirroring `ReproReport`:
```json
{
  "reproducible": false,
  "shipped_sha256": "a3b2...",
  "rebuilt_sha256": "9c8e...",
  "diffs": [
    {"kind": "package_field", "name": "fs-read", "field": "tree_sha256", "shipped": "1111...", "rebuilt": "2222..."},
    {"kind": "agent_field", "id": "writer", "field": "system_prompt_sha256", "shipped": "aaaa...", "rebuilt": "bbbb..."},
    {"kind": "bundle_meta_field", "field": "tau_version", "shipped": "0.1.0", "rebuilt": "0.2.0"}
  ]
}
```

Renderers (`render_repro_human` + `render_repro_json`) live in `tau-cli::cmd::verify`, parallel to the existing package-verify renderers. Hash abbreviation is display-only; JSON carries full hashes.

### 5.1 Exit codes

| Situation | Code |
|---|---|
| `Ok`, reproducible | 0 |
| `Ok`, not reproducible | 2 |
| `Err(Rebuild(MissingLockfile / PackageNotInstalled))` — can't verify, not installed | 3 |
| `Err(BundleRead / BundleParse / ShippedSelfHashInvalid)` — bad input | 2 |
| `Err(TempDir / RebuiltRead / RebuiltParse / other Rebuild)` — internal | 70 |

## 6. Error types

```rust
// crates/tau-pkg/src/bundle/reproduce_error.rs
#[derive(Debug, thiserror::Error)]
pub enum ReproError {
    #[error("failed to read bundle at {path:?}: {source}")]
    BundleRead { path: PathBuf, #[source] source: std::io::Error },

    #[error("bundle parse failed: {source}")]
    BundleParse { #[source] source: crate::bundle::error::BundleParseError },

    #[error("shipped bundle self-hash is invalid ({detail}); it is corrupt — cannot use it as a reproducibility reference")]
    ShippedSelfHashInvalid { detail: String },

    #[error("could not create temp dir for rebuild: {source}")]
    TempDir { #[source] source: std::io::Error },

    #[error("rebuild failed: {source}")]
    Rebuild { #[source] source: crate::bundle::build_error::BuildError },

    #[error("failed to read rebuilt bundle at {path:?}: {source}")]
    RebuiltRead { path: PathBuf, #[source] source: std::io::Error },

    #[error("rebuilt bundle parse failed: {source}")]
    RebuiltParse { #[source] source: crate::bundle::error::BundleParseError },
}
```

## 7. Test plan

**Unit tests in `tau_pkg::bundle::reproduce`:**
- `reproducible_when_tree_unchanged` (load-bearing — clean rebuild is bit-stable)
- `not_reproducible_when_package_tree_changes` → `PackageField { field: "tree_sha256" }`
- `not_reproducible_when_prompt_file_changes` → `AgentField { field: "system_prompt_sha256" }`
- `not_reproducible_when_tau_toml_changes` → at least a `ProjectField { field: "tau_toml_sha256" }`
- `repro_error_when_bundle_missing` → `ReproError::BundleRead`
- `repro_error_when_shipped_bundle_corrupt` → `ReproError::ShippedSelfHashInvalid`
- `repro_error_when_not_installed` → `ReproError::Rebuild(PackageNotInstalled | MissingLockfile)`

**Unit tests for `diff_manifests` (synthesized manifests, no I/O):**
- `diff_detects_added_package` → `PackageMissing { side: RebuiltOnly }`
- `diff_detects_removed_agent` → `AgentMissing { side: ShippedOnly }`
- `diff_ignores_sha256_and_created_at` → empty diff when only those two fields differ
- `diff_reports_tau_version_skew` → one `BundleMetaField` for tau_version

**Integration `crates/tau-pkg/tests/bundle_reproduce_e2e.rs`:**
- `e2e_clean_rebuild_is_reproducible` — §C.2 fixture (2 pkg + 2 agents) → reproducible
- `e2e_mutated_package_breaks_reproducibility` — mutate a package file → not reproducible + right `PackageField`

**CLI `crates/tau-cli/tests/cmd_verify_bundle.rs`:**
- `verify_bundle_reproducible_exits_zero` — build via `tau build`, verify unchanged tree → exit 0, "Reproducible"
- `verify_bundle_drift_exits_two` — mutate a package file → exit 2, "NOT reproducible" + package name
- `verify_bundle_not_installed_exits_three` — delete package dir → exit 3
- `verify_bundle_json_emits_structured_result` — `--json` → parse stdout, assert `reproducible` + `diffs` shape

## 8. Out of scope

See §1. Deferred items are clean follow-ups not requiring this design to change.

## 9. References

- ADR-0035 — bundle format (§C.1)
- §C.2 spec — `2026-05-27-tau-build-design.md` (the producer §E rebuilds with)
- §C.3 spec — `2026-05-27-tau-run-bundle-design.md` (sibling consumer; §E is independent of it)
- ADR-0034 — target triple registry
