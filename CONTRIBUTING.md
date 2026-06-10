# Contributing to tau

Thanks for considering a contribution. Tau is run as a solo-maintainer
project today (see [`GOVERNANCE.md`](GOVERNANCE.md)), but external
contributions are welcome when they align with the constitution.

## Before you start

1. **Read [`CONSTITUTION.md`](CONSTITUTION.md).** It is the source of
   truth for what tau is, what tau is not, and how tau is built. The
   one-line summary in
   [`GUIDELINES_CHEATSHEET.md`](GUIDELINES_CHEATSHEET.md) is for
   reference once you have read the full text.

2. **Check the alignment bar.**
   - Phase 0–1: bug fixes and documentation PRs are welcome directly.
     Feature contributions need an issue discussion first (PG2).
   - Phase 2+: any aligned PR is welcome regardless of origin. LLM-
     generated PRs face the same alignment bar as human-authored ones;
     provenance does not excuse misalignment.

3. **Check the non-goals.** Tau has 12 explicit non-goals
   ([`CONSTITUTION.md` §2](CONSTITUTION.md)). If your idea trips one,
   it does not belong in tau core — it might fit in a plugin or a
   downstream project.

## Workflow

1. Fork the repo and branch off `main`.
2. Make your change. Follow the rules in the next section.
3. Ensure CI passes locally (commands below).
4. Open a PR. Reference any related issue.
5. Wait for review. Per QG22, even maintainer PRs wait overnight before
   merge — fresh eyes catch what tired eyes miss.

## Code rules

- **Conventional Commits** (QG17). Format: `<type>(<scope>): <subject>`,
  followed by an optional body. Types: `feat`, `fix`, `docs`, `style`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Scope is the crate
  name (`tau-domain`, `tau-cli`, etc.) or empty for repo-wide changes.
  Body explains *why*, not just *what* (PG3).

- **Tests required** (QG5). Four mandatory layers:
  - Unit tests inline with code.
  - Integration tests in `tests/` per crate.
  - Doc tests on every public API item.
  - CLI behavioral tests via `assert_cmd` for `tau-cli`.
  Plus property tests (`proptest`) for parsers of external input
  (manifest, IPC messages, config), and fuzz targets for the IPC
  protocol once it lands.

- **Docs required** (QG9). `#![deny(missing_docs)]` is enforced on
  every library crate. Every public item gets at least one rustdoc
  example.

- **No `.unwrap()` / `.expect()` / `panic!()` in library code** (QG3).
  Propagate errors with `thiserror`-typed errors. The binary
  (`tau-cli`) may use `anyhow` and may panic.

- **No `unsafe`** without an ADR (QG4).

- **No new dependency without justification in the PR description**
  (QG25): why this crate, why not std, what is the license, how
  actively maintained.

- **No silent tech debt** (QG24). If your change introduces something
  that needs follow-up, file an issue tagged `tech-debt` and link it
  from the PR.

## Local checks

Before opening a PR, run the same commands CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --all-targets
cargo test --workspace --doc
```

All five exit `0` on success.

## Running container-sandbox tests locally

The `tau-plugin-compat` integration tests under
`tests/layer4_container.rs` require per-plugin Docker images to be built
locally. Run once after pulling, and again whenever you edit a plugin's
source:

```bash
cargo xtask build-plugin-images          # all 5 plugins
cargo xtask build-plugin-images --name shell-plugin   # just one
```

Tests skip gracefully (with a hint message) if the relevant image is not
present.

The build auto-detects the container runtime — Podman first, Docker
fallback. On Apple Silicon, install Podman via `brew install podman` and
start the VM with `podman machine init && podman machine start`.

## PR labels

The CI strategy (ADR-0039) recognizes the following PR labels:

- **`full-matrix`** — opts the PR into Tier 2 heavy validation pre-merge (macOS + Windows nextest, coverage, plugin-compat matrices, sandbox-e2e, runtime-e2e). Use this for PRs touching:
  - Sandbox layer (`tau-sandbox-*` crates)
  - Transports (`tau-mcp-tokio::transport_*`)
  - Anything platform-specific (path handling, process spawn, etc.)
  - Anything cross-platform-test-relevant

  Tier 2 results post as a comment from `tau-ci-bot`. **Non-blocking** — auto-merge still fires on Tier 1 green even if Tier 2 surfaces an issue. Use the label as an informational signal.

- **`dependencies`** — dependabot adds this automatically to its PRs. The `dependabot-auto-merge.yml` workflow enables auto-merge for patch-level updates carrying this label.

- **`nightly-regression`** — applied by `tier2.yml`'s `nightly-regression-handler` when a cron run on main fails. Tracks rolling issues.

- **`security`** — applied to issues filed by `security-daily.yml` / `codeql.yml` / `cargo-geiger.yml` when new findings appear.

## Filing an ADR

If your change touches anything in the QG18 list — guidelines, public
APIs, the serve-mode protocol, the package manifest format, plugin
trait boundaries — your PR must include an ADR. Copy
[`docs/decisions/template.md`](docs/decisions/template.md) and number
sequentially. Per Constitution §4, guideline-changing ADRs wait at
least 24 hours between draft and merge.

## Working with escape hatches

tau core uses two structural escape hatches (`Custom { ... }` variants
on enums, plus the singleton `FailureKind::InternalError`) to leave room
for unknown shapes that haven't been typed yet. Every escape hatch is
tracked in `docs/explanation/escape-hatches.md` with a rationale and a
promotion trigger.

If your PR adds, promotes (replaces with a typed variant), or removes
an escape hatch, you must update `docs/explanation/escape-hatches.md`
in the same commit. The CI test
`crates/tau-domain/tests/escape_hatch_registry.rs` checks this
mechanically — every source variant named `Custom` or `InternalError`
must have a rustdoc link of the form `escape-hatches.md#<anchor>`,
and every active registry row must point at a live source variant.

If you're introducing a new escape hatch, name the rustdoc anchor and
add the row to the registry's "Active" table; copy the rustdoc
convention from existing variants.

## License

By contributing, you agree your contribution is dual-licensed under
MIT or Apache-2.0 at the project's option, per the Apache 2.0 §5
inbound=outbound norm.
