# `tau build --agent <id>` per-agent slicing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `tau build --agent <id>` (repeatable) so a multi-agent project can emit a bundle scoped to one or more agents, pruning packages those agents don't reference, while staying reproducible under `tau verify --bundle`.

**Architecture:** Slicing lives in `tau_pkg::bundle::build`, gated by a new `BuildOptions.agent_filter: Option<Vec<AgentId>>`. A sliced build keeps only the named agents and prunes `packages[]` to their direct reference closure (`{agent home package} ∪ {required_tools}`). The slice is recorded in a new optional `[bundle].selected_agents` field so `verify_reproducible` can replay it. `None` filter = today's full-build behavior, byte-identical.

**Tech Stack:** Rust, `serde`/`toml`, `clap` (CLI), `thiserror`, `insta` (snapshots), `assert_cmd`/`predicates` (CLI integration tests). Workspace cargo rules in `CLAUDE.md` apply — every cargo command is `timeout <n> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`.

**Spec:** `docs/superpowers/specs/2026-05-28-tau-build-agent-slice-design.md`

**Crates touched:** `tau-pkg` (build, manifest, reproduce, project/agent), `tau-cli` (cli, cmd/build, tests).

---

## Cargo command reference (per `CLAUDE.md`)

Use a distinct `CARGO_TARGET_DIR` per agent role. Examples below assume role `impl`; substitute your own:

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
```

Prefer `cargo nextest run -p <crate>` for tests where available; `cargo test --doc -p <crate>` for doctests.

---

## File Structure

- `crates/tau-pkg/src/project/agent.rs` — bump `parse_package_ref` from private `fn` to `pub(crate) fn` (Task 1). Module is already `pub mod agent;`.
- `crates/tau-pkg/src/bundle/manifest.rs` — add `selected_agents: Option<Vec<String>>` to `BundleMeta` (Task 2).
- `crates/tau-pkg/src/bundle/build_error.rs` — add `UnknownAgent` variant (Task 3).
- `crates/tau-pkg/src/bundle/build.rs` — add `agent_filter` to `BuildOptions`; update internal `BundleMeta`/`BuildOptions` literals; add slicing logic + unit tests (Tasks 2, 3, 4, 5).
- `crates/tau-pkg/src/bundle/verify.rs`, `crates/tau-pkg/src/bundle/reproduce.rs`, `crates/tau-pkg/tests/bundle_*_e2e.rs` — add `agent_filter: None` to existing `BuildOptions` literals (Task 4).
- `crates/tau-pkg/src/bundle/reproduce.rs` — derive `agent_filter` from the shipped bundle's `selected_agents`; add reproduce tests (Task 6).
- `crates/tau-cli/src/cli.rs` — add `agents: Vec<String>` to `BuildArgs` (Task 7).
- `crates/tau-cli/src/cmd/build.rs` — map `agents` → `agent_filter`; map `UnknownAgent` → exit 2; unit test (Task 7).
- `crates/tau-cli/tests/cmd_build.rs` + `crates/tau-cli/tests/snapshots/help_snapshots__build_help.snap` — CLI integration tests + help snapshot regen (Task 8).

---

## Task 1: Make `parse_package_ref` crate-visible

**Files:**
- Modify: `crates/tau-pkg/src/project/agent.rs:286`

No behavior change — `bundle/build.rs` (Task 5) needs to call this to extract a package name from an agent's `package = "<name>@<req>"` field.

- [ ] **Step 1: Bump visibility**

In `crates/tau-pkg/src/project/agent.rs`, change line 286 from:

```rust
fn parse_package_ref(package: &str) -> Result<(String, semver::VersionReq), String> {
```

to:

```rust
pub(crate) fn parse_package_ref(package: &str) -> Result<(String, semver::VersionReq), String> {
```

- [ ] **Step 2: Verify it compiles**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg`
Expected: compiles clean (a `pub(crate)` fn with existing in-module callers — no dead-code warning since `agent.rs` tests already call it).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/src/project/agent.rs
git commit -m "refactor(tau-pkg): make parse_package_ref pub(crate) for bundle slicing"
```

---

## Task 2: Add `selected_agents` to `BundleMeta`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/manifest.rs` (`BundleMeta` struct ~line 31; `sample_manifest` helper ~line 327; add tests in the `tests` module)
- Modify: `crates/tau-pkg/src/bundle/build.rs` (the `BundleMeta { .. }` literal in step 6, ~line 232)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tau-pkg/src/bundle/manifest.rs` (after `effective_capabilities_omitted_when_empty`):

```rust
    #[test]
    fn selected_agents_omitted_when_none() {
        let mut m = sample_manifest();
        m.bundle.selected_agents = None;
        let toml_str = toml::to_string(&m).expect("serialize");
        assert!(
            !toml_str.contains("selected_agents"),
            "selected_agents should be omitted when None: {toml_str}"
        );
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed.bundle.selected_agents, None);
    }

    #[test]
    fn selected_agents_round_trips_when_some() {
        let mut m = sample_manifest();
        m.bundle.selected_agents = Some(vec!["alpha".to_string(), "beta".to_string()]);
        let toml_str = toml::to_string(&m).expect("serialize");
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(
            parsed.bundle.selected_agents,
            Some(vec!["alpha".to_string(), "beta".to_string()])
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::manifest::tests::selected_agents`
Expected: FAIL to compile — `BundleMeta` has no field `selected_agents`.

- [ ] **Step 3: Add the field to `BundleMeta`**

In `crates/tau-pkg/src/bundle/manifest.rs`, add to the `BundleMeta` struct (after the `target` field, before the closing `}` at ~line 44):

```rust
    /// Agent ids this bundle was sliced to (sorted), or absent for a
    /// full build. Drives `tau verify --bundle` reproduction: a sliced
    /// bundle records its `--agent` set here so the rebuild replays the
    /// same slice. Covered by the self-hash (deterministic build input);
    /// omitted from canonical TOML when `None`, so existing full bundles
    /// serialize identically and their self-hashes are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_agents: Option<Vec<String>>,
```

- [ ] **Step 4: Update the two `BundleMeta` literals**

In `crates/tau-pkg/src/bundle/manifest.rs`, in `sample_manifest()` (~line 327), add `selected_agents: None,` after the `target: ...` line in the `BundleMeta { .. }` literal.

In `crates/tau-pkg/src/bundle/build.rs` step 6 (~line 232), add `selected_agents: None,` after the `target: opts.target,` line in the `BundleMeta { .. }` literal. (Task 5 will replace this `None` with the computed value.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::`
Expected: PASS, including the two new tests and all existing `bundle::manifest` round-trip tests (the additive optional field doesn't perturb them).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/bundle/manifest.rs crates/tau-pkg/src/bundle/build.rs
git commit -m "feat(tau-pkg): add BundleMeta.selected_agents slice marker"
```

---

## Task 3: Add `BuildError::UnknownAgent`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build_error.rs`

- [ ] **Step 1: Add the variant**

In `crates/tau-pkg/src/bundle/build_error.rs`, add inside the `BuildError` enum (after `ManifestInvalid`, before `WriteFailed`):

```rust
    /// `--agent <id>` named an agent not present in the project config.
    #[error("unknown agent `{id}`; available agents: {}", available.join(", "))]
    UnknownAgent {
        /// The requested agent id that does not exist.
        id: String,
        /// All agent ids declared in the project (sorted), for the hint.
        available: Vec<String>,
    },
```

- [ ] **Step 2: Verify it compiles**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg`
Expected: compiles clean (new variant, no exhaustive `match` on `BuildError` in tau-pkg breaks — `exit_code_for` lives in tau-cli and is updated in Task 7).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/src/bundle/build_error.rs
git commit -m "feat(tau-pkg): add BuildError::UnknownAgent"
```

---

## Task 4: Add `agent_filter` to `BuildOptions` and fix all literals

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build.rs` (`BuildOptions` struct ~line 20; `opts` test helper ~line 379; literal ~line 690)
- Modify: `crates/tau-pkg/src/bundle/reproduce.rs` (literals ~line 130, 388, 492)
- Modify: `crates/tau-pkg/src/bundle/verify.rs` (literals ~line 285, 463, 545)
- Modify: `crates/tau-pkg/tests/bundle_build_e2e.rs` (literals ~line 128, 199, 210)
- Modify: `crates/tau-pkg/tests/bundle_verify_e2e.rs` (literals ~line 110, 134)
- Modify: `crates/tau-pkg/tests/bundle_reproduce_e2e.rs` (literals ~line 103, 121)
- Modify: `crates/tau-cli/src/cmd/build.rs` (literal ~line 36)

This is a mechanical struct change: add the field, then add `agent_filter: None,` to every existing `BuildOptions { .. }` literal. No behavior change yet.

- [ ] **Step 1: Add the field**

In `crates/tau-pkg/src/bundle/build.rs`, add to the `BuildOptions` struct (after `output_path`, before the closing `}` at ~line 29):

```rust
    /// Restrict the bundle to these agents and prune packages they don't
    /// reference. `None` builds every agent and keeps every package (the
    /// §C.2 behavior). The CLI maps an empty `--agent` set to `None`.
    pub agent_filter: Option<Vec<tau_domain::AgentId>>,
```

- [ ] **Step 2: Run check to discover every broken literal**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg 2>&1 | grep -E "missing field|BuildOptions"`
Expected: errors at each `BuildOptions { .. }` literal listed under **Files** above, "missing field `agent_filter`".

- [ ] **Step 3: Add `agent_filter: None,` to every `BuildOptions` literal**

In each of these locations, add `agent_filter: None,` inside the `BuildOptions { .. }` literal (after `output_path: ...`):

- `crates/tau-pkg/src/bundle/build.rs` — `opts()` helper (~line 379) and the explicit literal in `build_writes_to_explicit_output_path_when_set` (~line 690).
- `crates/tau-pkg/src/bundle/reproduce.rs` — the `verify_reproducible` rebuild call (~line 130), and test helpers `sample_manifest` (~line 388) and `build_minimal_bundle` (~line 492).
- `crates/tau-pkg/src/bundle/verify.rs` — three test literals (~line 285, 463, 545).
- `crates/tau-pkg/tests/bundle_build_e2e.rs` — three literals (~line 128, 199, 210).
- `crates/tau-pkg/tests/bundle_verify_e2e.rs` — two literals (~line 110, 134).
- `crates/tau-pkg/tests/bundle_reproduce_e2e.rs` — two literals (~line 103, 121).
- `crates/tau-cli/src/cmd/build.rs` — the `opts` literal in `run` (~line 36).

- [ ] **Step 4: Verify both crates compile and existing tests pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg && timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli`
Expected: both compile clean.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::`
Expected: PASS — no behavior change, `agent_filter: None` everywhere reproduces today's output.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/bundle/build.rs crates/tau-pkg/src/bundle/reproduce.rs crates/tau-pkg/src/bundle/verify.rs crates/tau-pkg/tests/bundle_build_e2e.rs crates/tau-pkg/tests/bundle_verify_e2e.rs crates/tau-pkg/tests/bundle_reproduce_e2e.rs crates/tau-cli/src/cmd/build.rs
git commit -m "feat(tau-pkg): add BuildOptions.agent_filter field (no-op default None)"
```

---

## Task 5: Implement slicing + package pruning in `build()`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build.rs` (insert slicing block between step 5 agent-sort ~line 214 and step 6 ~line 216; add unit tests in the `tests` module)

- [ ] **Step 1: Write the failing tests**

Add these helpers + tests to the `#[cfg(test)] mod tests` block in `crates/tau-pkg/src/bundle/build.rs`. The helper writes a 2-agent project where `alpha` requires tool `pkg-a` and `beta` requires `pkg-b`, plus a home package `pkg-home`; both tools + home are locked and their install dirs created:

```rust
    /// Two-agent project: alpha→requires pkg-a, beta→requires pkg-b,
    /// both home package pkg-home. All three package dirs exist so
    /// step-3 install verification passes. Returns the project root.
    fn two_agent_project(tmp: &std::path::Path) {
        std::fs::write(
            tmp.join("tau.toml"),
            r#"
[project]
name = "multi"
version = "0.1.0"

[agents.alpha]
display_name = "Alpha"
package = "pkg-home@^0.1"
llm_backend = "anthropic"

[agents.alpha.prompt]
system = "you are alpha"

[[agents.alpha.requires.tools]]
name = "pkg-a"
source = "https://example.com/pkg-a.git"

[agents.beta]
display_name = "Beta"
package = "pkg-home@^0.1"
llm_backend = "anthropic"

[agents.beta.prompt]
system = "you are beta"

[[agents.beta.requires.tools]]
name = "pkg-b"
source = "https://example.com/pkg-b.git"
"#,
        )
        .unwrap();
        for name in ["pkg-a", "pkg-b", "pkg-home"] {
            std::fs::create_dir_all(tmp.join(format!(".tau/packages/{name}/0.1.0"))).unwrap();
        }
        std::fs::write(
            tmp.join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "pkg-a"
active_version = "0.1.0"
source = "https://example.com/pkg-a.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"

[[package]]
name = "pkg-b"
active_version = "0.1.0"
source = "https://example.com/pkg-b.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000002"
installed_at = "2024-01-01T00:00:00Z"

[[package]]
name = "pkg-home"
active_version = "0.1.0"
source = "https://example.com/pkg-home.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000003"
installed_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
    }

    fn opts_filtered(root: &std::path::Path, ids: &[&str]) -> BuildOptions {
        BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: Some(ids.iter().map(|s| s.parse().unwrap()).collect()),
        }
    }

    fn read_bundle(path: &std::path::Path) -> crate::bundle::manifest::BundleManifest {
        let s = std::fs::read_to_string(path).unwrap();
        crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap()
    }

    #[test]
    fn build_agent_filter_none_keeps_all() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts(tmp.path())).unwrap().path);
        assert_eq!(m.agents.len(), 2);
        assert_eq!(m.packages.len(), 3);
        assert_eq!(m.bundle.selected_agents, None);
    }

    #[test]
    fn build_agent_filter_selects_single_agent() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts_filtered(tmp.path(), &["alpha"])).unwrap().path);
        assert_eq!(m.agents.len(), 1);
        assert_eq!(m.agents[0].id.as_str(), "alpha");
        assert_eq!(m.bundle.selected_agents, Some(vec!["alpha".to_string()]));
    }

    #[test]
    fn build_agent_filter_prunes_unreferenced_packages() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts_filtered(tmp.path(), &["alpha"])).unwrap().path);
        let names: Vec<&str> = m.packages.iter().map(|p| p.name.as_str()).collect();
        // alpha references pkg-a (required tool) + pkg-home (home pkg).
        assert!(names.contains(&"pkg-a"), "got {names:?}");
        assert!(names.contains(&"pkg-home"), "got {names:?}");
        assert!(!names.contains(&"pkg-b"), "pkg-b must be pruned; got {names:?}");
    }

    #[test]
    fn build_agent_filter_multiple_agents_unions_packages() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts_filtered(tmp.path(), &["alpha", "beta"])).unwrap().path);
        assert_eq!(m.agents.len(), 2);
        let names: Vec<&str> = m.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"pkg-a") && names.contains(&"pkg-b") && names.contains(&"pkg-home"), "got {names:?}");
        assert_eq!(m.bundle.selected_agents, Some(vec!["alpha".to_string(), "beta".to_string()]));
    }

    #[test]
    fn build_agent_filter_unknown_id_errors() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let err = build(opts_filtered(tmp.path(), &["ghost"])).unwrap_err();
        match err {
            BuildError::UnknownAgent { id, available } => {
                assert_eq!(id, "ghost");
                assert_eq!(available, vec!["alpha".to_string(), "beta".to_string()]);
            }
            other => panic!("expected UnknownAgent, got {other:?}"),
        }
    }

    #[test]
    fn build_agent_filter_is_reproducible() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let a = build(opts_filtered(tmp.path(), &["alpha"])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let b = build(opts_filtered(tmp.path(), &["alpha"])).unwrap();
        assert_eq!(a.sha256, b.sha256, "sliced build self-hash must be stable");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::build::tests::build_agent_filter`
Expected: FAIL — `agent_filter: Some(..)` currently has no effect, so `selects_single_agent` sees 2 agents, `prunes_unreferenced_packages` sees pkg-b, `unknown_id_errors` returns `Ok` not `UnknownAgent`.

- [ ] **Step 3: Implement the slicing block**

In `crates/tau-pkg/src/bundle/build.rs`, immediately after the `agents.sort_by(...)` call (end of step 5, ~line 214) and before the `// Step 6:` comment, insert:

```rust
    // Step 5.5: per-agent slicing + package pruning (spec §C.2.2).
    //
    // When `agent_filter` is None this block is skipped entirely and the
    // full agent + package sets pass through (the §C.2 behavior). When
    // Some, keep only the named agents and prune packages to their
    // direct reference closure, then record the slice so `tau verify
    // --bundle` can replay it.
    let selected_agents: Option<Vec<String>> = match &opts.agent_filter {
        None => None,
        Some(wanted) => {
            // Validate every requested id against the project config so
            // the error fires even for agents a later step would reject.
            let mut available: Vec<String> = project_config.agents.keys().cloned().collect();
            available.sort();
            for id in wanted {
                if !project_config.agents.contains_key(id.as_str()) {
                    return Err(BuildError::UnknownAgent {
                        id: id.as_str().to_owned(),
                        available: available.clone(),
                    });
                }
            }

            let wanted_set: std::collections::BTreeSet<&str> =
                wanted.iter().map(|a| a.as_str()).collect();

            // Keep only the selected agents.
            agents.retain(|a| wanted_set.contains(a.id.as_str()));

            // Package keep-set: each kept agent's home package
            // (parsed from `[agents.<id>].package`) ∪ its required tools.
            // The flat lockfile has no inter-package deps, so direct
            // reference closure is complete (spec §4.2 assumption).
            let mut keep: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for a in &agents {
                if let Some(entry) = project_config.agents.get(a.id.as_str()) {
                    // Empty / unparseable package field contributes
                    // nothing — never a new failure vs. the full build.
                    if let Ok((name, _req)) =
                        crate::project::agent::parse_package_ref(&entry.package)
                    {
                        keep.insert(name);
                    }
                }
                for t in &a.required_tools {
                    keep.insert(t.clone());
                }
            }
            packages.retain(|p| keep.contains(&p.name));

            // Record the requested ids (sorted) as the slice marker.
            let mut sel: Vec<String> = wanted.iter().map(|a| a.as_str().to_owned()).collect();
            sel.sort();
            sel.dedup();
            Some(sel)
        }
    };
```

Then change the `agents` and `packages` bindings (step 4 ~line 105 and step 5 ~line 153) from `let mut packages` / `let mut agents` — they are already `let mut`, so no change needed. Confirm both are `mut` (they are: `let mut packages: Vec<...>` and `let mut agents: Vec<BundleAgent>`).

Finally, in step 6's `BundleMeta { .. }` literal, replace the `selected_agents: None,` line (added in Task 2) with:

```rust
            selected_agents,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::build::`
Expected: PASS — all six new `build_agent_filter_*` tests plus the existing `build_*` tests.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/bundle/build.rs
git commit -m "feat(tau-pkg): slice agents + prune packages on BuildOptions.agent_filter"
```

---

## Task 6: Replay the slice in `verify_reproducible`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/reproduce.rs` (the rebuild `BuildOptions` ~line 130; add tests in the `tests` module)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tau-pkg/src/bundle/reproduce.rs`. Reuse the existing `ropts` helper; add a multi-agent fixture builder:

```rust
    /// Build a two-agent project (solo + extra) sliced to `solo`, and
    /// return (bundle_path, project_root tempdir kept alive by caller).
    fn build_sliced_solo(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::write(
            root.join("tau.toml"),
            r#"
[project]
name = "sliced-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are solo"

[agents.extra]
display_name = "Extra"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.extra.prompt]
system = "you are extra"
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let artifact = build(BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: Some(vec!["solo".parse().unwrap()]),
        })
        .unwrap();
        artifact.path
    }

    #[test]
    fn verify_reproducible_sliced_bundle_roundtrips() {
        let tmp = tempdir().unwrap();
        let bundle = build_sliced_solo(tmp.path());
        // Sanity: the shipped bundle records the slice.
        let shipped = BundleManifest::from_path(&bundle).unwrap();
        assert_eq!(shipped.bundle.selected_agents, Some(vec!["solo".to_string()]));
        assert_eq!(shipped.agents.len(), 1);

        let report = verify_reproducible(ropts(bundle, tmp.path())).expect("repro ran");
        assert!(report.reproducible, "sliced bundle must reproduce; diffs={:?}", report.diffs);
        assert!(report.diffs.is_empty());
    }

    #[test]
    fn verify_reproducible_full_bundle_still_roundtrips() {
        // Regression guard: a None-filter build has no selected_agents
        // and rebuilds full, exactly as before this feature.
        let tmp = tempdir().unwrap();
        let bundle = build_minimal_bundle(tmp.path());
        let shipped = BundleManifest::from_path(&bundle).unwrap();
        assert_eq!(shipped.bundle.selected_agents, None);
        let report = verify_reproducible(ropts(bundle, tmp.path())).expect("repro ran");
        assert!(report.reproducible, "full bundle must still reproduce; diffs={:?}", report.diffs);
    }

    #[test]
    fn verify_reproducible_sliced_bundle_detects_drift() {
        let tmp = tempdir().unwrap();
        let bundle = build_sliced_solo(tmp.path());
        // Mutate solo's prompt → rebuild's solo prompt hash differs.
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "sliced-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "you are MUTATED solo"

[agents.extra]
display_name = "Extra"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.extra.prompt]
system = "you are extra"
"#,
        )
        .unwrap();
        let report = verify_reproducible(ropts(bundle, tmp.path())).expect("repro ran");
        assert!(!report.reproducible, "prompt drift must break reproduction");
        assert!(
            report.diffs.iter().any(|d| matches!(d, ManifestDiff::AgentField { id, .. } if id == "solo"))
                || report.diffs.iter().any(|d| matches!(d, ManifestDiff::ProjectField { field, .. } if field == "tau_toml_sha256")),
            "expected a solo agent-field or tau_toml_sha256 diff; got {:?}", report.diffs,
        );
    }
```

Note: `BundleManifest::from_path` and `ManifestDiff::AgentField`/`ProjectField` already exist (see manifest.rs:211 and the diff enum in reproduce.rs). If `AgentField`'s exact field name for the agent id differs, adjust the `matches!` guard to the real field (grep `enum ManifestDiff`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce::tests::verify_reproducible_sliced`
Expected: FAIL — `verify_reproducible` currently rebuilds with `agent_filter: None` (set in Task 4), so it produces the full 2-agent bundle; its self-hash won't match the sliced shipped bundle → `reproducible == false`, failing the `roundtrips` assertion.

- [ ] **Step 3: Derive the filter from the shipped bundle**

In `crates/tau-pkg/src/bundle/reproduce.rs`, in `verify_reproducible`, just before the rebuild `build(BuildOptions { .. })` call (~line 130), add:

```rust
    // Replay the shipped bundle's slice (if any) so a sliced bundle
    // rebuilds to the same agent + package set. A full bundle has
    // `selected_agents == None` and rebuilds whole.
    let agent_filter: Option<Vec<tau_domain::AgentId>> = match &shipped.bundle.selected_agents {
        None => None,
        Some(ids) => {
            let parsed: Result<Vec<_>, _> =
                ids.iter().map(|s| s.parse::<tau_domain::AgentId>()).collect();
            Some(parsed.map_err(|e| ReproError::ShippedSelfHashInvalid {
                detail: format!("selected_agents contains an invalid agent id: {e}"),
            })?)
        }
    };
```

Then set the rebuild's `agent_filter` field (currently `agent_filter: None,` from Task 4) to:

```rust
        agent_filter,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce::`
Expected: PASS — sliced roundtrips, full still roundtrips, drift detected. Existing reproduce tests unchanged.

- [ ] **Step 5: Run the bundle e2e integration tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --test bundle_reproduce_e2e --test bundle_build_e2e --test bundle_verify_e2e`
Expected: PASS — Task 4's `agent_filter: None` additions keep these green.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/bundle/reproduce.rs
git commit -m "feat(tau-pkg): replay selected_agents slice in verify_reproducible"
```

---

## Task 7: CLI `--agent` flag + dispatch + exit code

**Files:**
- Modify: `crates/tau-cli/src/cli.rs` (`BuildArgs` ~line 193)
- Modify: `crates/tau-cli/src/cmd/build.rs` (`run` builds `agent_filter`; `exit_code_for` maps `UnknownAgent`; add unit test)

- [ ] **Step 1: Write the failing unit test**

In `crates/tau-cli/src/cmd/build.rs`, extend the `exit_code_mapping_per_spec` test (add inside it, after the existing assertions):

```rust
        // Unknown agent (bad --agent input) → 2.
        assert_eq!(
            exit_code_for(&BuildError::UnknownAgent {
                id: "ghost".into(),
                available: vec!["alpha".into()],
            }),
            2,
        );
```

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --lib cmd::build::tests::exit_code_mapping_per_spec`
Expected: FAIL to compile — `exit_code_for`'s `match` is non-exhaustive (missing `UnknownAgent`).

- [ ] **Step 3: Add the flag to `BuildArgs`**

In `crates/tau-cli/src/cli.rs`, add to the `BuildArgs` struct (after the `output` field, before the closing `}` at ~line 201):

```rust
    /// Restrict the bundle to one or more agents (repeatable:
    /// `--agent a --agent b`). When omitted, every agent is built.
    /// Selecting agents also prunes packages they don't reference.
    /// To avoid overwriting a full bundle at the default path, pass
    /// `-o`/`--output`.
    #[arg(long = "agent", value_name = "ID")]
    pub agents: Vec<String>,
```

- [ ] **Step 4: Map `UnknownAgent` to exit 2**

In `crates/tau-cli/src/cmd/build.rs`, in `exit_code_for`, add `UnknownAgent` to the exit-2 arm so it reads:

```rust
        BuildError::ProjectConfig(_)
        | BuildError::LockfileLoad(_)
        | BuildError::ManifestInvalid(_)
        | BuildError::UnknownAgent { .. } => 2,
```

- [ ] **Step 5: Build `agent_filter` in `run` and pass it**

In `crates/tau-cli/src/cmd/build.rs`, in `run`, after `target` is resolved and before the `let opts = BuildOptions { .. }` literal (~line 36), add:

```rust
    // Map the repeatable `--agent` flag to the builder's filter. Empty
    // → None (build all). Parse each id to AgentId; a malformed id is a
    // config-level input error (exit 2).
    let agent_filter = if args.agents.is_empty() {
        None
    } else {
        let mut parsed = Vec::with_capacity(args.agents.len());
        for raw in &args.agents {
            match raw.parse::<tau_domain::AgentId>() {
                Ok(id) => parsed.push(id),
                Err(e) => {
                    let _ = output.error(format!("invalid agent id '{raw}': {e}"));
                    std::process::exit(2);
                }
            }
        }
        Some(parsed)
    };
```

Then change the `BuildOptions` literal's `agent_filter: None,` (added in Task 4) to:

```rust
        agent_filter,
```

Confirm `tau_domain` is a dependency of `tau-cli` (it is — `cmd/build.rs` and siblings already reference `tau_domain` / `tau_ports`). If the `AgentId` import isn't in scope, use the fully-qualified `tau_domain::AgentId` as written above (no `use` needed).

- [ ] **Step 6: Run unit tests + check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --lib cmd::build::`
Expected: PASS — `exit_code_mapping_per_spec` (now covering `UnknownAgent`) and the existing `resolve_target_*` tests.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/build.rs
git commit -m "feat(tau-cli): tau build --agent flag + UnknownAgent exit code"
```

---

## Task 8: CLI integration tests + help snapshot

**Files:**
- Modify: `crates/tau-cli/tests/cmd_build.rs` (add a multi-agent fixture + three tests)
- Modify: `crates/tau-cli/tests/snapshots/help_snapshots__build_help.snap` (regenerate)

- [ ] **Step 1: Write the failing integration tests**

Add to `crates/tau-cli/tests/cmd_build.rs` (after the existing tests). Reuse `make_tau_home`. Add a 2-agent fixture writer:

```rust
/// Two-agent project (alpha + beta), empty lockfile (no packages so the
/// install-verify step is a no-op). Used by the `--agent` slice tests.
fn write_two_agent_project(root: &std::path::Path) {
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "multi"
version = "0.1.0"

[agents.alpha]
display_name = "Alpha"
package = "multi@^0.1"
llm_backend = "anthropic"

[agents.alpha.prompt]
system = "you are alpha"

[agents.beta]
display_name = "Beta"
package = "multi@^0.1"
llm_backend = "anthropic"

[agents.beta.prompt]
system = "you are beta"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();
}

#[test]
fn build_agent_flag_slices_bundle() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_two_agent_project(&project);
    let tau_home = make_tau_home(scratch.path());
    let out = project.join("alpha.tau");

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--agent", "alpha", "-o", out.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    let body = std::fs::read_to_string(&out).unwrap();
    let m = tau_pkg::bundle::manifest::BundleManifest::parse_str(&body).unwrap();
    assert_eq!(m.agents.len(), 1);
    assert_eq!(m.agents[0].id.as_str(), "alpha");
    assert_eq!(m.bundle.selected_agents, Some(vec!["alpha".to_string()]));
}

