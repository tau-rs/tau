# ADR-0029 — Skills Anthropic interop (Skills-5)

> **Updated 2026-05-31 (β.1.4):** runtime references below are split
> between `tau-runtime-core` (no_std + alloc kernel) and
> `tau-runtime-tokio` (host shell).

**Status:** Accepted 2026-05-16.
**Branch / PR:** `feat/skills-5-anthropic-interop-design` (PR #102).
**Spec:** `docs/superpowers/specs/2026-05-15-skills-5-anthropic-interop-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-15-skills-5-anthropic-interop.md`.
**Depends on:** ADR-0025 (Skills-1), ADR-0026 (Skills-2), ADR-0027 (Skills-3), ADR-0028 (Skills-4).

## Context

Fifth of 6 sub-projects from ROADMAP §16. Skills-1 established the two-layer architecture ("a tau skill IS an Anthropic skill plus tau.toml"). Skills-5 ships the tooling that makes that bidirectional in practice: import vanilla Anthropic skills into tau, export tau-installed skills back to vanilla Anthropic format, and validate Anthropic-conformance of installed skills.

## Decision

Five locked decisions:

### D1 — Bidirectional scope (export + import + conformance)

Picked from a 4-option scope question (export-only / bidirectional / conformance-only / extension-key full-interop). Bidirectional gives ergonomic UX for the common "consume an Anthropic skill from GitHub" flow; conformance closes the validation loop. The `x-tau-capabilities` YAML extension for round-trippable export of capability-bearing skills was rejected as premature — only export-without-capabilities use case is concrete today.

### D2 — Both auto-detect inside `tau install` AND explicit `tau skill import`

`tau install <src>` detects Anthropic format and synthesizes a `tau.toml` transparently (ergonomic happy path). `tau skill import <src> --output <dir>` produces an editable directory so users can inspect or customize the synthesized tau.toml before installing.

### D3 — Capabilities dropped on export

Synthesized manifests start `capabilities = []`. Skill authors who want capabilities must add them by hand (via `tau skill import` + edit). On export, capabilities (and `requires_skills`) are silently dropped from the output with an stderr note. `tau skill export --strict` makes drops a hard error.

For non-SKILL.md content (refs/, assets/, etc.), export uses a simple **copy-everything-except-tau.toml** strategy. `SkillManifest` does not currently carry a `files` glob (Skills-1 schema), so this is the natural approach: the install pipeline already copies the full source tree into `<scope>/.tau/packages/<name>/<version>/`, and export walks that directory verbatim minus `tau.toml`.

### D4 — Lockfile schema v5 → v6 with `synthesized_from`

`LockedPackage.synthesized_from: Option<SynthesizedSource>` records the manifest origin. Enum currently has one variant: `SynthesizedSource::Anthropic`. Field defaults to `None` for v5 entries on read; lockfile auto-upgrades to v6 on next write (existing pattern from v4→v5 in PR #64). `tau skill show` displays `Source format: synthesized (Anthropic Agent Skills)` when the field is `Some`.

### D5 — `tau verify --anthropic-strict`

New flag on the existing `tau verify` command. Without the flag, behavior unchanged. With the flag, additionally checks each installed skill against Anthropic SKILL.md conformance:
- Frontmatter must parse as valid YAML with required `name` + `description`
- `description` must be non-whitespace
- Body must be non-whitespace

Emits `VerifyStatus::AnthropicConformance { skill_name, issue }` with `AnthropicConformanceIssue::{MissingDescription, EmptyBody, MalformedFrontmatter}` failure modes.

## Alternatives considered

- **Conformance-only Skills-5** (no commands, just validation in `tau install` + `tau verify --anthropic-strict`). Rejected: import is the highest-leverage ergonomic feature; defers the most valuable UX gain.
- **Export-only Skills-5** (no import). Rejected for the same reason.
- **Auto-detect only inside `tau install`** (no explicit `tau skill import`). Rejected: power users want to inspect/edit the synthesized tau.toml; without explicit import, they'd have to edit the on-disk install location post-fact (awkward).
- **Explicit `tau skill import` only** (no auto-detect). Rejected: forces every user to know about format mismatch and run extra steps; common case should Just Work.
- **`x-tau-capabilities` YAML extension key for round-trippable export.** Rejected: YAGNI; no real use case yet for round-trip with capability preservation.
- **Plural-format `--format <foo>` flag.** Rejected: only Anthropic format is in scope; premature plumbing.
- **Bulk export (`tau skill export --all`).** Rejected: YAGNI; trivially scriptable via `tau skill list --json | jq | xargs`.

## Consequences

- **`tau-domain`** public surface grows by `SkillFormat`, `detect_format`, `synthesize_manifest_from_skill_md`, `SynthesizeError`.
- **`tau-pkg`** public surface grows by new `synthesize` module (`synthesize_anthropic_skill` + tau-pkg's own `SynthesizeError`) + `SynthesizedSource` + 2 new `InstallError` variants (`NotASkillPackage`, `SynthesizeFailed`) + `AnthropicConformanceIssue` + new `VerifyStatus::AnthropicConformance` variant + `verify_all_with_options(scope, anthropic_strict)` entry point. Existing `verify_all` preserved as a delegating wrapper.
- **`tau-pkg::install`**: Anthropic-format installs now write the synthesized `tau.toml` to disk at `<scope>/.tau/packages/<name>/<version>/tau.toml` — surfaced during T7 integration as a gap; without it `tau skill show` + `tau skill export` couldn't operate on Anthropic-installed packages.
- **`tau-cli`** public surface grows by `SkillSubcommand::{Import, Export}` + `--anthropic-strict` flag on `tau verify`.
- **Lockfile schema v6** (additive); v5 reads cleanly with `synthesized_from = None`; v5→v6 upgrade is automatic on next write.
- **`tau skill import` URL handling**: All Git-scheme URLs (including `file://`) route through `git clone`. Only bare local paths (no scheme) use filesystem copy. This was a T7 integration fix.
- **No new external dependencies.** No CI changes.

## Discovered during implementation

Two T7 integration gaps surfaced and were fixed in scope:

1. **Synthesized `tau.toml` was never written to the install directory.** T3 added the in-memory manifest synthesis path but kept the install behavior unchanged in terms of what landed on disk. `tau skill show` + `tau skill export` failed for Anthropic-installed skills because they require `tau.toml` to exist. Fixed by writing the synthesized manifest to the staging directory before the staging→final rename.

2. **`tau skill import` treated `file://` URLs as local-path fs copies.** For bare git repos, this copied `.git/` metadata instead of working-tree content. Fixed by routing all Git-scheme URLs (including `file://`) through `git clone`; only bare local paths now use fs copy.

Both gaps blocked the Skills-5 user story end-to-end. T7's roundtrip e2e tests caught them; the fix landed in scope so the un-ignored tests pass.

## Out of scope (deferred to Skills-6+ / future)

- **Reference skill packages** themselves — Skills-6.
- **Sub-skill `requires_skills` cross-format mapping** — silently dropped on export (advisory only).
- **Conformance against future Anthropic spec revisions** — re-evaluate when the next major spec lands.
- **`tau skill convert`** as an alias or alternative to import/export — Skills-5 v2 if requested.
- **MCP-adjacent interop** (consuming MCP server descriptors as tau skills) — separate sub-project; out of ROADMAP §16 scope.
- **`x-tau-capabilities` YAML extension key** for capability preservation across export — revisit if a concrete use case emerges.

## References

- Spec: `docs/superpowers/specs/2026-05-15-skills-5-anthropic-interop-design.md`
- Plan: `docs/superpowers/plans/2026-05-15-skills-5-anthropic-interop.md`
- Skills-1 ADR: `docs/decisions/0025-skills-foundation.md`
- Skills-2 ADR: `docs/decisions/0026-skills-install-pipeline.md`
- Skills-3 ADR: `docs/decisions/0027-skills-discovery.md`
- Skills-4 ADR: `docs/decisions/0028-skills-runtime-invocation.md`
- ROADMAP §16
- Priority queue: `docs/superpowers/specs/2026-05-12-post-multi-agent-priority-queue.md`
