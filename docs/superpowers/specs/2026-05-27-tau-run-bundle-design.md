# `tau run --bundle` — Phase 2 §C.3 design

**Status:** Draft
**Date:** 2026-05-27
**Authors:** titouanlebocq
**Depends on:** ADR-0035 (bundle format §C.1, merged 2026-05-19), §C.2 (`tau build` MVP producer, PR #242)
**Successor:** Phase 2 §E (cross-machine reproducibility verify) — uses `verify_bundle` but against a separate source tree

## 1. Goal

Ship the **MVP `tau run --bundle <path> <agent-id>` consumer**: a strict-by-default integrity verifier that confirms a `.tau` bundle matches its source tree, then dispatches the named agent through the existing Runtime/Agent machinery.

Non-goals (deferred):

- `tau workflow run --bundle` — deferred follow-up
- `tau chat --bundle` — deferred
- `--allow-drift` / `--force` escape hatches (no v1 use case)
- Cross-machine reproducibility verify — §E (reuses `verify_bundle` against a different `project_root`)
- Cross-target sandbox plan resolution (bundle target ≠ host) — refused per §2
- Self-contained bundles (embedded plugin binaries) — indefinitely deferred per ADR-0035

## 2. Headline decisions

- **Recipe model.** Bundle is a content-addressed lock + integrity envelope. `tau run --bundle <foo.tau>` requires the source tree that produced it to be present at cwd; runtime reads tau.toml + prompt files from disk, verifies sha256s match the bundle, then runs. Aligns with ADR-0035's reference-only stance.
- **Refuse on drift.** Any mismatch (tau.toml sha256, prompt sha256, package tree_sha256, target triple, schema_version) is a hard exit with a remediation hint. No `--force` escape hatches in v1.
- **Strict-everything verification.** Bundle target MUST equal host triple. Every locked package MUST be installed at the exact `tree_sha256` recorded in the bundle. Mirrors `tau build`'s strict-install posture.
- **CLI surface: `--bundle <path>` flag on existing `tau run`.** Reuses the existing positional `<agent-id>` arg + all other `tau run` flags. No new top-level verb.
- **Verifier lives in `tau-pkg::bundle::verify`** next to `build`. Thin CLI shim in `tau-cli::cmd::run` adds the `--bundle` branch and exit-code mapping. `tau-runtime` stays bundle-agnostic.

## 3. Module structure

```
crates/tau-pkg/src/bundle/
├── mod.rs                  (re-exports + public surface)
├── manifest.rs             (§C.1)                                  [exists]
├── canonical.rs            (§C.1)                                  [exists]
├── hash.rs                 (§C.1 + §C.2 amendment)                 [exists]
├── build.rs                (§C.2 — producer)                       [exists]
├── build_error.rs          (§C.2)                                  [exists]
├── verify.rs               (§C.3 — bundle-vs-source verification)  [new]
└── verify_error.rs         (§C.3 — VerifyError enum)               [new]

crates/tau-cli/src/cmd/
├── run.rs                  (extend with --bundle flag)             [modify]

crates/tau-cli/src/cli.rs   (add `--bundle` to existing RunArgs)    [modify]
```

## 4. Verify API

```rust
pub fn verify_bundle(opts: VerifyOptions) -> Result<VerifyReport, VerifyError>;

pub struct VerifyOptions {
    /// Path to the `.tau` bundle file on disk.
    pub bundle_path: PathBuf,
    /// Project source tree to verify the bundle against. Typically
    /// the cwd. §E will pass a different path for cross-machine repro.
    pub project_root: PathBuf,
}

pub struct VerifyReport {
    /// The parsed manifest with self-hash already verified.
    pub manifest: BundleManifest,
    /// Per-agent context the CLI needs to dispatch without re-reading
    /// from disk. Keyed by agent id.
    pub agent_lookup: BTreeMap<String, ResolvedAgent>,
}

pub struct ResolvedAgent {
    /// The bundle's record for this agent (target shapes etc.).
    pub bundle_entry: BundleAgent,
    /// The verified-clean system-prompt bytes (re-hashed against the
    /// bundle as part of step 8).
    pub system_prompt: Vec<u8>,
}
```

The `VerifyReport` carries everything `tau-cli::cmd::run` needs to dispatch into the existing Runtime without re-reading anything from disk.

## 5. Verify pipeline (8 steps)

```rust
pub fn verify_bundle(opts: VerifyOptions) -> Result<VerifyReport, VerifyError> {
    let bundle_str = read_bundle(&opts.bundle_path)?;                    // 1
    let manifest   = parse_bundle(&bundle_str)?;                         // 2
    verify_self_hash(&manifest)?;                                        // 3
    verify_schema_version(&manifest)?;                                   // 4
    verify_target_matches_host(&manifest)?;                              // 5
    verify_tau_toml_sha256(&manifest, &opts.project_root)?;              // 6
    verify_packages_installed_and_hashed(&manifest, &opts.project_root)?;// 7
    let agent_lookup = verify_agent_prompts(&manifest, &opts.project_root)?; // 8
    Ok(VerifyReport { manifest, agent_lookup })
}
```

### 5.1 Per-step

1. **Read bundle file** — `std::fs::read_to_string(bundle_path)`. Failure → `VerifyError::BundleRead { path, source }`.
2. **Parse** — `BundleManifest::parse_str` (existing §C.1). Failure → `VerifyError::BundleParse { source }`.
3. **Self-hash** — `bundle::hash::verify_self_hash` (existing). Failure → `VerifyError::SelfHashMismatch { claimed, computed }`.
4. **Schema version** — `manifest.schema_version == 1` (v1 only). Mismatch → `VerifyError::UnsupportedSchemaVersion { found, supported: 1 }`.
5. **Target ↔ host** — `manifest.bundle.target == TargetTriple::host()`. Mismatch → `VerifyError::TargetMismatch { bundle, host }`.
6. **`tau_toml_sha256`** — read `<project_root>/tau.toml`, sha256 it, compare to `manifest.project.tau_toml_sha256`. Mismatch → `VerifyError::TauTomlDrift { claimed, computed }`. Read failure → `VerifyError::ProjectTomlRead { path, source }`.
7. **Packages installed + hashed** — for each `BundlePackage`:
   - Check `<root>/.tau/packages/<name>/<version>/` exists. Missing → `VerifyError::PackageMissing { name, expected_path }`.
   - Compute `tree_hash` on the install dir. Compare to `package.tree_sha256`. Mismatch → `VerifyError::PackageDrift { name, claimed, computed }`. Hash failure → `VerifyError::PackageTreeHash { name, source }`.
8. **Agent prompts** — re-parse `<project_root>/tau.toml` once (the bytes are already verified clean in step 6, so the parse must succeed in practice — propagate any unexpected failure as `VerifyError::ProjectTomlRead`). Then iterate `manifest.agents` (the bundle's record):
   - Look up the agent's id in the parsed tau.toml's `[agents.*]` table. Not found → `VerifyError::AgentSetMismatch { id }`. (Note: step 6's `tau_toml_sha256` already catches the symmetric case where tau.toml has extra agents not in the bundle, because adding an agent changes the file's bytes.)
   - Resolve the prompt: inline `system` string → bytes; or `system_file` path → file read relative to `project_root`. Read failure → `VerifyError::AgentPromptResolve { id, source }`.
   - Compute sha256, compare to `agent.system_prompt_sha256`. Mismatch → `VerifyError::AgentPromptDrift { id, claimed, computed }`.
   - Build the `agent_lookup` entry with the verified prompt bytes + the bundle's `BundleAgent` record.

### 5.2 Note on step 6 vs step 8 overlap

Step 6's `tau_toml_sha256` check is the umbrella for "the source-of-truth tau.toml hasn't changed." If it passes, all `[agents.*]` blocks within tau.toml are by definition unchanged — but the prompt FILES referenced from tau.toml may have changed independently, which is why step 8 still re-hashes each prompt's content. (A `system = "inline string"` prompt is fully covered by step 6's tau.toml hash and step 8's re-hash is technically redundant for those — kept for symmetry and so the same code path handles both inline + file-based prompts uniformly.)

## 6. CLI surface

```
tau run --bundle <bundle-path> <agent-id> [other existing tau run flags]
```

- `--bundle <path>` — new optional flag on the existing `tau run` `RunArgs`. Path is absolute or relative to cwd.
- `<agent-id>` — same positional arg as today. Must match one of `bundle.agents[*].id`.
- All other existing `tau run` flags continue to work unchanged.

### 6.1 Behavior when `--bundle` is set

1. Resolve `bundle_path` (absolute, or relative to cwd).
2. Resolve `project_root` = cwd (same as default `tau run`).
3. Call `verify_bundle(VerifyOptions { bundle_path, project_root })`.
4. Look up `<agent-id>` in `report.agent_lookup`. Not found → exit 2 with `agent \`X\` not in bundle`.
5. Dispatch into existing Runtime/Agent spawn machinery, using the verified manifest + resolved prompt bytes as the source of truth (instead of re-parsing tau.toml directly).
6. All existing tracing / REPL / streaming machinery works unchanged because the agent definition is the same shape, just sourced from a verified envelope.

### 6.2 Exit codes

| Code | Meaning |
|---|---|
| 0 | Run succeeded |
| 2 | Bad config / parse / agent-not-in-bundle |
| 3 | Bundle integrity error (self-hash, target, drift, install state) |
| 70 | Internal / IO error |
| Other | Inherited from existing `tau run` (e.g. agent failure exit codes) |

### 6.3 Output

Stdout/stderr unchanged from existing `tau run`. The `--bundle` flag is a sourcing decision; runtime output is the same.

## 7. Error types

```rust
// crates/tau-pkg/src/bundle/verify_error.rs
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("failed to read bundle at {path:?}: {source}")]
    BundleRead { path: PathBuf, #[source] source: std::io::Error },

    #[error("bundle parse failed: {source}")]
    BundleParse {
        #[source]
        source: crate::bundle::manifest::BundleParseError,
    },

    #[error("bundle self-hash mismatch — claimed {claimed}, computed {computed}; the bundle was tampered with or corrupted")]
    SelfHashMismatch { claimed: String, computed: String },

    #[error("unsupported bundle schema_version {found}; this tau supports {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error("bundle target {bundle} does not match host {host}; rebuild for this host or run on a matching machine")]
    TargetMismatch { bundle: TargetTriple, host: TargetTriple },

    #[error("tau.toml drift — claimed sha256 {claimed} but cwd has {computed}; rebuild the bundle or check out the source at the recorded version")]
    TauTomlDrift { claimed: String, computed: String },

    #[error("project tau.toml missing or unreadable at {path:?}: {source}")]
    ProjectTomlRead { path: PathBuf, #[source] source: std::io::Error },

    #[error("locked package `{name}` missing from {expected_path:?}; run `tau install` in this project")]
    PackageMissing { name: String, expected_path: PathBuf },

    #[error("package `{name}` tree drift — claimed {claimed}, computed {computed}; reinstall or rebuild bundle")]
    PackageDrift { name: String, claimed: String, computed: String },

    #[error("package `{name}` tree-hash failed: {source}")]
    PackageTreeHash {
        name: String,
        #[source]
        source: crate::tree_hash::TreeHashError,
    },

    #[error("agent `{id}` system prompt drift — claimed {claimed}, computed {computed}")]
    AgentPromptDrift { id: String, claimed: String, computed: String },

    #[error("agent `{id}` prompt resolve failed: {source}")]
    AgentPromptResolve { id: String, #[source] source: std::io::Error },

    #[error("agent `{id}` named in tau.toml but missing from bundle (or vice versa); rebuild bundle")]
    AgentSetMismatch { id: String },
}
```

### 7.1 Exit-code mapping in `tau-cli::cmd::run`

- `BundleRead`, `BundleParse`, `ProjectTomlRead`, `UnsupportedSchemaVersion` → 2 (bad input/config)
- `SelfHashMismatch`, `TargetMismatch`, `TauTomlDrift`, `PackageMissing`, `PackageDrift`, `AgentPromptDrift`, `AgentSetMismatch` → 3 (integrity / install-state)
- `PackageTreeHash`, `AgentPromptResolve` → 70 (internal/IO)

## 8. Test plan

### 8.1 Unit tests in `tau_pkg::bundle::verify`

Each test builds a fixture project + bundle, then calls `verify_bundle` and asserts the exact `VerifyError` variant.

- `verify_succeeds_on_clean_built_bundle` — happy path
- `verify_rejects_missing_bundle_file` → `BundleRead`
- `verify_rejects_malformed_bundle_toml` → `BundleParse`
- `verify_rejects_self_hash_tampered_bundle` — mutate one byte after build → `SelfHashMismatch`
- `verify_rejects_unsupported_schema_version` — synthesize `schema_version = 2` → `UnsupportedSchemaVersion`
- `verify_rejects_target_triple_mismatch` — build with `target = PASSTHROUGH` → `TargetMismatch`
- `verify_rejects_tau_toml_drift` → `TauTomlDrift`
- `verify_rejects_missing_package_dir` → `PackageMissing`
- `verify_rejects_package_tree_drift` → `PackageDrift`
- `verify_rejects_agent_prompt_drift` (file-based prompt) → `AgentPromptDrift`
- `verify_succeeds_on_inline_prompts_after_tau_toml_unchanged` — sanity for the step-6/step-8 overlap

### 8.2 Integration test in `crates/tau-pkg/tests/bundle_verify_e2e.rs`

- `e2e_verify_roundtrip_on_realistic_fixture` — reuses §C.2's e2e fixture (2 packages + 2 agents, one inline + one file-based prompt). Build → verify → assert Ok + correct `agent_lookup` keys + correct prompt bytes.

### 8.3 CLI integration tests in `crates/tau-cli/tests/cmd_run_bundle.rs`

- `run_bundle_on_clean_fixture_succeeds_and_drives_runtime` — full path with echo-llm fixture. Exit 0; assert expected output.
- `run_bundle_with_drift_exits_three_with_diagnostic` — mutate tau.toml; exit 3; stderr contains "tau.toml drift".
- `run_bundle_with_missing_install_exits_three` — delete `.tau/packages/<x>/`; exit 3; stderr contains "missing from".
- `run_bundle_with_foreign_target_exits_three` — build with `target = passthrough`; exit 3; stderr contains "target".
- `run_bundle_with_nonexistent_agent_exits_two` — agent ID not in `bundle.agents[*]`; exit 2; stderr names the agent.

## 9. Out of scope

See §1. All deferred items are clean follow-ups that don't require revisiting this design.

## 10. References

- ADR-0035 — bundle format (§C.1)
- §C.2 spec — `2026-05-27-tau-build-design.md`, ships the producer this design consumes
- ADR-0034 — target triple registry
- Phase 2 §E — cross-machine reproducibility verify (consumer of `verify_bundle` against a different `project_root`)
