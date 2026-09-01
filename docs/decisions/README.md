# Architecture Decision Records (ADRs)

This directory holds tau's ADRs. Each ADR is a numbered Markdown file
(`NNNN-title.md`) recording one decision in MADR style.

## When an ADR is required

Per QG18, ADRs are required for:

- Changes to project guidelines (anything in `CONSTITUTION.md`).
- Additions to or breaking changes in public APIs (`tau-runtime` exports,
  serve-mode IPC schema).
- Changes to the serve-mode protocol.
- Changes to the package manifest format.
- Changes to plugin trait boundaries.

Other changes (bugfixes, refactors within a crate, docs updates) do not
require ADRs and are recorded in commit messages and PR discussion (PG3).

## Known numbering collisions

Three ADR numbers were assigned twice by parallel sessions before this
note existed. The files are **kept as-is** — renumbering would break
merged cross-references in specs, plans, and commit messages. Cite these
by number **and title**:

- **0022** — [`0022-sandbox-darwin.md`](0022-sandbox-darwin.md) *and*
  [`0022-tau-workflow.md`](0022-tau-workflow.md) (the latter superseded,
  2026-09-01)
- **0028** — [`0028-docs-deployment.md`](0028-docs-deployment.md) *and*
  [`0028-skills-runtime-invocation.md`](0028-skills-runtime-invocation.md)
- **0044** — [`0044-deliverables-and-goals.md`](0044-deliverables-and-goals.md)
  *and* [`0044-trigger-ingress-slice-1.md`](0044-trigger-ingress-slice-1.md)

To avoid new collisions: before picking a number, check both the
directory listing **and** open PRs touching `docs/decisions/`
(`gh pr list --search "docs/decisions"`).

## Filing an ADR

1. Copy [`template.md`](template.md) to `NNNN-<short-title>.md` where
   `NNNN` is one greater than the highest existing ADR number.
2. Fill in Context, Decision, Consequences, and Alternatives.
3. Open a PR. The ADR's status starts as **Proposed**.
4. The maintainer reviews. On acceptance, status changes to **Accepted**
   and the PR is merged.
5. Per the Constitution §4 amendment process, guideline-changing ADRs
   wait at least 24 hours between draft and merge.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-bootstrap.md) | Bootstrap decisions | Accepted |
| [0002](0002-manifest-format.md) | Manifest format, capability evolution, escape-hatch policy | Accepted |
| [0003](0003-tau-ports.md) | tau-ports trait surface | Accepted |
| [0004](0004-tau-pkg.md) | tau-pkg package manager — public API, storage layout, lockfile | Accepted |
| [0005](0005-package-source-and-kind-serde.md) | Custom serde for PackageSource and PackageKind | Accepted |
| [0006](0006-tau-runtime.md) | tau-runtime kernel + Tool capabilities amendment | Accepted |
| [0007](0007-tau-cli.md) | tau-cli + tau-runtime amendments (capability filter, run_with_history) | Accepted |
| [0008](0008-plugin-loading.md) | Plugin loading mechanism — IPC over MessagePack-RPC + tau-pkg, tau-runtime, tau-domain amendments | Accepted |
| [0033](0033-tau-serve-mode.md) | Tau serve mode v1 — JSON-RPC 2.0 over NDJSON-framed stdio | Accepted |
| [0056](0056-contract-versioning-stability-surface.md) | The two contracts are the semver stability surface | Accepted |
| [0071](0071-three-surface-split.md) | Three-surface split — TOML vocabulary, TS choreography, Rust muscle | Accepted |
| [0072](0072-synth-contract.md) | The synth contract — subprocess synthesis emitting ProjectConfig JSON | Accepted |
| [0073](0073-ir-v3-multi-pipeline.md) | IR v3 — multi-pipeline modules and pipeline imports | Accepted |
| [0074](0074-journal-record-substrate.md) | The journal — one event-sourced record substrate | Accepted |
| [0075](0075-ops-lane-local-first.md) | The ops lane — env `local`, pins, plan/apply, run-or-refuse | Accepted |
| [0076](0076-agentic-instruction-set.md) | The agentic instruction set — kernel, taxonomy, extension rules | Accepted |
| [0077](0077-agent-exposure-surfaces.md) | Agent-exposure surfaces — emitters, MCP facade plan, agent-grade CLI | Accepted |
