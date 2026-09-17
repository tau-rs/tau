#!/usr/bin/env bash
# Tier 3 job 1 (docs/HANDOFF.md §8): the determinism drift sentinel's fold.
#
# Refolds every fixture in corpus/ with `tau replay --expect` (the kernel's
# own reader, #91; ADR-0010 §6 names this the CLI's acceptance test) and
# compares each state hash to its committed `.hash` sidecar. Drift is not a
# failure of this script: a fixture whose hash moved is appended to DRIFT_OUT
# (tab-separated: name, expected, actual) and the workflow files it as an
# issue, because drift is not the committer's fault and a red build blames
# the wrong person. What *is* a failure here is anything that would let the
# job report green without having checked the corpus.
#
# Fails CLOSED:
#   * fewer than CORPUS_FLOOR fixtures (an empty or half-checked-out corpus);
#   * a log with no sidecar, an unreadable log, a fold that refuses an entry
#     — every exit of `tau replay` that is not the hash comparison itself
#     (exit 6; the others are documented in kernel/src/bin/tau.rs).
set -euo pipefail

CORPUS="${CORPUS:-corpus}"
CORPUS_FLOOR="${CORPUS_FLOOR:-10}"
DRIFT_OUT="${DRIFT_OUT:-drift.tsv}"
TAU="${TAU:-./target/release/tau}"

if [ ! -x "$TAU" ]; then
  echo "::error::$TAU is not an executable; build it first (cargo build --release -p tau-kernel --bin tau)"
  exit 1
fi

shopt -s nullglob
logs=("$CORPUS"/*.log)
count=${#logs[@]}
if [ "$count" -lt "$CORPUS_FLOOR" ]; then
  echo "::error::$CORPUS holds $count fixture(s); the floor is $CORPUS_FLOOR"
  exit 1
fi

: > "$DRIFT_OUT"
ok=0
tooling=0
for log in "${logs[@]}"; do
  name="$(basename "$log" .log)"
  sidecar="$CORPUS/$name.hash"
  if [ ! -f "$sidecar" ]; then
    echo "::error title=no sidecar::$log has no $sidecar"
    tooling=$((tooling + 1))
    continue
  fi
  if out="$("$TAU" replay "$log" --expect "$sidecar" 2>&1)"; then
    echo "ok       $name  ${out##*$'\n'}"
    ok=$((ok + 1))
    continue
  fi
  # `tau replay` exits 6 for exactly one reason we want: the hash moved. Its
  # message names both hashes. Any other exit is the tooling, and that is red.
  if [[ "$out" == *"hash mismatch across builds"* ]]; then
    expected="$(tr -d '[:space:]' < "$sidecar")"
    actual="${out##*folded }"
    actual="${actual//[[:space:]]/}"
    printf '%s\t%s\t%s\n' "$name" "$expected" "$actual" >> "$DRIFT_OUT"
    echo "::warning title=determinism drift::$name folded to $actual, sidecar says $expected"
    echo "DRIFT    $name  expected $expected  folded $actual"
  else
    # The last line is the refusal; the lines before it are the header report.
    echo "::error title=refold failed::$name: ${out##*$'\n'}"
    echo "TOOLING  $name"
    tooling=$((tooling + 1))
  fi
done

drifted="$(wc -l < "$DRIFT_OUT" | tr -d ' ')"
echo "fixtures=$count ok=$ok drifted=$drifted tooling=$tooling"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "fixtures=$count"
    echo "ok=$ok"
    echo "drifted=$drifted"
    echo "tooling=$tooling"
  } >> "$GITHUB_OUTPUT"
fi
[ "$tooling" -eq 0 ]
