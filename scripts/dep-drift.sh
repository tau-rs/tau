#!/usr/bin/env bash
# Tier 3 item 3 (docs/HANDOFF.md §8, #205): the whole-lockfile canary.
#
# Moves every crate in Cargo.lock to the newest version its manifest range
# allows (`cargo update`, in the checkout, never pushed) and runs the Tier 1
# build-and-test path on the result: clippy, the `ci` nextest profile,
# doctests, cargo-deny. Dependabot sends one PR per bump; this answers the
# question those cannot: does everything-at-latest still pass Tier 1?
#
# A leg that fails is drift, not a failure of this script: the leg, the
# crate the first error points into, and an excerpt go to DRIFT_OUT
# (tab-separated: leg, crate, excerpt file) and the workflow files an issue,
# because a crate that moved is not the committer's fault. The legs stop at
# the first failure; the ones after it would mostly repeat it.
#
# Fails CLOSED:
#   * `cargo update` itself fails (the registry, the network);
#   * the suite passes with fewer than TESTS_FLOOR tests.
#
# This script rewrites Cargo.lock. Locally: `git checkout Cargo.lock` after.
set -euo pipefail

TESTS_FLOOR="${TESTS_FLOOR:-400}"
DRIFT_OUT="${DRIFT_OUT:-drift-deps.tsv}"
WORK="${WORK:-target/dep-drift}"

rm -rf "$WORK"
mkdir -p "$WORK"
: > "$DRIFT_OUT"

# CARGO_TERM_COLOR=always wraps cargo's status words in escapes.
strip() { sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g'; }

if ! cargo update 2>&1 | strip | tee "$WORK/update.txt"; then
  echo "::error title=dependency drift::cargo update failed; see $WORK/update.txt"
  exit 1
fi
# `Updating crates.io index` is not a crate: a crate line carries a version.
moved="$(grep -cE '^[[:space:]]*(Updating|Adding|Removing|Downgrading) [^ ]+ v[0-9]' "$WORK/update.txt" || true)"
echo "moved=$moved crate(s)"

# The crate a compiler error points into: the first registry source path in
# the log names it. A test failure names the test instead; anything else is
# reported by leg.
attribute() {
  local log="$1" leg="$2"
  local crate
  crate="$(grep -oE 'registry/src/[^/]+/[A-Za-z0-9_.-]+-[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*/' "$log" \
    | head -1 | sed -E 's#registry/src/[^/]+/##; s#-[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*/$##')"
  if [ -n "$crate" ]; then
    echo "$crate"
    return
  fi
  if [ "$leg" = nextest ]; then
    # `FAIL [ 0.1s] (110/589) binary test`; the counter is not part of the name.
    crate="$(grep -oE '^[[:space:]]+FAIL \[[^]]+\] (\([0-9]+/[0-9]+\) )?[^[:space:]]+ [^[:space:]]+' "$log" | head -1 \
      | sed -E 's/^[[:space:]]+FAIL \[[^]]+\] (\([0-9]+\/[0-9]+\) )?//')"
    [ -z "$crate" ] || { echo "test $crate"; return; }
  fi
  echo "$leg"
}

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
  local crate
  crate="$(attribute "$log" "$leg")"
  # The first error and what follows it, for the issue; the whole log is
  # an artifact.
  local first
  first="$(grep -nE '^(error|thread .* panicked|[[:space:]]+FAIL )' "$log" | head -1 | cut -d: -f1)"
  if [ -n "$first" ]; then
    sed -n "${first},\$p" "$log" | head -60 > "$WORK/excerpt.txt"
  else
    tail -60 "$log" > "$WORK/excerpt.txt"
  fi
  printf '%s\t%s\t%s\n' "$leg" "$crate" "$WORK/excerpt.txt" >> "$DRIFT_OUT"
  echo "::warning title=dependency drift::$leg failed after cargo update; first error points at $crate"
  echo "DRIFT    $leg  $crate  ${SECONDS}s  (exit $rc)"
  return 1
}

leg=""
crate=""
tests=0
while :; do
  run_leg clippy cargo clippy --workspace --all-targets --all-features -- -D warnings || break
  run_leg nextest cargo nextest run --workspace --all-features --profile ci --no-fail-fast || break
  tests="$(sed -nE 's/^[[:space:]]+Summary \[[^]]+\] +([0-9]+) tests? run.*/\1/p' "$WORK/nextest.log" | tail -1)"
  if [ -z "$tests" ] || [ "$tests" -lt "$TESTS_FLOOR" ]; then
    echo "::error title=dependency drift::the suite passed with ${tests:-0} test(s); the floor is $TESTS_FLOOR (an empty lane must not read green)"
    exit 1
  fi
  run_leg doctests cargo test --workspace --all-features --doc || break
  run_leg deny cargo deny check advisories licenses bans sources || break
  break
done
if [ -s "$DRIFT_OUT" ]; then
  IFS=$'\t' read -r leg crate _ < "$DRIFT_OUT"
fi

echo "moved=$moved tests=$tests leg=${leg:-none} crate=${crate:-none}"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "moved=$moved"
    echo "tests=$tests"
    echo "leg=$leg"
    echo "crate=$crate"
  } >> "$GITHUB_OUTPUT"
fi