#[test]
fn build_agent_flag_unknown_exits_two() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_two_agent_project(&project);
    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--agent", "ghost"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ghost"))
        .stderr(predicate::str::contains("alpha"));
}

#[test]
fn build_agent_flag_repeatable() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_two_agent_project(&project);
    let tau_home = make_tau_home(scratch.path());
    let out = project.join("both.tau");

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--agent", "alpha", "--agent", "beta", "-o", out.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    let body = std::fs::read_to_string(&out).unwrap();
    let m = tau_pkg::bundle::manifest::BundleManifest::parse_str(&body).unwrap();
    assert_eq!(m.agents.len(), 2);
    assert_eq!(m.bundle.selected_agents, Some(vec!["alpha".to_string(), "beta".to_string()]));
}
```

Confirm `tau_pkg` is a dev-dependency of `tau-cli` for the integration test (check `crates/tau-cli/Cargo.toml` `[dev-dependencies]`; existing `cmd_build.rs` tests parse via stdout paths, so `tau_pkg` may not yet be a dev-dep). If absent, add `tau-pkg = { path = "../tau-pkg" }` under `[dev-dependencies]` in `crates/tau-cli/Cargo.toml`. Also confirm `predicate` is imported (top of `cmd_build.rs` — it is, via `predicates::prelude` for the existing exit-3 test).

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --test cmd_build build_agent_flag`
Expected: PASS (Task 7 already wired the behavior). If `tau_pkg` dev-dep was missing, Step 1 failed to compile until added — fix then rerun. These tests are an end-to-end guard over the Task 7 wiring; they should go green immediately.

