# ADR-0039: CI Strategy — Three-Tier Model

**Status:** Accepted (superseded in part by ADR-0063 — per-PR Tier 1 job set)
**Date:** 2026-06-09
**Supersedes:** —

## Context

Pre-2026-06-09 CI ran every check on every PR (~24 jobs, ~15-25 min wall-clock). This was comprehensive but slow + costly. Pain points included recurring macOS infra flakes (chat_ephemeral_writes_no_file, echo-tool fixture race), linker bus errors on Linux, and CI-only clippy lints catching things local rustc missed.

## Decision

Adopt a three-tier CI model plus a periodic security-scan layer:

| Tier | Trigger | Required to merge? |
|---|---|---|
| 1 — PR (fast loop) | every push to PR branch + merge-queue + push to main | YES (via ci-summary aggregator) |
| 2 — Nightly + label (heavy validation) | `0 4 * * *` cron against main HEAD; PR `full-matrix` label opt-in | NO (informational; auto-opens issue on cron regression) |
| 3 — Release (tag-driven) | `push: tags: ['v*']` | YES (gates GitHub Release artifact creation) |

Plus security cron: daily `cargo audit` + `osv-scanner`, weekly Mon CodeQL, weekly Sun cargo-geiger.

Six DevOps add-ons: concurrency groups everywhere, flaky-test quarantine scaffold, action SHA pinning + dependabot bumps, dependabot patch auto-merge, required-checks audit guard, auto-bisect on nightly regression.

## Rationale

- Tier 1 keeps the cheap signal (fmt, clippy, deny, audit, osv, gitleaks, cargo-check × 3, nextest linux, doctests, msrv, fixtures-ports, feature-flag-matrix, no_std). The slow + flake-prone macOS + Windows nextest jobs moved to Tier 2.
- 24h regression detection via nightly cron beats waiting weeks for the next release tag.
- Release model is feature-driven (tag whenever a meaningful batch is ready), not time-driven (no Rust-style 6-week train). The 24h drift detection compensates for irregular release cadence.
- Security depth: `Strong` bundle (cargo-deny + audit + osv + gitleaks + CodeQL + cargo-geiger + SBOM at tag). Provides defense-in-depth without enterprise-grade ceremony.
- `full-matrix` PR label gives an escape hatch for high-risk PRs (sandbox, transports) that want pre-merge cross-platform confirmation without losing the fast PR loop default.

## Consequences

- **Pros**: ~5-7 min PR turnaround (warm) vs ~15-25 min. Nightly drift detection. Signed release artifacts with SBOM. Security findings flow into GitHub Security tab automatically.
- **Cons**: Cross-platform regressions slip onto main and surface ≤24h later. Mitigations: cargo-check on macOS + Windows still runs on PR; `full-matrix` label opts into Tier 2 pre-merge.
- Required to merge: only `ci-summary` (unchanged). Tier 2 / 3 are gated by their own workflow success.

## Alternatives considered

- **(A) Every-main-commit Tier 2** — would catch regressions within minutes of merge but multiplies CI cost. Rejected: nightly is sufficient for tau's commit volume.
- **(B) Time-driven release (Rust 6-week train)** — predictable cadence but forces ship-or-skip decisions on artificial dates. Rejected: feature-driven serves a workspace repo better.
- **(C) Tag-only validation (no nightly)** — simpler but late detection. Rejected: 24h detection is worth the cron infra.

## Links

- Spec: `docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md`
- Plan: `docs/superpowers/plans/2026-06-09-ci-strategy-redesign.md`
- Predecessor CI documentation: (none formal; this is the first ADR for CI strategy)
