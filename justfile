# tau workspace task runner — canonical verbs shared with CI and lefthook so
# "local == CI" and the same muscle memory works across the sibling repos.
#
# Each recipe carries ONLY the cargo command string (identical to the matching
# CI job). The execution environment is supplied by the CALLER:
#   - CI            sets CARGO_INCREMENTAL=0 at the workflow `env:` level.
#   - lefthook      sets CARGO_INCREMENTAL=0 / CARGO_TARGET_DIR per command.
#   - agents (this  per CLAUDE.md "CARGO RULES", prefix with an isolated dir:
#     workspace)      env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main just test
# `just` passes the inherited environment through to recipe shells, so the
# executed cargo invocation + env is byte-equivalent to running the command
# directly. Do NOT bake CARGO_TARGET_DIR into a recipe — it would clobber
# lefthook's per-command target dirs and agents' isolated dirs.

# List the available recipes (default when `just` is run with no arguments).
default:
    @just --list

# Format check — mirrors the `rustfmt` CI job.
fmt:
    cargo fmt --all -- --check

# Lint — mirrors the `clippy` CI job.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Extra args are forwarded to nextest so callers can append flags — lefthook
# appends `--target-dir target/lefthook/test` for its per-command isolation.

# Test — mirrors the `test-stable` CI job.
test *args:
    cargo nextest run --profile ci --workspace --all-targets {{args}}

# `--all-features` is a GLOBAL flag (before the subcommand) in cargo-deny 0.14+,
# and the cargo-deny-action passes it that way too (arguments → command), so this
# is byte-for-byte what CI runs: `cargo-deny --all-features check`.

# Dependency / license / advisory audit — mirrors the `cargo-deny` CI job.
deny:
    cargo deny --all-features check

# Full local gate: everything a PR must pass. Same set the CI fast tier runs.
ci: fmt lint test deny

# Auto-fix: apply rustfmt + machine-applicable clippy suggestions in place.
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged

# Approximates the heavy CI tier (`tier2.yml` / the lefthook deep-gate) for a
# local pre-flight — the image build + e2e suites, not the full conformance /
# layer4 matrix. WRAPS xtask — never reimplements image logic. Needs
# podman/docker on PATH.

# Heavy tier — build per-plugin images via xtask, then run the e2e suites.
heavy:
    cargo run -p xtask -- build-plugin-images
    cargo nextest run --profile ci -p tau-runtime-tokio    --features integration-tests --tests
    cargo nextest run --profile ci -p tau-sandbox-native   --features integration-tests --tests
    cargo nextest run --profile ci -p tau-plugin-compat    --features integration-tests --tests