- [ ] **Step 3: Regenerate the `build_help` snapshot**

The `--agent` flag changes `tau build --help`. Regenerate:

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl INSTA_UPDATE=always cargo test -p tau-cli --test <help-test-name>`

(Find the help test: `grep -rn "build_help\|assert_snapshot" crates/tau-cli/tests/`. It likely lives in a `help_snapshots.rs` integration test. Use that test's name above.)

Then review the diff: `git diff crates/tau-cli/tests/snapshots/help_snapshots__build_help.snap` — confirm the only change is the added `--agent <ID>` line (and any reflow). Do NOT accept unrelated snapshot churn.

- [ ] **Step 4: Run the full tau-cli + tau-pkg test suites**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli && timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/tests/cmd_build.rs crates/tau-cli/tests/snapshots/help_snapshots__build_help.snap crates/tau-cli/Cargo.toml
git commit -m "test(tau-cli): tau build --agent integration tests + help snapshot"
```

---

## Task 9: Final verification + docs

**Files:**
- Modify: `docs/superpowers/specs/2026-05-28-tau-build-agent-slice-design.md` (status note) — optional
- Check: ROADMAP §C for a per-agent-slicing line item to tick, if present.

- [ ] **Step 1: fmt + clippy on both crates**

Run:
```
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-pkg -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg -p tau-cli --all-targets
```
Expected: fmt clean, clippy zero warnings. If fmt fails, run without `--check` and recommit.

