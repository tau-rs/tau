# CI tiering redesign — thin pre-merge gate, nightly authority

**Date:** 2026-07-20
**Status:** Approved (design), pending implementation
**Supersedes:** the per-PR full-matrix portion of ADR-0039 (3-tier CI strategy). A new ADR will record this decision.

## Problem

Every change currently pays the full 23-job `ci.yml` matrix plus a 15–30 min
`coverage.yml` run **three times**: on PR push, again in the merge queue
(`merge_group`), and again on push to `main`. This is School-1
("pre-merge authority", Rust/bors-style) cost in a repo that already owns
School-2 ("post-merge authority", Google-TAP-style) machinery: nightly
`tier2.yml` with `auto-bisect`, `auto-rerun-flaky.yml`, `security-daily.yml`,
a merge queue, and `release.yml` preflight that re-runs everything via
`workflow_call`.

## Decision

Adopt trunk-based post-merge authority:

- **Tier 0 (pre-merge + merge queue + main push):** a thin fast gate,
  target ≤ ~8 min wall clock.
- **Nightly (authority for trunk health):** the full matrix, absorbed into
  `tier2.yml`, with auto-bisect finding culprit commits.
- **Contract:** a red nightly on `main` is a stop-the-line event — first
  action next session is fix or revert. Accepted by the maintainer
  2026-07-20.

## Tier 0 — jobs that stay in `ci.yml`

| Job | Rationale |
|---|---|
| `changes` | path-detection plumbing (docs-only skip) |
| `fmt`, `clippy` | warn==deny gate; a landed warning breaks every later PR |
| `test-stable` (nextest / linux) | core correctness signal |
| `doc-tests` | fast; doctest rot is per-PR |
| `cargo-check-macos`, `cargo-check-windows` | ~2–3 min; catches the cfg-root local-green/CI-red class |
| `gitleaks` | secrets in pushed history are unrecoverable |
| `cargo-deny` | ~1 min; bans/licenses are code-triggered (new dep in the PR) |
| `runtime-core-no-std` | borderline by cost, kept deliberately: this class (feature-graph / no_std link regressions) breaks often and hard-blocks the wasm epic; runs in parallel so it does not extend wall clock past `test-stable`. Revisit if it stops firing. |

Triggers on `ci.yml` are unchanged: `push: [main]` (kept for Swatinem
rust-cache warming — `save-if` only runs on main), `pull_request`,
`merge_group`, `workflow_call`. The required branch-protection check stays
`ci-summary`, which polls the whole CI workflow-run conclusion and is
insensitive to the job-set change.

## Moves out of the pre-merge path

| Job | Destination | Notes |
|---|---|---|
| `coverage.yml` (standalone workflow) | **deleted** | `tier2.yml` already has a `coverage` job (nightly + `full-matrix` label + release preflight). The standalone per-PR run was the duplicate. |
| `cargo-audit`, `osv-scanner` | **deleted from ci.yml** | already run daily in `security-daily.yml`; advisory risk is time-triggered, not code-triggered |
| `msrv-check` | tier2 | rare breakage; bisect finds it trivially. `release.yml` also has its own `msrv` job — verify no dedupe needed. |
| `feature-flag-matrix` | tier2 | slow combinatorial sweep |
| `test-fixtures-ports` | tier2 | feature-quadrant sweep |
| `test-credential-chain` | tier2 | scoped port tests, low churn |
| `conformance` (tau-conformance crate) | tier2 | distinct from tier2's existing `test-conformance` (plugin suites); lands as a new job |
| `schema-conformance` | tier2 | contract gate; release preflight re-runs it |
| `wit-host-drift` | tier2 | contract gate |
| `ports-semver` | tier2 | semver breaks are rare and caught by release preflight |
| `mock-sandbox-prod-gate` | tier2 | structural guard, low churn |
| `build-fixtures-linux`, `build-checks-linux` | tier2 | tier2 already has its own `build-fixtures-linux`; merge/dedupe rather than duplicate |

Jobs moved into `tier2.yml` are automatically covered by all three of its
existing consumers: the nightly cron, the `full-matrix` PR label (escape
hatch for risky PRs), and `release.yml`'s `preflight-tier2` — no extra
wiring for the release gate.

## What runs when (end state)

```
PR push ──────► ci.yml Tier 0 (10 jobs, ≤8 min) + ci-summary (required)
merge queue ──► same Tier 0 on candidate-merge SHA
main push ────► same Tier 0 (cache warming + fast trunk signal)
nightly 04:00 ► tier2.yml full matrix (now incl. moved jobs) + coverage
                 └─ red → auto-bisect → culprit → fix-or-revert next session
daily 04:00 ──► security-daily (audit, osv)          [unchanged]
weekly ───────► codeql, cargo-geiger, mutants        [unchanged]
tag/release ──► release.yml preflight = ci.yml + tier2.yml (full)  [unchanged]
on demand ────► `full-matrix` label pulls tier2 onto a PR          [unchanged]
```

## Error handling / failure modes

- **Nightly red:** `tier2.yml`'s `nightly-regression-handler` + `auto-bisect`
  identify the culprit commit. Implementation must verify the bisect job
  covers the newly moved jobs (it should be job-generic; confirm).
- **Flaky Tier 0:** `auto-rerun-flaky.yml` already retries; unchanged.
- **Docs-only PRs:** `paths-ignore` + ci-summary grace logic unchanged.
- **Job-name coupling:** `required-checks-audit.yml` and `ci-summary.yml`
  verified insensitive to the job-set change (summary polls run conclusion,
  not job names). Branch protection requires only `ci-summary` — no
  protection-settings change needed.
- **`needs:` graph:** removed ci.yml jobs may appear in other jobs'
  `needs:`; implementation must re-check the dependency graph after
  removal (notably `changes` consumers and `build-fixtures-linux`).

## Testing

- CI workflow changes are validated by the PR's own run: Tier 0 must go
  green on the PR that ships this.
- `workflow_dispatch` tier2 once after merge to prove the moved jobs run
  green in their new home before trusting the nightly.
- `gh pr checks` / required-checks-audit confirm `ci-summary` still
  resolves.
- Docs: new ADR page added to `docs/SUMMARY.md`; `mdbook build` locally
  before the PR (linkcheck is deploy-fatal).

## Out of scope

- No change to weekly jobs (codeql, geiger, mutants), `security-daily`,
  `fuzz-nightly`, `release.yml` structure, or branch protection.
- No "full CI only on version tags" — releases already re-run everything;
  correctness authority is the nightly, not the tag.
