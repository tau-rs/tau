# Link phase at `tau build` + verified `LinkRecord` in the bundle (D10-B)

**Status:** Approved (brainstorm) — ready for `writing-plans`
**Date:** 2026-07-19
**Handoff:** `.context/attachments/w9FCSH/pasted_text_2026-07-19_13-03-44.txt` (D10-B)
**ADR:** ADR-0059 (this design) lands in PR 1; credential-flip ADR at PR 4 (next-free then)
**Builds on:** #284 (pinned MCP resolver), ADR-0052 (per-agent model resolution)
**Verified against:** `maseru` @ `e3aacd6f` (main advanced past the handoff's `d678802d`)

## Problem

Symbol binding to the installed world — "does every plugin/skill/tool/model
this IR names actually resolve against what is installed, on this platform?"
— happens today at **every `tau run`**, scattered across the run path as late
`anyhow::bail!`s, and partly duplicated defensively:

- Plugin resolution from lockfile: `plugin_loader.rs:367-397` (4 checks: name
  parse, not-installed, data-only-rejection, port mismatch), invoked
  per-LLM-backend (`:253-258`) and per-tool (`:291-293`).
- Package version satisfiability: `agent.rs:157-206` (core at `:182-195`).
- tool-id → plugin binding: `ir_dispatcher.rs:184-210` ("5b Pre-check"),
  duplicated at invoke (`:474-478`, `:577-584`).
- Skill `SKILL.md` **read + parse on every `skill.<name>.spawn`**:
  `skill_resolver_impl.rs:58-66` (no cache).
- Model alias resolved 3×: lowering (correct — `tau-ir-lower/.../parse.rs:285-296`),
  `plugin_loader.rs:147-156` (defensive), and `agent.rs:213-226` with a silent
  `"unresolved"` sentinel fallback (`:217-219`, tagged `TODO(Task 4)`).
- Sandbox adapter + per-plugin plans recomputed at run (`plugin_loader.rs:169-250`,
  per-plugin `build_plan` at `:263-269`, `:298-299`) though statically exposed
  via `tau resolve --check-sandbox`.

Worse, **`tau run --bundle` re-lowers the whole project every run**
(`run.rs:102-142` → `verify_bundle_against_source` at `:781-801`) using an
**empty MCP contract cache** (`run.rs:786`). Re-lowering an MCP project with an
empty cache produces a *different* IR hash, which is conservatively rejected
with `IrSourceDivergence`. **Consequence: an MCP project cannot run from its own
bundle.** This is a confirmed bug (regression test
`run_bundle_rejects_ir_lowered_from_a_different_source` at `run.rs:1051-1060`;
the fn doc at `:771-780` already names the intended fix).

## Decision

**One link function, computed once at `tau build`, recorded as a verified
`LinkRecord` in the bundle; three build-time/dev callers share it; `tau run
--bundle` verifies-then-trusts the record instead of re-lowering.**

- `link(cfg, module, lockfile, installed) -> Result<LinkRecord, Vec<LinkError>>`
  lives in a new `tau-pkg/src/link.rs` (flat sibling to `resolve.rs`/`install.rs`;
  tau-pkg has no `resolve/` submodule tree — precedent for multi-file features is
  `bundle/`).
- The run path **calls the recorded bindings instead of re-deriving them**. The
  `"unresolved"` sentinel and the redundant alias re-resolutions are deleted —
  the IR + `LinkRecord` are the only sources of truth.
- `tau run --bundle` stops re-lowering: it verifies the bundle IR hash (existing)
  + `LinkRecord` invariants against the current installed set, then **trusts**.
- The MCP bug is fixed where lowering-equivalence is genuinely still wanted
  (`tau verify --bundle`'s reproduce path): re-lower with the **pinned** MCP
  resolver, not an empty cache.

This mirrors the D1-B "one implementation, three callers, no-drift test" pattern
already used for the governance gate design.

## Sequencing decision (resolves the handoff's stale premise)

The handoff assumed D1-B (governance gate in `build.rs`) had landed. **It has
not** — governance-by-default (`GOV000`) is the uncommitted
`allow-ungoverned-opt-out` branch. `link()` needs the lowered `IrModule` +
lockfile + installed set + platform, so it slots **after `lower_ir`, before
`build(bundle)`** regardless of governance.

**Decision:** proceed now; insert `link()` after `lower_ir` and before
`build()`. Governance is a *policy* gate on the source; link is a *binding* gate
on the world — policy should reject first, so when the governance gate lands it
slots **ahead of** link. This work leaves the insertion point
governance-compatible and does not block on D1-B.

Resulting `build.rs` order:

```
resolve path → extract config → resolve_target → parse --agent
  → resolve_mcp_cache → lower_ir (typecheck)
  → [governance gate, when it lands]
  → link()  ← NEW
  → build(bundle, link_record) → persist mcp_entries → emit_artifact
```

## Design

### 1. `LinkRecord` + `LinkError` (PR 1, `tau-pkg/src/link.rs`)

```rust
pub struct LinkRecord {
    resolved_plugins: Vec<ResolvedPlugin>,  // name, version, binary sha256
    resolved_skills:  Vec<ResolvedSkill>,   // name, content sha256, parsed_ok
    tool_bindings:    BTreeMap<ToolId, PluginRef>,
    model_bindings:   BTreeMap<ModelAlias, ModelRef>,  // final; no re-resolution
    sandbox_plans:    Vec<PluginSandboxPlan>,          // platform-dependent, hence:
    platform:         TargetTriple,
    lockfile_sha256:  String,
}

pub enum LinkError {
    PluginNotInstalled { .. }, PluginPortMismatch { .. },
    VersionUnsatisfied { .. }, SkillMissing { .. }, SkillParse { .. },
    ToolUnbound { .. }, ModelAliasUnknown { .. }, SandboxUnavailable { .. },
}

pub fn link(
    cfg: &ProjectConfig, module: &IrModule,
    lockfile: &Lockfile, installed: &InstalledSet,
) -> Result<LinkRecord, Vec<LinkError>>;
```

- **Collect ALL errors** (`Vec`), never stop at the first — linker UX. Ordering
  is deterministic (sorted by a stable key) so identical inputs yield identical
  error lists across callers (the no-drift bar).
- `link()` absorbs the logic currently at the scattered sites above; it does not
  re-implement resolution primitives it can reuse (lockfile lookup, version
  satisfiability, sandbox `build_plan`).
- All `BTreeMap`/`Vec` fields carry deterministic ordering for reproducible
  serialization into the manifest.

### 2. Skills: parse-once, seed the resolver (resolves the handoff's contradiction)

The handoff's struct records only `sha256 + parsed_ok`, but the prose says
per-spawn fs reads become a lookup. A sha + bool cannot be spawned. **Chosen
approach (Option A): the record stores `sha256 + parsed_ok`; the *parsed body*
lives in an in-memory map produced once at link time.**

- `link()` reads + parses every `SKILL.md` **once**, recording `content sha256`
  and `parsed_ok` in the `LinkRecord`, and returns (alongside the record) a
  `BTreeMap<SkillName, ParsedSkill>`.
- A new **record-seeded `SkillResolver` adapter** in `tau-runtime-tokio` is
  constructed from that map. Per-spawn `resolve()` becomes a **map lookup** —
  the per-spawn `std::fs::read_to_string` + `parse_skill_md` at
  `skill_resolver_impl.rs:58-66` is deleted.
- The **`SkillResolver` port trait is unchanged** (hexagonal boundary intact) —
  only a new impl is added and the fs-backed impl retired.
- Bundle-run rebuilds the map by reading the **verified-present, sha-matched**
  installed `SKILL.md`s once at startup (consistent with how run --bundle treats
  plugins: verify-present-with-sha, then trust the installed set — it does *not*
  embed plugin binaries in the bundle, so skills are not embedded either).
- Content sha256 in the record makes **skill drift detectable at verify** and a
  **corrupt `SKILL.md` fails at BUILD** (via `SkillParse`), not at spawn.

### 3. Callers (PR 2) — three, one implementation

1. **`tau build`** (`build.rs`, after `lower_ir`, before `build()`): call
   `link()`, fail the build on `Vec<LinkError>`, embed `LinkRecord` in the bundle
   manifest as an **additive optional field** with deterministic ordering
   (coordinate with any in-flight `bundle/manifest.rs` changes, D6/D8/D9).
2. **dev `tau run` / `tau chat`**: call the **same** `link()` at startup as one
   phase with **one** error surface, replacing the scattered `bail!`s. These stay
   runtime-executed by nature (no bundle) but are now complete and coherent. The
   run path **calls** `LinkRecord` bindings instead of re-deriving them.
3. **`tau check`**: a new **"link" category** = `link()` in dry-run against the
   current installed set; `LinkError` → `Severity::Error` findings.

**Cleanups in PR 2** (same touched files):
- Delete the `"unresolved"` sentinel (`agent.rs:217-219`) and the redundant
  alias re-resolutions (`plugin_loader.rs:147-156`, `agent.rs:213-226`) — IR +
  `LinkRecord` are the only sources.
- `AgentId::from_str().expect(...)` panics at `resolve_helpers.rs:34,67` →
  structured errors (same flavor as the sentinel removal — turning
  panics/sentinels into structured results; belongs with the other PR-2 cleanups
  because PR 2 already rewrites `resolve_helpers.rs` to call bindings).

**No-drift test:** the same broken fixture yields **identical** `LinkError`
findings via `build`, `check`, and dev-`run`.

### 4. `tau run --bundle` trust model + MCP fix (PR 3)

**Replace** re-lowering (`run.rs:102-142` / `:770-801`) with **verification**:

- bundle IR hash (existing) **+** `LinkRecord` checks:
  - `lockfile_sha256` matches the cwd lockfile;
  - each resolved plugin/skill still **present with matching sha** (reuse
    `VerifyError::PackageMissing` / `PackageDrift`, mapped at `run.rs:816-817`);
  - `platform` matches the current triple (mismatch → clear
    `"bundle was linked for <triple>"` error).
- On pass: **TRUST**. Delete the duplicated startup re-resolution
  (`resolve_and_install_for_agent` at `ir_dispatcher.rs:94-99`, lockfile reload
  `:167-173`); the 5b re-check (`ir_dispatcher.rs:184-210`) collapses to a debug
  assertion.

**The MCP fix:** where lowering-equivalence is still wanted (`tau verify
--bundle`'s reproduce path), re-lower with contracts from the **pinned** resolver
(`PinnedResolver` over `LockedMcpEntry`, `tau-mcp/src/contract/resolver.rs:128`)
instead of the empty cache at `run.rs:786`. **Regression test: an MCP project's
bundle RUNS.**

**Genuinely-runtime remains** (keep as-is — well-designed): MCP reachability +
handshake drift check; credential env-var **values** (posture change is PR 4).

### 5. Credential posture flip (PR 4) — behavior change, isolated

Today `inject_credentials` silently falls back to ambient env-var passthrough
when a credential is not found in any provider (`process.rs:243`,
`Ok(None) => {}`). This is a *runtime credential-resolution posture*, orthogonal
to build-time binding, and it is **behavior-breaking** (projects relying on
ambient env break at startup). It gets its **own PR + own ADR + own escape
hatch**:

- Default: **hard startup error** naming the credential id **and** the searched
  providers.
- Escape hatch: `--allow-ambient-credentials` (and/or `[credentials] ambient =
  true`) restores passthrough — explicit opt-out, per the "escape hatches
  explicit" / "build enforcement, opt-out loudly" stance.

## PR split (4 PRs)

| PR | Headline | Contents |
|---|---|---|
| **1** | `link()` + `LinkRecord` + ADR-0059 | New `tau-pkg/src/link.rs`; `LinkRecord`/`LinkError`; truth-table + multi-error tests. ADR + SUMMARY.md. Nothing calls it yet (unit-tested dead code). |
| **2** | 3 callers + cleanup | Wire build/dev-run/check; embed record in manifest; delete `"unresolved"` sentinel + redundant alias re-resolution; `AgentId .expect` → structured errors; no-drift test. |
| **3** | run --bundle trust model + **MCP-bundle bugfix** | Stop re-lowering; verify-then-trust; MCP reproduce path uses pinned resolver; MCP-bundle-RUNS regression test. |
| **4** | Credential posture flip | `Ok(None)` → hard error + `--allow-ambient-credentials` escape hatch + credential-flip ADR. |

Rationale for splitting from the handoff's 3-PR plan: the credential flip is the
only behavior-breaking change and is orthogonal to linking — isolating it gives
it its own revert boundary and ADR. The MCP fix is the highest-value, most
independent bugfix (MCP bundles cannot run today) and is the headline of PR 3
rather than a footnote.

## Tests

- **`link()` truth table:** one fixture per `LinkError` variant; multi-error
  collection (all errors, deterministic order).
- **No-drift:** the same broken fixture yields identical findings via `build`,
  `check`, and dev-`run`.
- **Bundle e2e:** build → uninstall a plugin → `run --bundle` fails at **VERIFY**
  with `PackageMissing` (not mid-run); reinstall → runs **without re-lowering**
  (assert via span/log absence of the lowering phase).
- **MCP bundle e2e (the bug):** a cassette-backed MCP project builds **and RUNS**
  from its bundle.
- **Skill spawn e2e:** `SKILL.md` read happens **zero times during run** (fs spy
  or span assertion); a corrupt `SKILL.md` fails at **BUILD**.
- **Platform mismatch:** a `LinkRecord` with a different triple → clear error.
- **Credential (PR 4):** missing credential → hard startup error naming id +
  providers; `--allow-ambient-credentials` restores passthrough.

## Deliverables & conventions

- ADR-0059 "tau build links; bundles carry a verified LinkRecord; run trusts
  after verify" (0057 held by open #423, 0058 taken); add to
  `docs/decisions/` + `SUMMARY.md`. Credential-flip ADR at PR 4 (next-free then).
- `feat/*` branches; conventional commits; CLAUDE.md cargo rules
  (`CARGO_TARGET_DIR`, `-p <crate>`, `timeout`, `CARGO_INCREMENTAL=0`); nextest.
- Manifest change is additive/optional; coordinate with in-flight
  `bundle/manifest.rs` work (D6/D8/D9) if any.

## Open questions — resolved in brainstorm

1. **D1-B dependency** → proceed now; link after `lower_ir`/before `build()`;
   governance slots ahead of link when it lands (Q1: A).
2. **Skill record content** → `sha + parsed_ok` in record, parse-once into
   in-memory map, record-seeded resolver, port trait unchanged (Q2: A).
3. **Credential posture** → hard error + `--allow-ambient-credentials` escape
   hatch (Q3: A).
4. **PR split** → 4 PRs; `AgentId .expect` cleanup in PR 2; credential flip
   isolated in PR 4 (Q4: 4-PR).
