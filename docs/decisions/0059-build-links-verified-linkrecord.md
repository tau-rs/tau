# ADR-0059: tau build links; bundles carry a verified LinkRecord; run trusts after verify.

**Status:** Proposed
**Date:** 2026-07-19
**Deciders:** tau core

## Context

Symbol binding to the installed world — "does every plugin, skill, tool, and
model this IR names actually resolve against what is installed, on this
platform?" — happens today at **every `tau run`**, scattered across the run
path as late `anyhow::bail!`s, and partly duplicated defensively:

- Plugin resolution from the lockfile (installed?, port match?, version
  satisfiable?) is checked per-LLM-backend and per-tool in `tau-cli`'s
  `plugin_loader.rs`.
- Package version satisfiability is re-derived in `tau-pkg`'s
  `project/agent.rs`.
- `SKILL.md` is read and parsed **on every** `skill.<name>.spawn`, with no
  cache.
- Model aliases are resolved three times: once correctly at lowering, and
  twice more defensively at run — one of the run-time paths falls back to a
  silent `"unresolved"` sentinel.

Worse, `tau run --bundle` re-lowers the whole project on every run using an
empty MCP contract cache, which produces a *different* IR hash for MCP
projects and is conservatively rejected as `IrSourceDivergence`. An MCP
project therefore cannot run from its own bundle — a confirmed bug.

This binding work is knowable statically, from the lockfile, package
manifests, and on-disk `SKILL.md` files, without spawning any plugin
process. Doing it once, at build time, and recording the outcome removes
the repeated computation, deletes the scattered `bail!`s and the
`"unresolved"` sentinel, and gives `run --bundle` something concrete to
verify instead of re-deriving from scratch.

## Decision

`tau-pkg` gains a new module, `tau-pkg/src/link.rs`, exposing:

```rust
pub fn link(
    cfg: &ProjectConfig,
    module: &IrModule,
    lockfile: &LockFile,
    scope: &Scope,
) -> Result<LinkOutcome, Vec<LinkError>>;

pub struct LinkOutcome {
    pub record: LinkRecord,
    pub parsed_skills: BTreeMap<String, tau_domain::SkillContent>,
}

pub struct LinkRecord {
    pub resolved_plugins: Vec<LinkedPlugin>,   // name, version, binary_sha256, provides
    pub resolved_skills:  Vec<LinkedSkill>,    // name, content_sha256, parsed_ok
    pub model_bindings:   BTreeMap<String, ModelRef>, // alias -> ModelRef
    pub platform:         TargetTriple,
    pub lockfile_sha256:  String,
}

pub enum LinkError {
    PluginNotInstalled { package: String },
    PluginPortMismatch { package: String, found: PortKind, expected: PortKind },
    VersionUnsatisfied { package: String, req: String },
    SkillMissing { package: String, path: String },
    SkillParse { package: String, detail: String },
}
```

**`link()` is a static linker, not a loader.** It validates the
package-level symbol table — every plugin the project references exists,
is installed at a version satisfying the requirement, and provides the
right port; every model alias resolves to a `ModelRef`; every installed
skill's `SKILL.md` reads and parses — entirely from the lockfile, package
manifests, and disk, with **no process spawning**. It runs once, after
`lower_ir` and before `build()` in `tau build`'s pipeline.

There is deliberately **no `ModelAliasUnknown` variant**.
`ProjectConfig::validate()` already rejects an agent whose `model` is
empty or names an alias absent from `[models]`
(`ProjectConfigError::MissingAgentModel` /
`ProjectConfigError::UnknownModelAlias`) at parse time. A `ProjectConfig`
that reaches `link()` therefore cannot carry an unresolvable alias, so
`link()`'s model-binding step is infallible: it only builds the
`alias -> ModelRef` map. `LinkError` has exactly the 5 variants above.

**Two checks stay at runtime, not because they're overlooked but because
they cannot be computed statically:**

1. **`ToolId` -> loaded-plugin binding.** There is no static `ToolId` ->
   package edge: `PluginManifest` carries `{provides, kind, bin}`, not tool
   names, so matching a workflow's `ToolId` references against a specific
   plugin's advertised tool names requires the plugin to actually be
   spawned and asked. This is the **dynamic loader**'s job. The cheap
   "5b" tool-name pre-check in the run path stays exactly where it is;
   `LinkRecord` carries no `tool_bindings` field and there is no
   `ToolUnbound` error variant.
2. **Sandbox adapter availability and plan construction.** `build_plan`
   lives in `tau-runtime-tokio`; `tau-pkg` cannot call it without creating
   a dependency cycle. Adapter availability is host-inherent — it depends
   on the machine actually running the bundle, which for a portable bundle
   is not knowable at build time on the build host. Sandbox planning and
   adapter probing stay a runtime concern, exactly as today. `LinkRecord`
   carries no sandbox fields; `resolved_plugins[].binary_sha256` plus the
   lockfile's existing `required_shapes` are sufficient for what the run
   path still needs.

