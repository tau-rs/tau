# ADR-0025 — Skills foundation (manifest extension)

> **Updated 2026-05-31 (β.1.4):** runtime references below are split
> between `tau-runtime-core` (no_std + alloc kernel) and
> `tau-runtime-tokio` (host shell).

**Status:** Accepted 2026-05-13.
**Branch / PR:** `feat/skills-1-manifest-extension` (PR pending).
**Spec:** `docs/superpowers/specs/2026-05-12-skills-1-manifest-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-13-skills-1-manifest-extension.md`.

## Context

ROADMAP §16 — "Skills as first-class packages." Constitution G10
commits to Skills being first-class in core ("Skills and MCP are
first-class concepts in core. Tau understands the Agent Skills spec
natively"). `kinds::SKILL = "skill"` has been a recognized
`PackageKind` since the v0.1 manifest design, but no runtime concept,
manifest block, parser, or install pipeline has existed.

Skills-1 is the first of six sub-projects that close this gap. It
ships the manifest types + parser + interpolation-variable constant
in `tau-domain` only — no `tau-pkg`, `tau-runtime`, or `tau-cli`
changes. Skills-2 (install pipeline), Skills-3 (discovery), Skills-4
(runtime invocation), Skills-5 (Agent Skills spec compliance), and
Skills-6 (reference packages + docs) follow as separate PRs.

## Decision

A tau skill package is a **directory with two manifest files**:

- **`SKILL.md`** — content. Pure Anthropic skill format: YAML
  frontmatter + Markdown body. Bit-identical to what claude-code /
  claude.ai / any compliant runtime expects. Zero tau-specific
  extensions in this file.
- **`tau.toml`** — packaging. Capability declaration, tool / skill
  dependencies, version, source. The same shape as a plugin or any
  other tau package, with a new `[skill]` block.

A tau skill IS an Anthropic skill (strip `tau.toml`, ship to
claude-code). An Anthropic skill becomes a tau skill by adding
`tau.toml`. No translation; no two-spec divergence.

Skills-1 lands:
- `tau-domain::package::skill` module with `SkillManifest`,
  `SkillFrontmatter`, `SkillContent`, `SkillContentError`.
- `parse_skill_md` function: frontmatter splitter (handles `---`
  delimiters + CRLF) + `serde_yaml`-driven YAML parse + required-
  field validation (`name`, `description`).
- `UncheckedManifest.skill: Option<SkillManifest>` field, defaulting
  to `None` (mirrors the existing `plugin: Option<PluginManifest>`
  pattern).
- `PackageManifest::skill()` accessor.
- `SKILL_DIR_VAR = "${SKILL_DIR}"` public constant for the
  interpolation variable. Symbolic in v1; substitution lives in
  Skills-4.

## Alternatives considered

During the brainstorm, three alternatives were considered before
landing on the two-layer design:

1. **Typed `[skill]` block embedding `system_prompt` inline.** Rejected:
   TOML triple-quoted strings are awkward for long prompts; the format
   diverges from the Anthropic ecosystem; no cross-runtime portability.
2. **Metadata-only `[skill]` + separate prompt file in a tau-specific
   layout.** Rejected: invents a layout convention that diverges from
   Anthropic skills for no benefit. The chosen design is this option
   except the layout convention IS the Anthropic format.
3. **Adopt the Agent Skills spec format verbatim as the only
   manifest.** Rejected as the *only* manifest: would require either
   replacing `tau.toml` or splitting capability declarations across
   two manifest files. The chosen design adopts the Anthropic format
   for content (`SKILL.md`) while keeping tau's packaging machinery
   (capabilities, tool deps, lockfile) in `tau.toml` — best of both.

## Consequences

- `serde_yaml` (MIT/Apache-2.0) is now a tau-domain optional dep
  behind the `serde` feature. Allow-listed under cargo-deny.
- The public re-export surface of `tau_domain` grows by ~6 items
  (the new types + parse function + constant).
- Skills-2 can now wire `tau install <skill-pkg>` through the
  install pipeline using the parser + manifest field added here.
- Skills-3 / Skills-4 can read `SkillManifest` via the existing
  manifest serde flow.

## Out of scope (deferred to Skills-2+)

- The `tau install` install-time validation pipeline (Skills-2).
- `${SKILL_DIR}` runtime substitution (Skills-4).
- Cross-field validation that `kind = "skill"` rejects `[plugin]`
  block (Skills-2 — closer to where the install-time error message
  is rendered).
- Lockfile schema migration for cached frontmatter + content_sha256
  (Skills-2).

## References

- Constitution G10 (Skills + MCP as first-class).
- Spec: `docs/superpowers/specs/2026-05-12-skills-1-manifest-design.md`.
- Priority queue: `docs/superpowers/specs/2026-05-12-post-multi-agent-priority-queue.md`.
- ROADMAP §16.
