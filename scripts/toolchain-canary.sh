#!/usr/bin/env bash
# Tier 3 item 4 (docs/HANDOFF.md §8, #206): the toolchain canary.
#
# Runs the Tier 1 build-and-test path on CHANNEL (beta or nightly, installed
# by the caller): clippy under `-D warnings`, the `ci` nextest profile,
# doctests. The pin in rust-toolchain.toml moves by hand and Dependabot has
# no ecosystem for it; this is what says the pin *should* move, or that the
# next stable will break us, six weeks before it is stable.
#
# A leg that fails is drift, not a failure of this script: the leg, the
# compiler's version and an excerpt go to DRIFT_OUT (tab-separated: leg,
# rustc version, excerpt file) and the workflow files an issue, because a
# compiler that moved is not the committer's fault. The legs stop at the
# first failure; the ones after it would mostly repeat it. New clippy lints
# only show up under clippy, which is why it is a leg and not left to
# RUSTFLAGS.
#
# Fails CLOSED:
#   * CHANNEL is not installed (the caller's job, and red is right);
#   * the suite passes with fewer than TESTS_FLOOR tests.
set -euo pipefail

CHANNEL="${CHANNEL:?set CHANNEL to the toolchain to run on (beta, nightly)}"
TESTS_FLOOR="${TESTS_FLOOR:-400}"
DRIFT_OUT="${DRIFT_OUT:-drift-toolchain.tsv}"
WORK="${WORK:-target/toolchain-canary}"

export RUSTUP_TOOLCHAIN="$CHANNEL"
if ! version="$(rustc --version 2>&1)"; then
  echo "::error title=toolchain canary::$CHANNEL is not installed: $version"
  exit 1
fi
echo "toolchain: $version"

rm -rf "$WORK"
mkdir -p "$WORK"
: > "$DRIFT_OUT"

# CARGO_TERM_COLOR=always wraps cargo's status words in escapes.
strip() { sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g'; }

run_leg() {
  local leg="$1"; shift
  local log="$WORK/$leg.log"
  local rc=0
  SECONDS=0
  "$@" 2>&1 | strip > "$log" || rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "ok       $leg  ${SECONDS}s"
    return 0
  fi
  # The first error and what follows it, for the issue; the whole log is
  # an artifact.
  local first
  first="$(grep -nE '^(error|thread .* panicked|[[:space:]]+FAIL )' "$log" | head -1 | cut -d: -f1)"
  if [ -n "$first" ]; then
    sed -n "${first},\$p" "$log" | head -60 > "$WORK/excerpt.txt"
  else
    tail -60 "$log" > "$WORK/excerpt.txt"
  fi
  printf '%s\t%s\t%s\n' "$leg" "$version" "$WORK/excerpt.txt" >> "$DRIFT_OUT"
  echo "::warning title=toolchain canary::$leg failed on $version"
  echo "DRIFT    $leg  $version  ${SECONDS}s  (exit $rc)"
  return 1
}

leg=""
tests=0
while :; do
  run_leg clippy cargo clippy --locked --workspace --all-targets --all-features -- -D warnings || break
  run_leg nextest cargo nextest run --locked --workspace --all-features --profile ci --no-fail-fast || break
  tests="$(sed -nE 's/^[[:space:]]+Summary \[[^]]+\] +([0-9]+) tests? run.*/\1/p' "$WORK/nextest.log" | tail -1)"
  if [ -z "$tests" ] || [ "$tests" -lt "$TESTS_FLOOR" ]; then
    echo "::error title=toolchain canary::the suite passed with ${tests:-0} test(s); the floor is $TESTS_FLOOR (an empty lane must not read green)"
    exit 1
  fi
  run_leg doctests cargo test --locked --workspace --all-features --doc || break
  break
done
if [ -s "$DRIFT_OUT" ]; then
  IFS=$'\t' read -r leg _ _ < "$DRIFT_OUT"
fi

echo "channel=$CHANNEL version=$version tests=$tests leg=${leg:-none}"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "version=$version"
    echo "tests=$tests"
    echo "leg=$leg"
  } >> "$GITHUB_OUTPUT"
fi
