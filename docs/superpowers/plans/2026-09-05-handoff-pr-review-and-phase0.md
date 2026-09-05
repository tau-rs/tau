# HANDOFF — land the redesign backlog PR, then start Phase 0

**From:** the 2026-09-01→05 backlog session (branch
`claude/tau-redesign-backlog-8dqrjb`, remote).
**To:** the local Claude Code session on the maintainer's machine.
**Authority:** the consolidated design
[`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
(§10 decisions LOCKED — do not re-litigate) and the ADR wave 0071–0077 on
this branch. The backlog-creation mission
([`2026-09-01-handoff-redesign-backlog.md`](2026-09-01-handoff-redesign-backlog.md))
is **complete**; your job is to land it and start executing.

---

## State you inherit (verified 2026-09-05)

`claude/tau-redesign-backlog-8dqrjb` is 5 commits ahead of `main`
(c65a968), 0 behind, docs-only, no PR open anywhere:

1. `f22c3f7` — design doc + backlog handoff + banner-stamped 2026-08-29
   framing doc (cherry-picked from `claude/pipeline-creation-patterns-dsidka`,
   identical content — that branch needs **no PR of its own**; if it ever
   merges separately, identical files merge clean).
2. `3d3fb1d` — vision-roadmap E-0..E-4 epics + v2 backlog; EPIC 5.3
   superseded; EPIC 5 DoD re-pointed; ROADMAP retirements (killed-item
   narrowing, β.8 "one way", δ.2 QuickJS).
3. `47ce7f4` — ADR wave 0071–0077; banners on ADR-0022 (tau-workflow) +
   ADR-0041; numbering-collision note + index in `docs/decisions/README.md`;
   SUMMARY chapters; CONSTITUTION G6/QG12 + cheatsheet + tau-philosophy
   amendments.
4. `fa71a97` — implementation plans E-0..E-4 (`2026-09-01-epic-e*.md`).
5. `e99857e` — implementation trees (authoring-surfaces, instruction-set,
   ops-lane, exposures).

Known risks / open threads:

- **Book build was NOT run in the remote container** (mdbook not
  installed there). A scripted relative-link check over all 28 changed
  .md files passed, but linkcheck (`warning-policy = "error"`) is
  stricter — build locally before/while the PR is open (DOCS RULES).
- **ADR-0077 amends CONSTITUTION G6/QG12.** Per `docs/decisions/README.md`,
  guideline-changing ADRs wait ≥24h between draft and merge — drafted
  2026-09-01, so the window has elapsed; note it in the PR body.
- **PR #687** (`feat/621-wasm-guest-flip`) overlaps E-2 Task 10 (the
  `any-wasi-strict` feature repair). Nothing to do now; E-2's plan and
  the trees already carry the coordinate-don't-duplicate note.

## Your mission (in order)

1. **Sanity-build the book locally** (you have the binaries;
   the remote session didn't):
   `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` → only
   `[INFO]` lines; then `rm -rf docs/book`. Fix any linkcheck fallout
   with a fixup commit on this branch (identity-pinned commit per AGENT
   PUSH RULES; docs-only ⇒ `--no-verify` is acceptable).
2. **Open the PR** for `claude/tau-redesign-backlog-8dqrjb` → `main`.
   Body: what each of the 5 commits carries (list above), the ADR-0077
   24h note, the mdbook-verification status, and that decisions are
   locked per design §10 (review = "does the record match the design",
   not "re-open the design").
3. **Enrol auto-merge + babysit:**
   `gh pr merge <PR#> --squash --delete-branch --auto`; poke
   `gh pr update-branch <PR#>` whenever it goes BEHIND; drive CI to
   green (docs-check/docs-deploy are the real gates). Address review
   comments in place — but locked §10 decisions are answered by citing
   the design/ADR, not re-argued.
4. **After merge, start E-0** on a **fresh branch off updated `main`**
   (never stack on this one):
   [`2026-09-01-epic-e0-align-and-clean.md`](2026-09-01-epic-e0-align-and-clean.md)
   via superpowers:executing-plans (or subagent-driven-development).
   Task 1 is the verify-the-paper-trail gate; then the `tau-workflow`
   deletion (T2–T3), then dead weight (T4–T6 — T6 has a deliberate
   stop-and-verify on `tau-plugin-base`: it's a Dockerfile dir CI may
   still build). One green commit per task; update the
   [authoring-surfaces tree](../implementation-trees/authoring-surfaces.md)
   as you go (its update protocol is in the file header).
5. E-1..E-4 follow the same pattern, one epic per branch/PR train, in
   phase order. Do **not** plan v2 items (backlog-only per the roadmap).

## Constraints you must honor

- `CLAUDE.md` in full: CARGO RULES (per-role `CARGO_TARGET_DIR`, `-p`
  scoping, timeouts, `CARGO_INCREMENTAL=0`, nextest), ISSUE RULES
  (sweep `gh pr list --search` before creating anything), AGENT PUSH
  RULES (identity-pinned commits), DOCS RULES (book build + SUMMARY).
- Never push to `claude/tau-redesign-backlog-8dqrjb` after it merges,
  and never to `claude/pipeline-creation-patterns-dsidka` at all.
- No-flag-day: every removal per its stated deprecation path; E-0 is
  **zero behavior change** (the one sanctioned message change is the
  `tau embed --host c` named error, plan T5).
- ADR numbering: next free is 0078; check open PRs touching
  `docs/decisions/` before claiming it (collision note in the README).

## Where everything is

Backlog: [`vision-roadmap.md`](vision-roadmap.md) (redesign section) ·
ADRs: `docs/decisions/0071..0077-*.md` · Plans:
`2026-09-01-epic-e{0..4}-*.md` (this directory) · Trees:
[`../implementation-trees/`](../implementation-trees/authoring-surfaces.md)
(authoring-surfaces · instruction-set · ops-lane · exposures) — each
tree's **Next slices** section is the ranked work queue, and its
**Discoveries** log carries this session's code-verification findings.
