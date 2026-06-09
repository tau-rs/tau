# CI Strategy Redesign — design spec

**Date:** 2026-06-09
**Worktree:** `feat/ci-strategy-redesign` off `origin/main` at `d98dff6`.
**Status:** Design approved in chat; ready for implementation plan via `superpowers:writing-plans`.

## Goal

Refactor tau's CI from "every job runs on every PR" to a tiered model: a fast PR loop that gates merge, a nightly cron + opt-in label that catches platform-specific and slow-running regressions on main within 24h, and a release tag tier that produces signed, attested, SBOM-bearing release artifacts. Add a periodic security-scan layer (RustSec, OSV, CodeQL, cargo-geiger) and DevOps best-practice add-ons (concurrency, flaky-test quarantine, auto-bisect, action SHA pinning, Dependabot patch auto-merge, required-checks audit).

## Motivation

Current CI is mature and comprehensive but runs everything on every PR. Wall-clock PR time today: ~15-25 min (parallel jobs; slowest dominate). The slowest jobs (`test-stable / macos`, `test-stable / windows`, `coverage`) account for the bulk of that latency and rarely catch bugs that the Linux nextest doesn't. The user wants:
1. Minimum testing on PR (target ~5-7 min warm).
2. Full validation on "release" — feature-driven git tags, not time-driven.
3. Automatic security problem detection running periodically.
4. Adopt other DevOps best practices currently missing.

