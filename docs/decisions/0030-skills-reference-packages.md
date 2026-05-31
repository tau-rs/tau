# ADR-0030 — Skills reference packages + user docs (Skills-6)

> **Updated 2026-05-31 (β.1.4):** runtime references below are split
> between `tau-runtime-core` (no_std + alloc kernel) and
> `tau-runtime-tokio` (host shell).

**Status:** Accepted 2026-05-16.
**Branch / PR:** `feat/skills-6-reference-packages` (PR #115).
**Spec:** `docs/superpowers/specs/2026-05-16-skills-6-reference-packages-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-16-skills-6-reference-packages.md`.
**Depends on:** ADR-0025 (Skills-1), ADR-0026 (Skills-2), ADR-0027
(Skills-3), ADR-0028 (Skills-4), ADR-0029 (Skills-5).

## Context

Final sub-project of ROADMAP §16. Skills-1 through Skills-5 built
the infrastructure; Skills-6 ships the **content** that turns the
infrastructure into a complete, usable product:

- 3 exemplary skill packages under `skills/` (critic, fact-checker,
  pr-reviewer) covering pure-prompt / fs.read / process.spawn axes.
- 6 mdBook documentation pages filling all four Diátaxis quadrants.
- 9 end-to-end integration tests proving the user story works in CI
  across Linux, macOS, Windows.

After Skills-6, a new contributor can clone tau, build it, install
a reference skill, render it, and export it back to Anthropic
format — the entire Skills track makes sense end-to-end without
prior knowledge.

## Decision

Three locked decisions:

### D1 — Three reference skills covering three capability axes

- `critic` (no capabilities) → Anthropic-roundtrip proof
- `fact-checker` (fs.read on `${SKILL_DIR}/references/**`) → substitution + multi-file
- `pr-reviewer` (process.spawn on git + rg) → third axis

Rejected: 1-skill (too minimal) and 2-skill (skips the process.spawn axis).

### D2 — Add-only: no refactor of existing tempdir test fixtures

Skills-1–5 inline-synthesize critic fixtures in 10+ test files.
Refactoring them to reference `skills/critic/` would risk subtle
regressions and merge conflicts with parallel Claude sessions for
marginal benefit. Skills-6 ADDS new integration tests against the
in-tree skills; existing fixtures stay as-is.

### D3 — Full Diátaxis mdBook documentation

mdBook is already wired with the four Diátaxis quadrants
(tutorials / how-to / reference / explanation). Skills-6 fills each
quadrant: tutorial (narrative walkthrough), 3 how-to recipes,
manifest schema reference, two-layer architecture explanation.
Auto-deploys to GitHub Pages via PR #67's existing workflow.

Rejected: in-repo README only (under-invests in user-facing surface);
how-to + reference only (lacks tutorial narrative + explanation
anchoring).

## Alternatives considered

- 1-skill scope (just critic). Rejected: doesn't prove capability story.
- 2-skill scope (critic + fact-checker, no pr-reviewer). Rejected:
  skips the process.spawn capability axis.
- Refactor existing test fixtures. Rejected: 10+ files touched,
  refactor risk, merge-conflict risk.
- Separate `tau-skills` git repo. Rejected: in-tree is fine for
  proof-of-concept; external repo addressable when external
  versioning needs emerge.
- In-repo README only. Rejected: mdBook is already wired + auto-deploys.
- Tutorial-only docs. Rejected: reference + explanation are load-bearing
  for authoring + understanding.
- `tau skill new <name>` scaffolding command. Rejected: useful but
  separate sub-project; not blocking the user story Skills-6 ships.

## Consequences

- **New top-level `skills/` directory** (sibling to `crates/`).
  Future reference skills land here.
- **New `docs/tutorials/build-your-first-skill.md`**, three
  `docs/how-to/` recipes, `docs/reference/skill-manifest-schema.md`,
  `docs/explanation/two-layer-skills.md`. All indexed in
  `docs/SUMMARY.md`.
- **9 new integration tests** across `crates/tau-pkg/tests/` and
  `crates/tau-cli/tests/`. Existing tests untouched.
- **`.gitattributes`** forces LF on `skills/**/SKILL.md` to keep
  the byte-identical export roundtrip test passing on Windows.
- **No new external dependencies. No CI changes.** The new tests
  run under the existing `test-stable` matrix.

## Closes ROADMAP §16

Skills-6 is the final sub-project of the Skills track. ROADMAP §16
is complete after this PR merges. Future Skills work is additive
(more reference packages, `tau skill new`, marketplace, etc.) and
sits outside §16.

## References

- Spec: `docs/superpowers/specs/2026-05-16-skills-6-reference-packages-design.md`
- Plan: `docs/superpowers/plans/2026-05-16-skills-6-reference-packages.md`
- Predecessor ADRs: 0025 (foundation), 0026 (install pipeline),
  0027 (discovery), 0028 (runtime invocation), 0029 (Anthropic interop)
- ROADMAP §16