**Precondition:** the `lockfile` argument must equal the lockfile persisted
on disk at `scope.lockfile_path()`. Plugin resolution reads only the
passed `lockfile`; skill resolution and `LinkRecord::lockfile_sha256` both
read from disk via `scope`. A `lockfile` that has drifted from disk is
undefined territory for skills specifically — a skill package listed in
the passed `lockfile` but absent from the on-disk lockfile surfaces as
`LinkError::SkillMissing` rather than being silently dropped. Every caller
in PR 2 passes the lockfile it just loaded from `scope.lockfile_path()`,
so the invariant holds in practice.

**Errors are collected, never short-circuited.** `link()` returns
`Err(Vec<LinkError>)` containing every fault found across plugins, models,
and skills, sorted by a stable key `(variant_rank, package_name)` so
identical inputs yield an identical error list regardless of which caller
invoked `link()` — the no-drift bar shared with `build`, `check`, and
dev-`run`.

## Consequences

**Positive:**

- One implementation of "does the world satisfy the IR" replaces four
  scattered, partially-duplicated call sites.
- The `"unresolved"` model-alias sentinel and the redundant re-resolutions
  at run time become deletable (PR 2).
- `SKILL.md` moves from "read + parsed on every spawn" to "read + parsed
  once at link time"; a corrupt skill fails the **build**, not a spawn
  mid-run.
- `run --bundle` gets something to verify — bundle IR hash plus
  `LinkRecord` invariants (lockfile sha, per-plugin/-skill presence and
  sha, platform match) — instead of re-lowering the whole project, which
  both restores byte-for-byte trust semantics and removes the MCP
  empty-cache bug's root cause (fixed properly in PR 3 by re-lowering
  `verify --bundle`'s reproduce path with the pinned MCP resolver instead).
- `tau check` gains a "link" category for free: the same `link()` function
  run in dry-run mode against the current installed set.

**Negative / obligations:**

- PR 1 (this ADR + `link()` + `LinkRecord`/`LinkError` + unit tests) ships
  dead code: nothing calls `link()` yet. It is unit-tested in isolation
  (truth table: one fixture per `LinkError` variant, plus a multi-error
  collection test) but not wired into any command.
- PR 2 must wire the three callers — `tau build`, dev `tau run`/`tau chat`,
  and `tau check` — behind one no-drift test asserting identical findings
  for the same broken fixture across all three, embed `LinkRecord` in the
  bundle manifest as an additive optional field, and construct the
  record-seeded `SkillResolver` adapter in `tau-runtime-tokio` from
  `LinkOutcome::parsed_skills`.
- PR 3 must replace `run --bundle`'s re-lowering with verify-then-trust
  against the embedded `LinkRecord`, and fix the MCP reproduce-path bug by
  re-lowering with the pinned MCP resolver instead of an empty cache.
- PR 4, unrelated to linking but sequenced after it, flips the credential
  posture from silent ambient-env fallback to a hard startup error with an
  explicit `--allow-ambient-credentials` escape hatch; it gets its own ADR.
- `tau-pkg` gains a dependency on `tau-ir` for `ModelRef`/`IrModule`. This
  is not a new cycle: `tau-ir` is the no_std base crate.

## Alternatives considered

**A. Keep binding resolution scattered at run time, just deduplicate the
logic into a shared helper called from each run-time site.**
Rejected. This does not fix the core problem: the cost (fs reads for
skills, lockfile scans for plugins) is still paid on every run, and
`run --bundle` still has to re-derive the world instead of trusting a
verified artifact. It also does nothing for the MCP re-lowering bug, which
is caused by re-lowering at all, not by how the binding logic is
organized.

**B. Make `link()` a full loader — spawn plugins and bind `ToolId`s and
sandbox adapters at build time, embedding a complete "world" into the
bundle.**
Rejected. Spawning processes and probing sandbox adapters at build time
requires the build host to have the same live capabilities as the eventual
run host, which is false for portable bundles built on one machine and run
on another. It would also pull `tau-runtime-tokio` into `tau-pkg`,
creating a dependency cycle (`build_plan` lives in `tau-runtime-tokio`,
which itself depends on `tau-pkg`). The static/dynamic split keeps
`tau-pkg` a pure, no-process linker and leaves host-inherent decisions to
the runtime loader, where they belong.

**C. Record skill bodies (not just hashes) directly in the serialized
`LinkRecord`.**
Rejected. `LinkRecord` is embedded in the bundle manifest and needs to
stay small and stable; skill bodies can be large and are not needed for
the manifest's own verification checks (`content_sha256` is sufficient to
detect drift). Instead, `link()` returns the parsed bodies separately as
`LinkOutcome::parsed_skills`, used only in-process to seed the runtime
`SkillResolver`; they never travel through serialization.

**D. Give `link()` a `ModelAliasUnknown` error variant to mirror the
handoff's original sketch.**
Rejected once `ProjectConfig::validate()`'s existing guarantees were
checked against the actual code: an unresolvable model alias cannot reach
`link()` in the first place, because `ProjectConfig` construction already
rejects it. Adding a dead error variant would misrepresent `link()`'s
actual failure surface and invite untestable code paths.
