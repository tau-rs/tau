# Tier 0 — the developer loop. Everything here is seconds, not minutes.
#
# Rule: any test slower than 5s is not a unit test. Move it to an integration
# suite so `just check` stays fast enough to run on every save.

default: check

# The pre-commit gate. Mirrors Tier 1's blocking jobs, minus the slow ones.
check: fmt-check lint test-quick

# Format everything.
fmt:
    cargo fmt --all
    cargo fmt --manifest-path fuzz/Cargo.toml --all

fmt-check:
    cargo fmt --all --check
    cargo fmt --manifest-path fuzz/Cargo.toml --all --check

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Unit tests only, with a 5s per-test ceiling enforced by .config/nextest.toml.
test-quick:
    cargo nextest run --workspace --all-features --profile quick

# Everything Tier 1 runs, locally. Slower; use before opening a pull request.
test:
    cargo nextest run --workspace --all-features --profile ci
    cargo test --workspace --all-features --doc

# The ABI's wire format and the invariants behind it.
abi:
    cargo test -p tau-kernel --all-features --test abi_snapshot --test abi_invariants

# Review pending snapshot changes. A snapshot diff means the frozen surface
# moved: read it, do not reflexively accept it.
abi-review:
    cargo insta review

deny:
    cargo deny check advisories licenses bans sources
    cargo deny --manifest-path fuzz/Cargo.toml check --config deny.toml licenses bans sources

# Fuzz one target for `secs` seconds (Tier 2 item 3). Needs nightly and
# cargo-fuzz; the seed corpus is read, and what libFuzzer grows lands in
# fuzz/corpus/<target>, which is not committed. Targets: fuzz/fuzz_targets/.
fuzz target secs="60":
    #!/usr/bin/env bash
    set -euo pipefail
    cd fuzz
    mkdir -p "corpus/{{target}}"
    cargo +nightly fuzz run "{{target}}" "corpus/{{target}}" "seeds/{{target}}" -- -max_total_time={{secs}}

# The Tier 1 coverage ratchet, locally: line coverage against the merge base
# with origin/main may not drop more than 0.5 points. Same script as CI, so
# the numbers match. Override the base with BASE_SHA=<sha> just coverage.
coverage:
    #!/usr/bin/env bash
    set -euo pipefail
    export BASE_SHA="${BASE_SHA:-$(git merge-base origin/main HEAD)}"
    exec ./scripts/coverage-ratchet.sh

# Re-run the Tier 0 gate on every save.
watch:
    cargo watch -s 'just check'

# Install the pre-commit hook. Optional, never mandatory-slow: a hook that
# takes a minute is a hook people disable.
hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .git/hooks
    printf '#!/usr/bin/env bash\nexec just check\n' > .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    echo "installed .git/hooks/pre-commit -> just check"
