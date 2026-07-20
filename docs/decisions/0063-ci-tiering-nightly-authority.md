# ADR-0063: CI tiering — thin pre-merge gate, nightly authority for trunk health

**Status:** Accepted
**Date:** 2026-07-20
**Supersedes:** ADR-0039 (in part — the per-PR job set of Tier 1)

## Context

ADR-0039's three-tier model still ran the full 23-job matrix plus a
15–30 min standalone coverage workflow on every PR push, again in the
merge queue, and again on push to main — School-1 ("pre-merge
authority") cost, while the repo already owned School-2 ("post-merge
authority") machinery: nightly tier2 with auto-bisect,
auto-rerun-flaky, security-daily, a merge queue, and release preflight
that re-runs both tiers via workflow_call.

## Decision

Adopt trunk-based post-merge authority:

- **Tier 0 (pre-merge + merge queue + main push)** — `ci.yml` shrinks
  to 10 fast jobs: fmt, clippy, cargo-deny, gitleaks, cargo-check
  (macos + windows), test-stable, doc-tests, runtime-core-no-std,
  ports-semver. Target ≤ ~8 min wall clock. `ci-summary` remains the
  only required check.
- **Nightly (authority)** — the remaining ex-Tier-1 jobs move into
  `tier2.yml` (msrv-check, test-fixtures-ports, conformance,
  test-credential-chain, feature-flag-matrix, mock-sandbox-prod-gate,
  build-checks-linux, schema-conformance, wit-host-drift), covered by
  the nightly cron, the `full-matrix` PR label, and release preflight.
  The standalone coverage workflow is deleted; tier2's coverage job is
  the only coverage lane.
- **Contract** — a red nightly on main is a stop-the-line event: the
  first action next session is fix or revert. `auto-bisect` posts the
  culprit commit to the rolling regression issue.

Deliberate placements:

- `runtime-core-no-std` stays pre-merge: the feature-graph/no_std-link
  regression class fires often and hard-blocks the wasm epic.
- `ports-semver` stays pre-merge: its `--baseline-rev origin/main`
  compares main to itself on a nightly run and can never fail there —
  the check only has signal before merge.
- `cargo-audit`/`osv-scanner` are deleted from the PR path outright:
  advisory risk is time-triggered and `security-daily.yml` already
  runs both every day.

## Consequences

- PR feedback drops from ~23 jobs + coverage × (push + queue + main)
  to 10 fast jobs per event.
- Platform-specific *test* failures (macOS/Windows nextest) and the
  moved contract gates are now discovered nightly, not pre-merge;
  cargo-check on both platforms stays pre-merge to catch the
  compile-level cfg-root class. Risky PRs can opt back in with the
  `full-matrix` label.
- Releases are unaffected: `release.yml` preflight re-runs ci.yml and
  tier2.yml (now including the moved jobs) on the tag.