Recent operational pain points (referenced from project memory) the redesign addresses:
- macOS infra flakes (`chat_ephemeral_writes_no_file`, `echo-tool` fixture pre-build race) — keeping macOS tests on PR didn't catch real bugs, only generated noise.
- Linker bus error during compilation on linux (one-off LLVM bug; would benefit from auto-rerun's existing infrastructure).
- CI rustc newer than local rustc surfacing lints CI-only (`clippy::unnecessary_map_or`) — Tier 1 keeps `clippy` on PR; this redesign doesn't change that.
- Coverage runs on every PR (30-min ceiling) but is "measurement, not gating" per its own workflow comment — natural fit for nightly.

## Non-goals

- Migrating away from GitHub Actions (the existing `setup-rust` custom action + sccache + mold stack is well-tuned).
- Adopting a self-hosted runner (mentioned as a future escape hatch if GHA minute caps bite; not built in this redesign).
- Replacing Dependabot with Renovate (Dependabot is fine for tau's needs).
- Crates.io publishing (tau is a workspace repo, not a published crate set; release tier ships GitHub Release artifacts only).
- Container image build + scan (tau doesn't ship container images today).
- CI telemetry / dashboards (premature for tau's scale).

## Architectural decisions

### Three-tier structure

| Tier | Trigger | Cadence | Required? | Purpose |
|---|---|---|---|---|
| **PR** | `pull_request`, `merge_group`, `push: branches: [main]` | every commit | YES (gates merge via `ci-summary`) | "does this compile + pass the cheap tests + no obvious security issues" |
| **Nightly** | `schedule: '0 4 * * *'` against main HEAD; `pull_request: types: [labeled, synchronize]` if `full-matrix` label; `workflow_dispatch` | nightly + on-demand | NO (auto-issue on regression) | "does main still work cross-platform + coverage didn't crater" |
| **Release** | `push: tags: ['v*']` | when a tag is pushed (feature-driven, not time-driven) | YES (gates release artifact creation) | "ship gate — full preflight + SBOM + signing + artifact attestation" |

Branch protection on `main` continues requiring only the `ci-summary` aggregator. `ci-summary` aggregates Tier 1 jobs only. Tier 2 results are informational; the release tier blocks itself by failing to produce the GitHub Release on any failure.

### Release model: feature-driven git tags

A "release" is a git tag `v*` pushed by a human when a meaningful batch of features is ready. No fixed cadence (in contrast to Rust's 6-week train). The nightly cron provides the 24h regression detection that Rust gets from its nightly toolchain — so regressions surface within a day even when releases are weeks apart.

### Security tooling depth: "Strong" bundle

`cargo-deny` (existing) + `gitleaks` (every PR) + `cargo audit` daily + `osv-scanner` daily + SBOM at tag + CodeQL nightly weekly + `cargo-geiger` weekly.

### PR opt-in via `full-matrix` label

A PR can opt into the Tier 2 job set pre-merge by adding the literal label `full-matrix`. Results post as a single comment from `tau-ci-bot`. Non-blocking — `ci-summary` does not wait on this; auto-merge still fires on Tier 1 green even if the label-triggered Tier 2 run fails. Failure is informational.

## Workflow file map

### Modified

| File | Change |
|---|---|
| `.github/workflows/ci.yml` | Remove the moved jobs (`test-stable / macos`, `test-stable / windows`, `test (conformance)`, `test (tau-plugin-compat / linux)`, `test (tau-plugin-compat / linux / layer4-ignored / native + container)`, `test (tau-sandbox-native e2e / linux)`, `test (tau-runtime e2e / linux)`). Add new jobs `cargo-check / macos`, `cargo-check / windows`, `cargo-audit`, `osv-scanner`, `gitleaks`. Simplify the `changes` job's `skip_heavy_jobs` flag since the heavy jobs no longer exist in this file. Concurrency unchanged. |
| `.github/workflows/ci-summary.yml` | Unchanged in scope (still polls `ci.yml` for CI run conclusion). Update its job-name allow-list to match Tier 1's new shape. |
| `.github/workflows/auto-rerun-flaky.yml` | Add Tier 2 job-name patterns to its flaky-pattern list (macOS chat-ephemeral, etc.). |
| `.github/workflows/auto-update-prs.yml` | Concurrency group added. Otherwise unchanged. |
| `.github/workflows/docs-check.yml` | Concurrency group added. Otherwise unchanged. |
| `.github/workflows/docs-deploy.yml` | Concurrency group added. Otherwise unchanged. |
| `.github/workflows/claude-review.yml` | Concurrency group added. Otherwise unchanged. |
| `.github/workflows/claude.yml` | Concurrency group added. Otherwise unchanged. |
| `.github/workflows/fuzz-nightly.yml` | Already on schedule; unchanged. |
| `.github/workflows/mutants-scheduled.yml` | Already on schedule; unchanged. |
| `.config/nextest.toml` | Add `[profile.ci.overrides]` block with quarantine list (initially empty); document the promotion pattern. |
| `CONTRIBUTING.md` | Add "PR labels" section documenting `full-matrix`. |
| `.github/dependabot.yml` | Add grouped patch-level update config; pair with auto-merge workflow. Also add `package-ecosystem: github-actions` for action version updates. |

### Created (NEW)

| File | Responsibility |
|---|---|
| `.github/workflows/tier2.yml` | Heavy validation matrix. Triggers: `schedule: '0 4 * * *'` against main, `pull_request: types: [labeled, synchronize]` gated on `full-matrix` label, `workflow_dispatch` for manual runs. Jobs: `nextest / macos`, `nextest / windows`, `coverage` (existing llvm-cov content), `test (conformance)`, `test (tau-plugin-compat)`, `test (tau-plugin-compat / layer4-ignored)` matrix, `test (tau-sandbox-native e2e)`, `test (tau-runtime e2e)`, `nightly-regression-handler` (opens / updates rolling issue on cron failure), `auto-bisect` (runs `git bisect` between yesterday's pass and today's fail on regression). |
| `.github/workflows/release.yml` | Release tier. Triggers ONLY on `push: tags: ['v*']`. Jobs: `release-preflight` (re-runs Tier 1 + Tier 2 against tag SHA), `sbom-rust` (cargo-sbom SPDX 2.3), `sbom-aggregate` (anchore/sbom-action syft), `build-release-artifacts` (linux/macos/windows), `attest-build-provenance` (GitHub OIDC; SLSA v1.0 provenance), `attest-sbom`, `changelog-gen` (git-cliff), `gh-release-create` (publishes Release with binaries + SBOMs + attestations + changelog). |
| `.github/workflows/security-daily.yml` | Daily cron security scans. Triggers: `schedule: '0 4 * * *'`. Jobs: `cargo-audit`, `osv-scanner`, both diffed against yesterday's cached result. Opens issue `[security] new CVE: <id>` (label `security`) on NEW finding only. Issue auto-closes when subsequent scan passes. |
| `.github/workflows/codeql.yml` | Weekly CodeQL static analysis. Triggers: `schedule: '0 6 * * 1'` (Mon 06:00 UTC). Posts findings as GitHub Code Scanning alerts; critical-severity alerts also open issue. |
| `.github/workflows/cargo-geiger.yml` | Weekly unsafe-code surface report. Triggers: `schedule: '0 6 * * 0'` (Sun 06:00 UTC). Generates per-crate unsafe count; diffs vs last main commit; opens issue `[security] unsafe surface grew by N` on increase. |
| `.github/workflows/full-matrix-label.yml` | Per-PR Tier 2 dispatcher. Triggers: `pull_request: types: [labeled, synchronize]`. Gates on `contains(github.event.pull_request.labels.*.name, 'full-matrix')`. Calls `tier2.yml` (via `workflow_call`) against PR HEAD SHA. Posts results as a single PR comment from `tau-ci-bot`. |
| `.github/workflows/required-checks-audit.yml` | Trivial guard. Triggers on `pull_request` modifying `.github/branch-protection.json` (if checked in) or any workflow file that touches `name:` fields. Fails if a new "required" check is added without ADR reference in the PR body. |
| `.github/dependabot-auto-merge.yml` | Auto-merges PRs from `dependabot[bot]` where the PR is patch-level + labeled `dependencies` + all Tier 1 checks pass. Triggers on `pull_request: types: [labeled, opened, synchronize]`. |
| `docs/decisions/ADR-XXXX-ci-strategy.md` | ADR documenting the three-tier model, the security cron layer, the "what fires when" matrix, and rationale for each. ADR number assigned at write time. |

### Deleted

| File | Reason |
|---|---|
| `.github/workflows/coverage.yml` | Content moved into `tier2.yml`. Coverage is "measurement, not gating" per its own comment — natural fit for nightly. |

## Job-by-job specification

### Tier 1 — PR (in `ci.yml`)

| Job | Tool | Triggers | Branch protection |
|---|---|---|---|
| `fmt` | `cargo fmt --all -- --check` | always | required (via ci-summary) |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | always | required |
| `cargo-deny` | `EmbarkStudios/cargo-deny-action@v2` (advisories + licenses + bans + sources) | always | required |
| `cargo-audit` | `rustsec/audit-check` action | always (~5s) | required |
| `osv-scanner` | `google/osv-scanner-action` | always (~10s) | required |
| `gitleaks` | `gitleaks/gitleaks-action` | always (~5s) | required |
| `cargo-check / linux` | `cargo check --workspace --all-targets` | always | required |
| `cargo-check / macos` | `cargo check --workspace --all-targets` | always (compile-only; catches API-level platform breakage) | required |
| `cargo-check / windows` | `cargo check --workspace --all-targets` | always (same) | required |
| `nextest / linux` | `cargo nextest run --profile ci --workspace --all-targets` | always | required |
| `doc-tests / linux` | `cargo test --workspace --doc` | always | required |
| `msrv-check / linux` | `cargo check --workspace --all-targets --locked` on rustc 1.91 | always | required |
| `test-fixtures-ports / linux` | existing `ci.yml` job | always | required |
| `feature-flag-matrix / linux` | existing per-crate `--no-default-features` check | always | required |
| `runtime-core no_std smoke` | existing | always | required |
| `build-fixtures / linux` | existing | gated on `changes.skip_heavy_jobs != true` (unchanged) | required when run |
| `build-checks / linux` | existing | always | required |
| `ci-summary` | unchanged aggregator | always | **the one branch-protection-required check** |

Expected wall-clock: ~5-7 min warm, ~12 min cold.

### Tier 2 — Nightly + label (in `tier2.yml`)

| Job | Tool | Notes |
|---|---|---|
| `nextest / macos` | `cargo nextest run --profile ci --workspace --all-targets` | runs on `macos-latest` |
| `nextest / windows` | same on `windows-latest` | |
| `coverage` | `cargo llvm-cov nextest --workspace` + Codecov upload | existing `coverage.yml` content |
| `test (conformance)` | `cargo nextest run -p anthropic -p ollama -p openai --test conformance --no-capture` | consumes fixture binaries built by `tier2.yml`'s own `build-fixtures` job (see Build-fixtures dependency below) |
| `test (tau-plugin-compat)` | existing — docker buildx + per-plugin images + integration-tests feature | |
| `test (tau-plugin-compat / layer4-ignored / native)` | existing — `--run-ignored only --test layer4_native` | |
| `test (tau-plugin-compat / layer4-ignored / container)` | existing — `--run-ignored only --test layer4_container` | |
| `test (tau-sandbox-native e2e)` | existing | |
| `test (tau-runtime e2e)` | existing | |
| `nightly-regression-handler` | runs `if: failure() && github.event_name == 'schedule'` | opens / updates a single rolling issue with label `nightly-regression`; one issue per regression streak; auto-closes via bot when subsequent nightly passes |
| `auto-bisect` | runs `if: failure() && github.event_name == 'schedule'` | `git bisect run` between yesterday's passing main SHA and today's failing SHA; caps at 7 days back; posts offending commit + author to the issue opened by `nightly-regression-handler` |

**Build-fixtures dependency**: Tier 2 jobs consuming the fixture binaries (`test (conformance)`, plugin-compat × 2, sandbox-e2e, runtime-e2e) run `build-fixtures / linux` inside `tier2.yml` itself rather than downloading the Tier 1 artifact. Self-contained; ~5 min cold; avoids cross-workflow artifact race conditions.

**Label-triggered runs** (via `full-matrix-label.yml`):
- Same job set as nightly, checked out against PR HEAD SHA.
- Results post as a single PR comment from `tau-ci-bot` identity with the matrix outcome table (job × status).
- Trigger gate: `contains(github.event.pull_request.labels.*.name, 'full-matrix')`.
- Non-blocking — failure does NOT block auto-merge.

Expected wall-clock: ~25-30 min (windows nextest cold dominates).

### Tier 3 — Release (in `release.yml`)

| Job | Tool | Purpose |
|---|---|---|
| `release-preflight` | re-runs Tier 1 + Tier 2 job set against tag SHA | "everything green before we ship" — even if nightly was green yesterday, tag commit may be newer |
| `sbom-rust` | `cargo-sbom --output-format spdx-json-2.3` | Software Bill of Materials for Rust deps; uploaded as release asset |
| `sbom-aggregate` | `anchore/sbom-action@v0` (syft) on repo root | Cross-language SBOM (covers any non-Rust artifacts) |
| `build-release-artifacts` | `cargo build --release -p tau-cli` × `{ubuntu-latest, macos-latest, windows-latest}` | Pre-built `tau` binaries per-platform |
| `attest-build-provenance` | `actions/attest-build-provenance@v3` (GitHub OIDC) | SLSA v1.0 provenance for each built binary |
| `attest-sbom` | `actions/attest-sbom@v3` | Signs SBOMs with the same OIDC identity |
| `changelog-gen` | `orhun/git-cliff-action@v3` | Generates release notes from conventional-commit subjects between previous tag and HEAD |
| `gh-release-create` | `softprops/action-gh-release@v3` | Publishes the GitHub Release; uploads binaries + SBOMs + attestations + changelog |

Any job failing aborts the release. The git tag stays in the repo (tags cannot be unpublished without force-push), but no GitHub Release is created — downstream consumers see nothing until a fixed re-tag (`v0.x.y+1`).

### Security cron layer (separate from tiers)

| Workflow | Schedule | Tool | Behavior on finding |
|---|---|---|---|
| `security-daily.yml` | `0 4 * * *` (after nightly Tier 2) | `cargo audit` + `osv-scanner` | Diffs result vs cached yesterday; opens `[security] new CVE: <id>` with label `security` on NEW finding only; existing CVEs don't re-fire. Yesterday's report cached as artifact (retention 30 days). |
| `codeql.yml` | `0 6 * * 1` (weekly Mon) | `github/codeql-action` `language: rust` | Posts findings as GitHub Code Scanning alerts; critical-severity alerts also open issue. |
| `cargo-geiger.yml` | `0 6 * * 0` (weekly Sun) | `cargo-geiger` | Generates per-crate unsafe count; uploads as artifact; diffs vs last main commit's report; opens `[security] unsafe surface grew by N` on increase. |

Permissions: all three need `issues: write` + `contents: read`. CodeQL additionally needs `security-events: write`. Scanner failures (tool crash) do NOT auto-open issues unless they fail 3 consecutive runs — then `[security-infra] scanner X failing`.

## DevOps add-ons

| Add-on | Location | Detail |
|---|---|---|
| **Concurrency groups everywhere** | All workflow files | `concurrency: { group: ${workflow}-${ref}, cancel-in-progress: ${{ github.ref != 'refs/heads/main' }} }`. Already in `ci.yml` + `coverage.yml`; extend to `tier2.yml`, `release.yml`, `security-daily.yml`, `codeql.yml`, `cargo-geiger.yml`, `full-matrix-label.yml`, `auto-update-prs.yml`, `docs-check.yml`, `docs-deploy.yml`, `claude-review.yml`, `claude.yml`. Cancel-on-force-push for feature branches; preserve in-progress on main (so cache-write completes). |
| **Flaky-test quarantine list** | `.config/nextest.toml` `[profile.ci.overrides]` block | Initially empty. Test paths listed there continue to run but failures are non-blocking. Pair with `auto-rerun-flaky.yml` improvement that promotes a test to quarantine after 5 flakes in a rolling 7-day window (separate follow-up; not in initial spec). Manual promotion via PR also supported. Document the pattern in `docs/how-to/quarantine-flaky-tests.md`. |
| **Required-checks audit** | `.github/workflows/required-checks-audit.yml` | Fails CI when a PR adds a new "required" check pattern without ADR reference. Trivial regex check on workflow diffs for new `name:` fields that would land in branch protection. |
| **Auto-bisect on nightly failure** | `tier2.yml` `auto-bisect` job | Runs `git bisect run` between yesterday's passing main SHA and today's failing SHA. Cap 7 days back. Posts offending commit + author to the `nightly-regression` issue opened in the same workflow run. Test command for bisect: re-run the failing job's exact nextest invocation. |
| **Dependabot patch auto-merge** | `.github/workflows/dependabot-auto-merge.yml` + `.github/dependabot.yml` | Auto-merges PRs from `dependabot[bot]` if (1) PR is labeled `dependencies` + patch-level (via dependabot's `update-type` metadata), (2) all Tier 1 checks pass, (3) PR title matches `chore(deps): bump …`. Groups patch-level updates daily. Adds `package-ecosystem: github-actions` to dependabot.yml. |
| **Action SHA pinning + Dependabot bumps** | Every workflow file | Replace `actions/checkout@v6` etc. with `actions/checkout@<full-40-char-SHA> # v6`. Configure `dependabot.yml` to bump these via `package-ecosystem: github-actions`. Supply-chain hygiene: a compromised tag doesn't auto-affect us; a pinned SHA does not change unless a maintainer reviews the bump PR. |
| **Self-hosted runner** (mentioned, NOT built) | `docs/decisions/ADR-XXXX-ci-strategy.md` | If GHA minute caps bite (currently ~$30/mo free; nightly + weekly heavy might push past), the escape hatch is a self-hosted Linux runner for the slow Linux jobs in Tier 2. Not built in this redesign. |

## Migration plan

Four phases, each its own PR — small, reviewable, revertable individually.

| Phase | Scope | Risk |
|---|---|---|
| **A — Add without replacing** | Ship the new files (`tier2.yml`, `release.yml`, `security-daily.yml`, `codeql.yml`, `cargo-geiger.yml`, `full-matrix-label.yml`, `required-checks-audit.yml`, `dependabot-auto-merge.yml`, ADR draft). New workflows trigger but their jobs are NOT yet wired into `ci-summary`. Existing CI keeps gating PRs unchanged. Watch new workflows run for ~3-5 days to confirm. | Low — adds without breaking |
| **B — Refactor `ci.yml`** | Edit `ci.yml` to remove the moved jobs. Delete `coverage.yml` (its content now in `tier2.yml`). Update `ci-summary.yml`'s job-name allow-list. Simplify `changes` job. Verify via a test PR before merging. | Medium — could break `ci-summary` if the allow-list slips |
| **C — Add the add-ons** | Concurrency groups across all workflows. Quarantine list scaffold in nextest config. Action-SHA pinning + Dependabot bumps. Dependabot patch auto-merge. Required-checks audit. Auto-bisect job in `tier2.yml`. | Low — additive |
| **D — Document + ADR finalize** | Finalize `docs/decisions/ADR-XXXX-ci-strategy.md`. Update CONTRIBUTING.md with `full-matrix` label section + quarantine pattern. Add `docs/how-to/quarantine-flaky-tests.md`. | Trivial |

Each phase opens its own PR. The total estimated effort (single human, focused) is ~1 week elapsed (5-10 hours of active work spread across phases).

## Locked open items (defaults from spec section 7)

| # | Item | Locked value |
|---|---|---|
| 1 | Nightly cron time | `0 4 * * *` (04:00 UTC) |
| 2 | `full-matrix` label name | literal `full-matrix` |
| 3 | Failure-rolling-issue labels | `nightly-regression` for tier 2 cron failures; `security` for security-daily / CodeQL / cargo-geiger findings |
| 4 | Auto-bisect commit-range bound | yesterday's passing main SHA to today's failing main SHA; cap 7 days back; if no green ancestor within cap, file issue without bisect |
| 5 | SBOM format | SPDX 2.3 |
| 6 | Release attestation signing | GitHub OIDC via `actions/attest-build-provenance@v3` (no bring-your-own cosign key) |
| 7 | `full-matrix` label failure behavior | non-blocking — posts comment with results; auto-merge still proceeds if Tier 1 is green |

## Required vs advisory check matrix

| Check class | Tier | Blocks merge? | Blocks release? |
|---|---|---|---|
| Tier 1 jobs (fmt, clippy, deny, audit, osv, gitleaks, cargo-check × 3, nextest linux, doctests, msrv, fixtures-ports, feature-flag-matrix, no_std, build-fixtures, build-checks) | 1 | YES (via ci-summary) | YES (via release-preflight) |
| `ci-summary` aggregator | 1 | YES (the literal branch-protection-required check) | YES |
| Tier 2 jobs on nightly cron | 2 | N/A (no PR) | N/A |
| Tier 2 jobs via `full-matrix` PR label | 2 | NO (informational) | N/A |
| Tier 3 release-preflight | 3 | N/A | YES |
| `release.yml` post-preflight steps (SBOM, signing, GitHub Release) | 3 | N/A | YES (release artifact not produced on failure) |
| Security-daily / CodeQL / cargo-geiger | security cron | NO | NO (informational; issues filed) |
| Mutation testing / fuzz nightly | scheduled | NO | NO (existing behavior; informational) |

## Out of scope (deferred / explicitly punted)

- Crates.io publishing (tau is not a published crate; reconsider if it changes).
- Container image build + scan (tau doesn't ship containers).
- Self-hosted runners (escape hatch documented; not built).
- Renovate as Dependabot alternative.
- CI telemetry dashboards.
- Replacement of the `auto-rerun-flaky.yml` cron with `workflow_run` triggers (existing cron pattern is fine; documented workaround for the workflow_run + PR branch limitation).
- Flaky-test auto-promotion to quarantine (manual promotion via PR works; auto-promotion based on rolling-window flake count is a follow-up).

## Definition of done

- [ ] All 9 modified workflow files updated per Workflow file map.
- [ ] All 7 new workflow files created and triggering correctly.
- [ ] `coverage.yml` deleted; its content lives in `tier2.yml`.
- [ ] `ci-summary.yml`'s allow-list matches Tier 1's new shape; branch protection still passes.
- [ ] `.config/nextest.toml` has quarantine scaffold.
- [ ] `.github/dependabot.yml` has grouped patch updates + `package-ecosystem: github-actions`.
- [ ] `CONTRIBUTING.md` documents `full-matrix` label.
- [ ] `docs/how-to/quarantine-flaky-tests.md` exists.
- [ ] `docs/decisions/ADR-XXXX-ci-strategy.md` written, committed, ADR number assigned.
- [ ] A test PR confirms: Tier 1 fires + ci-summary passes; `full-matrix` label triggers Tier 2; nightly schedule next morning runs against main.
- [ ] One full release-tag dry-run (against a `v0.0.0-ci-test` tag) confirms `release.yml` produces SBOM + attestation + GitHub Release.

## What's next

After this spec is approved, hand off to `superpowers:writing-plans` to produce the implementation plan that translates this into per-phase PR tasks.