- [ ] **Step 2: Doctests (BundleMeta gained a field; sample doctests construct it via TOML, not literals, so should be unaffected)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg`
Expected: PASS.

- [ ] **Step 3: Tick the ROADMAP / spec status if applicable**

`grep -rn "C.2.2\|per-agent\|--agent" docs/ROADMAP*.md docs/**/ROADMAP*.md 2>/dev/null`. If a checklist item exists, tick it `[x]` and commit. Update the spec `Status:` line to note shipped if that's the repo convention (it isn't always — check sibling specs; PR-merge is the usual gate).

- [ ] **Step 4: Open the PR**

Use the agent push path (per `CLAUDE.md` AGENT PUSH RULES — Rust changes, gate is the only Linux validation surface):

```bash
scripts/agent-push.sh -u origin feat/tau-build-agent
gh pr create --title "feat(tau-cli): tau build --agent per-agent slicing (Phase 2 §C.2.2)" --body "$(cat <<'EOF'
## Summary
- `tau build --agent <id>` (repeatable) slices a bundle to the named agents and prunes packages they don't reference.
- New `[bundle].selected_agents` marker keeps sliced bundles reproducible under `tau verify --bundle`.
- Schema stays v1 (additive, skip-if-None); full builds are byte-identical to before.

## Test plan
- [ ] `cargo nextest run -p tau-pkg` (builder slicing + reproduce replay)
- [ ] `cargo nextest run -p tau-cli` (--agent integration + exit codes + help snapshot)
- [ ] `cargo test --doc -p tau-pkg`
- [ ] CI deep gate green

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review notes

