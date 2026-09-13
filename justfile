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

fmt-check:
    cargo fmt --all --check

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
