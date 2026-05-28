# `tau build --agent <id>` — per-agent bundle slicing — Phase 2 §C.2.2 design

**Status:** Accepted
**Date:** 2026-05-28
**Authors:** titouanlebocq
**Depends on:** §C.2 `tau build` MVP producer (PR #242, merged), §C.2.1 build flags (PR #251, merged), §E `tau verify --bundle` reproducibility (PR #250, merged)

## 1. Goal

Add `--agent <id>` to `tau build` so a multi-agent project can emit a bundle scoped to a single agent (or an explicit subset). The deferred follow-up from §C.2 / §C.2.1.

A sliced bundle keeps only the named agents in `agents[]` **and** prunes `packages[]` down to the packages those agents actually reference, so the artifact is a faithful, minimal deployment unit for that agent — not the whole project with one agent's metadata.

The hard constraint: a sliced bundle must remain **reproducible** under `tau verify --bundle`. `verify_reproducible` rebuilds from the project tree and compares self-hashes; it must be able to replay the same slice.

## 2. Headline decisions

- **Slicing lives in `tau_pkg::bundle::build`**, gated by a new `BuildOptions.agent_filter: Option<Vec<AgentId>>`. `None` = build all agents = today's behavior, byte-for-byte unchanged (no pruning, no new fields emitted).
- **Pruning is part of slicing, not a separate flag.** When `agent_filter` is `Some`, drop every package not in the kept agents' reference closure. A "slice" that still ships every other agent's plugins would defeat the purpose.
- **Agent→package mapping is direct.** `RequiredTool.name` is a `PackageName` ("package name to resolve"). The closure for a kept agent is `{ name from its [agents.<id>].package ref } ∪ { each required_tools[].name }`. No fuzzy resolution needed.
- **Reproducibility via a recorded marker.** Add optional `[bundle].selected_agents: Option<Vec<String>>`, present only on sliced bundles. `tau verify --bundle` reads it and replays it as the rebuild's `agent_filter`. This keeps verify's clean "rebuild and compare" model and makes the slice explicit, self-documenting provenance on a security-relevant artifact.
- **Schema stays v1.** The new field is additive + `skip_serializing_if = "Option::is_none"`, so existing full bundles serialize identically and their self-hashes still verify. No `schema_version` bump.
- **`project.tau_toml_sha256` always hashes the full source tau.toml.** Slicing changes the manifest, not the source identity. Build and rebuild hash the same full file, so this field is slice-invariant.

## 3. CLI surface

```rust
// crates/tau-cli/src/cli.rs — extend the existing BuildArgs
#[derive(Args, Debug)]
pub struct BuildArgs {
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<std::path::PathBuf>,
    /// Restrict the bundle to one or more agents (repeatable). When
    /// omitted, all agents are built. Prunes packages not referenced
    /// by the selected agents.
    #[arg(long = "agent", value_name = "ID")]
    pub agents: Vec<String>,
}
```

`clap` collects repeated `--agent a --agent b` into `agents: Vec<String>` (default action for a `Vec` field is append). Empty vec → `agent_filter: None`. Non-empty → parse each into `AgentId`, dedupe + sort, → `agent_filter: Some(...)`.

The CLI layer maps an empty/non-empty vec to the `Option`; `BuildOptions.agent_filter` is the single source of truth the builder reads.

## 4. Builder changes (`tau_pkg::bundle::build`)

### 4.1 `BuildOptions`

```rust
pub struct BuildOptions {
    pub project_root: PathBuf,
    pub target: TargetTriple,
    pub output_path: Option<PathBuf>,
    /// Restrict the bundle to these agents and prune unreferenced
    /// packages. `None` builds every agent and keeps every package
    /// (the §C.2 behavior).
    pub agent_filter: Option<Vec<tau_domain::AgentId>>,
}
```

Every existing `BuildOptions { .. }` literal (build.rs tests, reproduce.rs:130, cmd/build.rs, cmd_build.rs, bundle_build_e2e.rs) gains `agent_filter: None`. A grep-and-add at plan time; the field is not `Default`-able on the struct (no `#[derive(Default)]`), so each literal is updated explicitly.

### 4.2 Slicing in `build()`

Inserted between step 5 (gather agents) and step 6 (assemble manifest), operating on the already-built `agents` and `packages` vecs:

1. **Validate + select.** If `agent_filter` is `Some(ids)`:
   - For each requested id, error `BuildError::UnknownAgent { id, available }` (sorted available ids) if it's not in `project_config.agents`. Validate against the *project config*, not the post-gather `agents` vec, so the error fires even on agents that would fail a later gather step — clearer diagnostics.
   - Retain only `agents` whose id is in the requested set.
2. **Compute the keep-set of package names:**
   ```
   keep = ⋃ over kept agents of
       { package-name parsed from [agents.<id>].package }   // the agent's home package
     ∪ { t.name for t in agent.requires.tools }             // its required tools
   ```
   The agent's `package` field is `"<name>@<semver-req>"`; reuse the existing `parse_package_ref` (currently a private `fn` in `crates/tau-pkg/src/project/agent.rs:286`, returns `Result<(String, VersionReq), String>` — bump to `pub(crate)` so `bundle/build.rs` can call it within the crate) to extract the name. `required_tools` names are already `PackageName`s. If an agent's `package` field is empty or unparseable, skip it (contributes nothing to the keep-set, no error): the §C.2 full-build path never parsed it, so slicing must not introduce a new failure mode for projects that build today.
3. **Prune `packages`** to those whose `name ∈ keep`. Names in `keep` that aren't in the lockfile are simply not present (their absence is install's concern, not build's — step 3 already gated install-state for *locked* packages).
4. **Record the marker.** Set `selected_agents = Some(sorted requested id strings)` on `BundleMeta`. When `agent_filter` is `None`, leave it `None`.

When `agent_filter` is `None`, none of the above runs: `agents` and `packages` pass through whole and `selected_agents` is `None`.

**Assumption (stated):** the lockfile is flat with no inter-package dependency edges (`BundlePackage` carries no `deps`), so a package is reachable only via a direct agent reference. Direct-reference closure is therefore complete. Transitive plugin-to-plugin closure is out of scope (no dependency graph exists to traverse); if that model is ever added, pruning must be revisited.

### 4.3 Manifest field

```rust
// crates/tau-pkg/src/bundle/manifest.rs — BundleMeta
pub struct BundleMeta {
    pub sha256: String,
    pub created_at: String,
    pub tau_version: String,
    pub target: TargetTriple,
    /// Agent ids this bundle was sliced to (sorted), or absent for a
    /// full build. Drives `tau verify --bundle` reproduction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_agents: Option<Vec<String>>,
}
```

`selected_agents` is covered by the self-hash (it's deterministic build input, and two sliced builds of the same project + same `--agent` set must produce the same hash). Because it's `skip_serializing_if`, a full build omits it from the canonical TOML, so every already-shipped full bundle's self-hash is unaffected.

### 4.4 Errors

New `BuildError::UnknownAgent { id: String, available: Vec<String> }`. CLI exit code **2** (bad input / config), alongside `ProjectConfig`/`LockfileLoad`/`ManifestInvalid` in `exit_code_for`.

## 5. Reproducibility (`tau_pkg::bundle::reproduce`)

`verify_reproducible` (reproduce.rs) currently rebuilds with `agent_filter` absent. Change the rebuild's `BuildOptions` to derive the filter from the shipped bundle:

```rust
let agent_filter = shipped.bundle.selected_agents.as_ref().map(|ids| {
    ids.iter()
       .map(|s| s.parse::<tau_domain::AgentId>())
       .collect::<Result<Vec<_>, _>>()
}).transpose().map_err(/* ReproError::ShippedSelfHashInvalid-style: malformed marker */)?;

let artifact = build(BuildOptions {
    project_root: opts.project_root.clone(),
    target: shipped.bundle.target,
    output_path: Some(rebuilt_path.clone()),
    agent_filter,
})?;
```

- **Full bundle** (`selected_agents == None`) → rebuild full → unchanged from today.
- **Sliced bundle** → rebuild replays the same slice + prune → same `agents`, same `packages`, same `selected_agents` → identical self-hash → reproducible.
- A malformed `selected_agents` (unparseable id) on a bundle whose self-hash *did* verify is a corrupt/hand-edited artifact; surface a distinct `ReproError` variant rather than panicking.

The existing `diff_manifests` already diffs agents by id and packages by name, so a slice that fails to reproduce (e.g. project drifted, an agent removed) produces a meaningful diff with no extra work.

## 6. Output / naming

- Default output path stays `<project>/<name>-<version>.tau` for both full and sliced builds. A sliced build to the default path **overwrites** a previously-built full bundle at that path; documented in `--agent` help. Distinct filenames are the user's job via `-o` (e.g. `tau build --agent researcher -o researcher.tau`). No special-case naming — fewer rules, and `tau verify --bundle` takes an explicit path so naming never affects it.
- Human + JSON artifact output (`path`/`sha256`/`size_bytes`) unchanged from §C.2.1. The slice is observable by parsing the bundle's `[bundle].selected_agents`, not via new CLI output fields.

## 7. Test plan

**Builder unit tests (`bundle/build.rs`):**
- `build_agent_filter_none_keeps_all` — existing happy-path project with 2 agents + 2 packages, `agent_filter: None` → both agents, both packages, no `selected_agents` in the written bundle.
- `build_agent_filter_selects_single_agent` — `Some([alpha])` on a 2-agent project → `agents == [alpha]`, `selected_agents == Some(["alpha"])`.
- `build_agent_filter_prunes_unreferenced_packages` — agent `alpha` requires tool `pkg-a`; agent `beta` requires `pkg-b`; both packages locked+installed. `Some([alpha])` → packages == `[pkg-a]` (pkg-b pruned). Assert pkg-b absent.
- `build_agent_filter_keeps_home_package` — agent's `[agents.<id>].package = "pkg-home@^0.1"`, no required tools; `pkg-home` locked+installed → packages == `[pkg-home]`.
- `build_agent_filter_multiple_agents_unions_packages` — `Some([alpha, beta])` → both agents, union of both closures.
- `build_agent_filter_unknown_id_errors` — `Some([ghost])` → `BuildError::UnknownAgent { id: "ghost", available }`, `available` sorted and contains the real ids.
- `build_agent_filter_is_reproducible` — two sliced builds (>1s apart) of the same project + same filter → equal self-hashes (created_at excluded, selected_agents stable).

**Reproduce tests (`bundle/reproduce.rs`):**
- `verify_reproducible_sliced_bundle_roundtrips` — build with `Some([solo])` on a multi-agent project, then `verify_reproducible` → `reproducible == true`, no diffs.
- `verify_reproducible_full_bundle_still_roundtrips` — regression guard: a `None` build still reproduces (no `selected_agents` field).
- `verify_reproducible_sliced_bundle_detects_drift` — slice to `[solo]`, mutate the project's `solo` prompt, re-verify → `reproducible == false` with an agent-field diff.

**Manifest tests (`bundle/manifest.rs`):**
- `selected_agents_omitted_when_none` — a `BundleMeta` with `selected_agents: None` serializes without the key; round-trips back to `None`.
- `selected_agents_round_trips_when_some` — `Some(["a","b"])` survives serialize→parse.

**CLI exit-code test (`cmd/build.rs`):** extend `exit_code_mapping_per_spec` — `UnknownAgent` → 2.

**CLI integration tests (`crates/tau-cli/tests/cmd_build.rs`):**
- `build_agent_flag_slices_bundle` — fixture project with 2 agents; `tau build --agent <one>` → exit 0; parse the bundle, assert one agent + `selected_agents`.
- `build_agent_flag_unknown_exits_two` — `tau build --agent ghost` → exit 2; stderr names `ghost` + lists available ids.
- `build_agent_flag_repeatable` — `tau build --agent a --agent b` → bundle has both.

**Help snapshot:** `tau build --help` now shows `--agent`. Regenerate the `build_help` snapshot.

## 8. Out of scope

- Transitive package pruning (no dependency graph exists in the flat lockfile; §4.2 assumption).
- Per-agent output-filename defaulting (`-o` covers it).
- Slicing by package, capability, or tag. YAGNI — only agents.
- Any change to `tau run --bundle` consumption: a sliced bundle is a normal bundle with fewer agents; the consumer already handles arbitrary agent sets.

## 9. References

- §C.2 spec — `2026-05-27-tau-build-design.md` (the producer this extends; deferred `--agent` there)
- §C.2.1 spec — `2026-05-28-tau-build-flags-design.md` (sibling flag work; named `--agent` as the remaining deferral)
- §E spec — `2026-05-28-tau-verify-bundle-design.md` (the reproducibility contract this must not break)
- ADR-0035 — bundle format (`schema_version` discipline)
