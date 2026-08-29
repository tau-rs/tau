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

## Amendment (2026-08-29): the daily cadence is keeper-driven, not cron-driven

This ADR says the nightly authority lane is "covered by the nightly
cron". That premise no longer holds and the wording is now wrong.

From 2026-08-26 GitHub applied an escalating, repo-wide backoff to
`schedule` events in this repository (issue #736). Every cron here —
not just the `0 4 * * *` gates — converged on the same ~10-12h
effective period regardless of its declared expression:

| workflow | cron | expected/day | 08-22 | 08-25 | 08-26 | 08-27 | 08-28 |
|---|---|---|---|---|---|---|---|
| `auto-rerun-flaky` | `*/10` | 144 | 49 | 37 | 22 | 3 | 2 |
| `auto-update-prs` | `*/30` | 48 | 41 | 33 | 21 | 2 | 3 |
| `dependabot-auto-merge` | `*/30` | 48 | 31 | 23 | 16 | 3 | 2 |

Every drifted run has `created_at == run_started_at`, so this is late
*dispatch* by GitHub, not runner queueing on our side. Restaggering the
cron minute does not help: our own crons already span every minute of
the hour and are throttled identically.

What the backoff cost is **punctuality, not existence** — a throttled
cron still fires once or twice a day. So the daily cadence is now
driven by `daily-gate-keeper.yml`, which reacts to whichever trigger
fires first (`push` to main, its own best-effort cron, or a manual
dispatch), checks how long ago each gate last ran *against the default
branch*, and dispatches whatever is stale past 20h. Past 48h it fails
red, on the grounds that reaching that point means the repair path
itself is broken.

The `0 4 * * *` crons stay on all three gates as a best-effort second
path: when one does fire it satisfies the freshness check and nothing
is dispatched.

Consequence for this ADR's contract: "a red nightly on main is a
stop-the-line event" is unchanged, but a *missing* daily run is now
also a detectable event rather than a silent one — which is the failure
mode that let `Fuzz nightly` sit dead for months (#649). Note that
`tier2.yml`'s regression reporting keys off the daily run, so it must
recognise the keeper's dispatch as nightly (`inputs.nightly`) and not
`schedule` alone.
