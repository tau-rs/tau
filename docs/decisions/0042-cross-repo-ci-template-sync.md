# ADR-0042: Cross-repo CI template sync (tau = source of truth)

**Status:** Accepted
**Date:** 2026-06-11
**Supersedes:** —

## Context

tau is the most mature of four sibling repos (cairn, cairn-ui, tau-ui, tau) and
is the de-facto canonical CI source. But nothing *operationalized* "tau = source
of truth": there was no `renovate.json`, no `repo-file-sync-action` config, and
no sync workflow on `main` — only `.github/dependabot.yml`, which bumps
dependencies but does not distribute shared files cross-repo. CI drift between
the four repos was therefore silent rot, not a visible signal.

The audit (`audit/devops.md` §3 anti-drift, §4 sync-template item, Diagram 1)
defines the target as the **"B+C"** model:

- **B — self-contained workflows.** Each repo keeps its FULL `ci.yml`; there is
  NO runtime `workflow_call` to a central moving-tag workflow. One bad change can
  never red all four repos at once, and every `ci.yml` is debuggable in isolation.
- **C — thin SHA-pinned composite actions.** Stable atomic steps only
  (`setup-rust`, `place-fixture-binaries`), consumed via
  `uses: ./.github/actions/...`.

tau already satisfies B and C. The missing piece is the **sync mechanism** that
propagates the canonical surface to the three siblings as a reviewable PR. This
ADR also justifies adding a new workflow `name:` (`sync-template`) per the
`required-checks-audit` guard (ADR-0039 add-on).

## Decision

Add the **SOURCE side** of the sync in tau, using
**`BetaHuhn/repo-file-sync-action`** (SHA-pinned `@v1.21.1`):

- `.github/sync.yml` — declares the target repos (`tau-rs/cairn`,
  `tau-rs/cairn-ui`, `tau-rs/tau-ui`) and the canonical template surface
  (`.github/workflows/` minus `sync-template.yml`, `.github/actions/`,
  `deny.toml`, `lefthook.yml`; `justfile` pending its own landing).
- `.github/workflows/sync-template.yml` — runs the action on push-to-main of the
  template surface, plus a `workflow_dispatch` `dry_run` toggle (default true).
  The action copies file **bytes** into each target's own tree and opens a PR;
  drift becomes a VISIBLE open PR the target repo owns and merges.
- `scripts/verify-sync-config.py` — resolves the config against the tree as a
  local DRY_RUN stand-in and asserts the recursion guard; wired as a pre-flight
  step in the sync workflow.

The sync workflow (`sync-template.yml`) is **excluded** from its own synced set,
so a sibling never becomes a second sync source. Cross-repo writes use a
documented `REPO_FILE_SYNC_TOKEN` secret, never `GITHUB_TOKEN` or a hardcoded
token.

## Consequences

- **Pros:** "tau = source of truth" is now an executable artifact; CI drift
  surfaces as a reviewable PR within minutes of a template change. Composite
  actions stay byte-identical across repos. Constraint B is preserved — no
  runtime SPOF.
- **Negatives:** A UI repo (cairn-ui / tau-ui) receives a sync PR mirroring the
  full workflows directory, including Rust/tau-specific workflows it cannot run.
  This is intentional — the target owner triages per-PR; a future iteration may
  move to per-target file subsets (the action supports per-repo `files:` groups).
- **Obligations:** the `REPO_FILE_SYNC_TOKEN` secret must be configured before
  the first live sync (documented in `docs/dev-environment.md`); the sibling
  repos do their own copy work (out of scope here). Phase-2 optional follow-ups:
  a projen-style generator + `synth`-diff CI check, and cosign/SLSA provenance.

## Alternatives considered

- **Renovate config that also distributes shared files** — Renovate updates
  *dependencies* / SHA pins; it has no primitive to copy arbitrary files from a
  source repo into targets, and pin freshness is already handled by
  `.github/dependabot.yml`. Rejected: does not solve the file-distribution gap.
- **multi-gitter** — distributes files, but as an ad-hoc local CLI a human runs
  from a laptop. Rejected: leaves no committed in-repo SOURCE artifact, so it
  does not operationalize "tau = source of truth."
- **Central reusable workflow invoked via runtime `workflow_call` + moving tag** —
  rejected by the model: blast radius (one bad push reds all four repos) and
  indirection (a repo's CI can't be read in isolation). Violates constraint B.

## Links

- Plan: `docs/superpowers/plans/2026-06-11-devops-antidrift-sync-template.md`
- Audit: `audit/devops.md` §3 (anti-drift "B+C"), §4 (sync-template item), Diagram 1
- Predecessor: ADR-0039 (CI strategy three-tier model; `required-checks-audit` guard)