- **Spec coverage:** §3 CLI → Task 7; §4.1 BuildOptions → Task 4; §4.2 slicing+pruning+lenient-package-parse → Task 5; §4.3 manifest field → Task 2; §4.4 UnknownAgent+exit2 → Tasks 3+7; §5 reproduce replay → Task 6; §6 output/naming (default path, `-o`) → Task 7 (help text) + Task 8 (`-o` in tests); §7 test plan → Tasks 2,5,6,7,8. All covered.
- **Type consistency:** `agent_filter: Option<Vec<tau_domain::AgentId>>` used identically in BuildOptions (Task 4), build slicing (Task 5), reproduce (Task 6), CLI (Task 7). `selected_agents: Option<Vec<String>>` consistent in manifest (Task 2), build (Task 5), reproduce read (Task 6), tests (Tasks 2,5,6,8). `BuildError::UnknownAgent { id: String, available: Vec<String> }` consistent across Tasks 3,5,7.
- **Known verify-at-impl-time points (flagged inline, not placeholders):** exact `ManifestDiff` variant field name for agent id (Task 6 Step 1); `tau_pkg` dev-dep presence in `tau-cli/Cargo.toml` (Task 8 Step 1); help-snapshot test name (Task 8 Step 3). Each has a grep + fallback in the step.
