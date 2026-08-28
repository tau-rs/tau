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

# Lint the guest's cfg(tau_cap_net_http) effect arm (#597) — mirrors the
# `clippy` CI job's wasm-guest-net step. The workspace `just lint` above always
# builds the guest against the empty-cap BASELINE world, so the effect arm
# compiles out and is never linted (it hid a real clippy::redundant_import +
# clippy::while_let_loop in #585). The CALLER must set `TAU_WORLD_WIT` to an
# ABSOLUTE path of a net.http-granting world (build.rs runs with CWD = the crate
# dir, so a relative path resolves wrong) — e.g.
#   TAU_WORLD_WIT="$PWD/crates/tau-wasm-guest/wit-cfg/net-http.wit" just lint-wasm-guest-net
# so build.rs fires cfg(tau_cap_net_http) and the arm is compiled + linted.
lint-wasm-guest-net:
    cargo clippy -p tau-wasm-guest --target wasm32-wasip2 --release -- -D warnings

# Lint tau-domain's two no_std shapes. The workspace `just lint` above unifies
# features across members, so some host member always turns on tau-domain's
# `std` and the feature-less configuration is NEVER linted — even though the
# workspace `tau-domain` alias sets `default-features = false`, which is exactly
# what tau-sandbox-proxy and tau-wasm-guest build against. That gap hid a
# deny-level `unused_imports` (alloc::borrow::ToOwned) + `dead_code`
# (VocabMode::forward_open), both declared ungated but used only from
# `#[cfg(feature = "serde")]` blocks. `--features serde` is the guest's actual
# configuration; bare `--no-default-features` is the floor.
#
# Deliberately NOT `--all-targets`: tau-domain's own cfg(test) modules exercise
# the host surface (`MessageId::new` needs uuid/std, `PackageSource::Url` needs
# `package-source`, `detect_format` needs `skill-md`), so the test targets do
# not compile without `std` at all. Downstream no_std consumers link the lib
# only, which is precisely what this gate covers.
lint-domain-featureless:
    cargo clippy -p tau-domain --no-default-features -- -D warnings
    cargo clippy -p tau-domain --no-default-features --features serde -- -D warnings

# Same hole, same crate set: `just lint` never sees tau-ports without `process`,
# even though the workspace alias is `default-features = false` and the guest
# links `--no-default-features --features serde`
# (crates/tau-wasm-guest/Cargo.toml:33). The third shape is the one that was
# actually broken: `test-fixtures` did not declare its dependency on `process`,
# so `--no-default-features --features test-fixtures` failed to compile the LIB
# (E0061 on `SessionContext::new`, E0560 on `WorkingContext.working_dir`, plus a
# deny-level `unused_imports`) — src/fixtures.rs builds both through
# `#[cfg(feature = "process")]` API.
#
# `--all-targets` (unlike tau-domain above, which stays lib-only): #657 gated
# tau-ports' in-src `#[cfg(test)]` code off the `process`-only API it was
# calling, so the test targets now build in every shape here. Note what this
# does and does not prove — tau-ports links std unconditionally under cfg(test)
# (`crates/tau-ports/src/lib.rs:22`), so green here means the test code compiles
# against the feature-less FEATURE SET, not that the test targets are
# no-std-clean. The lib is the artifact downstream no_std consumers link, and it
# is covered by the same three lines.
lint-ports-featureless:
    cargo clippy -p tau-ports --no-default-features --all-targets -- -D warnings
    cargo clippy -p tau-ports --no-default-features --features serde --all-targets -- -D warnings
    cargo clippy -p tau-ports --no-default-features --features test-fixtures --all-targets -- -D warnings

# Measure + gate the wasm-guest bundle size (EPIC 5.6) — mirrors the CI step in
# the `runtime-core-no-std` job. Reports the shipped-component size (wasm-tools)
# + the wasm-metadce tree-shaken floor (Binaryen, optional), and fails if the
# shipped size exceeds TAU_WASM_SIZE_BUDGET. The CALLER supplies CARGO_TARGET_DIR
# per the CARGO RULES. See docs/reference/browser-capabilities.md.
wasm-guest-size:
    scripts/wasm-guest-size.sh

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
