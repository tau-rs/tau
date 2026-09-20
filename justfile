# Tier 0 — the developer loop. Everything here is seconds, not minutes.
#
# Rule: any test slower than 5s is not a unit test. Move it to an integration
# suite so `just check` stays fast enough to run on every save.

default: check

# The pre-commit gate. Mirrors Tier 1's blocking jobs, minus the slow ones.
check: fmt-check lint lock-check test-quick

# Format everything.
fmt:
    cargo fmt --all
    cargo fmt --manifest-path fuzz/Cargo.toml --all

fmt-check:
    cargo fmt --all --check
    cargo fmt --manifest-path fuzz/Cargo.toml --all --check

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Every Cargo.lock still satisfies its manifests — Tier 1's `lockfiles` job.
# Mostly about fuzz/, which is outside the workspace and so is not re-locked
# when a workspace dependency moves; the fuzz crate depends on the kernel by
# path, so a kernel bump changes what its lockfile must hold (issue #171).
# Offline as long as both locks are current: cargo only reaches for the index
# when it would have to re-resolve, which is the failure this catches.
lock-check:
    cargo metadata --locked --format-version 1 > /dev/null
    cargo metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1 > /dev/null

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

# Provider cassettes. `just live` replays; `just live record [anthropic|openai|ollama|probes]`
# re-records through the relay with keys from the macOS Keychain
# (`security find-generic-password`; store with `security add-generic-password -U -a "$USER"
# -s ANTHROPIC_API_KEY -w "$(pbpaste)"`); `just live render` re-renders
# drivers/tests/cassettes/MODELS.md from the cassettes already on disk, no
# provider call; `just live e2e [anthropic|openai|ollama]` runs the tool-loop
# programs of drivers/tests/e2e_live.rs against the real providers, nothing
# written. Costs money, capped by TAU_RECORD_CAP_MICROUSD (default three
# dollars); see drivers/tests/cassettes/MODELS.md.
live mode="replay" target="all":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{mode}}" = "replay" ]; then
      echo "replaying committed cassettes; re-record with: just live record [anthropic|openai|ollama|probes]"
      exec cargo nextest run -p tau-drivers --all-features --profile quick \
        --test cassette_guard --test cassettes_anthropic --test cassettes_openai --test probes
    fi
    if [ "{{mode}}" = "render" ]; then
      cargo test -p tau-drivers --all-features --test probes -- --ignored rewrite_models_md_from_disk
      echo "re-rendered drivers/tests/cassettes/MODELS.md"
      exit 0
    fi
    [[ "{{mode}}" == record || "{{mode}}" == e2e ]] || { echo "mode is replay, record, render or e2e"; exit 2; }
    key() { security find-generic-password -s "$1" -w 2>/dev/null || { echo "no Keychain entry $1" >&2; exit 2; }; }
    t="{{target}}"
    if [ "{{mode}}" = "e2e" ]; then
      [[ "$t" == all || "$t" == anthropic || "$t" == openai || "$t" == ollama ]] || { echo "target is all|anthropic|openai|ollama"; exit 2; }
      e2e() { cargo test -p tau-drivers --all-features --test e2e_live -- --ignored --nocapture "$1"; }
      if [[ "$t" == all || "$t" == anthropic ]]; then
        a="$(key ANTHROPIC_API_KEY)"
        TAU_ANTHROPIC_LIVE=1 ANTHROPIC_API_KEY="$a" e2e anthropic_live
      fi
      if [[ "$t" == all || "$t" == openai ]]; then
        o="$(key OPENAI_API_KEY)"
        TAU_OPENAI_LIVE=1 OPENAI_API_KEY="$o" e2e openai_live
      fi
      if [[ "$t" == all || "$t" == ollama ]]; then
        TAU_OLLAMA_LIVE=1 e2e ollama_live
      fi
      exit 0
    fi
    # TAU_RECORD_ONLY=name[,name] in the environment records just those scenarios.
    export TAU_RECORD=1
    if [[ "$t" == all || "$t" == anthropic ]]; then
      a="$(key ANTHROPIC_API_KEY)"
      ANTHROPIC_API_KEY="$a" cargo test -p tau-drivers --all-features --test cassettes_anthropic -- --ignored --nocapture record_all
    fi
    if [[ "$t" == all || "$t" == openai ]]; then
      o="$(key OPENAI_API_KEY)"
      OPENAI_API_KEY="$o" cargo test -p tau-drivers --all-features --test cassettes_openai -- --ignored --nocapture record_openai
    fi
    if [[ "$t" == all || "$t" == ollama ]]; then
      cargo test -p tau-drivers --all-features --test cassettes_openai -- --ignored --nocapture record_ollama
    fi
    if [[ "$t" == all || "$t" == probes ]]; then
      a="$(key ANTHROPIC_API_KEY)"
      o="$(key OPENAI_API_KEY)"
      ANTHROPIC_API_KEY="$a" OPENAI_API_KEY="$o" cargo test -p tau-drivers --all-features --test probes -- --ignored --nocapture record_probes
    fi
    if [[ "$t" != all && "$t" != anthropic && "$t" != openai && "$t" != ollama && "$t" != probes ]]; then
      echo "target is all|anthropic|openai|ollama|probes"
      exit 2
    fi
    echo; echo "re-recorded; drift against the committed cassettes:"
    git diff --stat -- drivers/tests/cassettes

# Install the pre-commit hook. Optional, never mandatory-slow: a hook that
# takes a minute is a hook people disable.
hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .git/hooks
    printf '#!/usr/bin/env bash\nexec just check\n' > .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    echo "installed .git/hooks/pre-commit -> just check"
