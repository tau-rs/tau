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
