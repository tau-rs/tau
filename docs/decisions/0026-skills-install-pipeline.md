# ADR-0026 — Skills install pipeline (Skills-2)

> **Updated 2026-05-31 (β.1.4):** runtime references below are split
> between `tau-runtime-core` (no_std + alloc kernel) and
> `tau-runtime-tokio` (host shell).

**Status:** Accepted 2026-05-13.
**Branch / PR:** `feat/skills-2-install-pipeline` (PR pending).
**Spec:** `docs/superpowers/specs/2026-05-12-skills-2-install-pipeline-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-13-skills-2-install-pipeline.md`.
**Depends on:** ADR-0025 (Skills-1 foundation).

## Context

Second of 6 sub-projects from ROADMAP §16 (Skills as first-class
packages, Constitution G10). Skills-1 (ADR-0025, PR #63) shipped the
manifest types + parser + interpolation-variable constant in
tau-domain only — no install pipeline integration. Skills-2 wires
`tau install <skill-pkg>` end-to-end through tau-pkg so a skill
package fetches, validates SKILL.md content + frontmatter, resolves
transitive deps, computes a content SHA-256 + caches frontmatter,
and writes to the lockfile.

## Decision

A new module `tau-pkg::skill_check` mirrors `tau-pkg::sandbox_check`.
Single entry point `cross_check_skill_package(install_dir, manifest)`
runs a 4-step flow:

1. Read `SKILL.md` from `install_dir/<content_path>` (default
   `"SKILL.md"`, from Skills-1's `SkillManifest.content`).
2. Parse via `tau_domain::parse_skill_md` (Skills-1).
3. Validate `frontmatter.name == manifest.name`.
4. Reference lint (**hard-fail**): scan body for `${SKILL_DIR}/<rel-path>`
   substrings; if no `[[capabilities]] kind = "fs.read"` glob covers,
   reject.

`install_with_options` dispatches on `manifest.kind()`:
- `kind = "plugin"` → existing Layer 2 `sandbox_check` (unchanged)
- `kind = "skill"` → new `skill_check`, then compute SHA-256 of
  SKILL.md bytes + snapshot frontmatter into a new `LockedSkill`
  lockfile entry
- other kinds → neither (existing behavior unchanged)

The Layer 2 sandbox cross-check is skipped for skill packages because
skills have no plugin process to spawn.

Lockfile schema bumps v4 → v5. New `LockedPackage.skill: Option<LockedSkill>`
field. `LockedSkill { content_sha256: String, frontmatter:
SkillFrontmatterSnapshot }`. Auto-upgrade follows the standard
`was_pre_vN` + `tracing::warn` once-per-process pattern (v2→v3
precedent).

`tau verify` gains a `VerifyStatus::SkillContentDrift { name,
expected, got }` variant parallel to `BinaryDrift`. Re-hashes SKILL.md
on verify, compares to the cached `content_sha256`.

tau-domain rejects packages combining `kind = "skill"` with a
`[plugin]` block at parse time via the new
`PackageManifestError::SkillCannotHavePluginBlock`. Cross-field
validation runs in `UncheckedManifest::validate()`.

## Alternatives considered

### Inline `skill_check` logic in `install_with_options`

Rejected: `sandbox_check` is its own module for the same reasons
(size of `install.rs`, future shareability, testability in isolation).
The shared `parse_skill_md` parser + the discrete 4-step flow benefit
from being callable from Skills-3 (`tau skill list / show` — reads
frontmatter on demand) and Skills-4 (runtime invocation — reads
body) without dragging install.rs along.

### Warn-only reference lint

The initial Skills-2 spec draft (commit `78f4352` in PR #62) had a
warn-only lint. Rejected on review (commit `6176e18` in PR #62):
- True-positive cost (caught at runtime when the agent gets a
  capability-denied error mid-task) is severe — confusing failure
  mode for the agent's LLM, no clear remediation path from the
  runtime error alone.
- False-positive cost (skill author adds one `[[capabilities]]`
  line or removes a stale reference) is trivial.
- Hard-fail at install time matches how the rest of tau handles
  capability declarations: explicit > implicit. Warn-only would be
  the outlier.

### Keep lockfile schema unchanged

The initial Skills-2 spec draft had no lockfile migration —
`tau skill list` would re-read every `SKILL.md` from disk on demand.
Rejected on the same PR #62 review:
- A scope with 30-50 installed skills incurs 30-50 file opens per
  list. Noticeable latency, compounds across CLI surfaces.
- Drift is solved by the same SHA-256 mechanism that protects plugin
  binaries today (`tau verify`).
- Cached frontmatter is ~200 bytes per skill — negligible lockfile
  growth.

## Consequences

- `tau-pkg`'s public surface grows by `cross_check_skill_package` +
  `verify_skill_content` + `LockedSkill` + `SkillFrontmatterSnapshot`.
- 4 new `InstallError` variants.
- 1 new `PackageManifestError` variant.
- 1 new `VerifyStatus` variant (the inner enum on `VerifyReport`).
- Lockfile schema v4 → v5 auto-upgrade. v4 lockfiles load cleanly;
  skill packages installed pre-v5 will surface as "unverified" via
  `tau verify` until reinstalled.
- `tau install <skill-pkg>` is now fully functional end-to-end.
- Skills-3 / Skills-4 unblocked.

## Out of scope (deferred to Skills-3+)

- `tau skill list` / `tau skill show` CLI subcommands → Skills-3
  (consumes the cached frontmatter added here).
- Runtime invocation (`agent.<skill>.spawn` resolves to installed
  manifest; SKILL.md body becomes spawned child's system_prompt) →
  Skills-4.
- Agent Skills spec export / import → Skills-5.
- Plugin-package symmetry (`kind = "plugin"` rejecting `[skill]`
  block) — Skills-5 if needed.

## References

- Spec: `docs/superpowers/specs/2026-05-12-skills-2-install-pipeline-design.md`
- Plan: `docs/superpowers/plans/2026-05-13-skills-2-install-pipeline.md`
- Skills-1 ADR: `docs/decisions/0025-skills-foundation.md`
- ROADMAP §16
