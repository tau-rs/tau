#!/usr/bin/env bash
# Tier 3 item 5 (docs/HANDOFF.md §8, #151): the benchmark regression judge.
#
# Reads the JSON iai-callgrind prints for `kernel/benches/kpi_ir.rs` (one
# summary per benchmark, `--output-format=json`) and compares each
# benchmark's instruction count (`Ir`) with the committed baseline. A
# regression is not a failure of this script: a benchmark that grew past the
# limit is appended to REGRESSION_OUT (tab-separated: name, baseline, new,
# diff in percent) and the workflow files it as an issue, because drift is
# not the committer's fault and a red build blames the wrong person. What
# *is* a failure here is anything that would let the job report green
# without having judged the KPIs.
#
# Modes (MODE):
#   judge   compare with BASELINE (default);
#   record  write BASELINE from this run, judge nothing. Used once to pin
#           and again to re-pin after an intended change; the workflow
#           uploads the file as an artifact for a PR to commit.
#
# Fails CLOSED:
#   * fewer than KPI_FLOOR benchmarks in the input (a filtered or empty
#     run; the lane that measures nothing must not read green);
#   * a benchmark with no `Ir` (the tool ran, the number did not arrive);
#   * in judge mode, a measured benchmark missing from the baseline, or a
#     baseline row that was not measured (a renamed or dropped benchmark
#     is a judgement that never happened).
set -euo pipefail

INPUT="${1:-ir.json}"
MODE="${MODE:-judge}"
BASELINE="${BASELINE:-kernel/benches/kpi_ir.baseline.tsv}"
KPI_FLOOR="${KPI_FLOOR:-3}"
LIMIT_PCT="${LIMIT_PCT:-5}"
REGRESSION_OUT="${REGRESSION_OUT:-regressions.tsv}"

if [ ! -s "$INPUT" ]; then
  echo "::error::$INPUT is missing or empty; run the bench with --output-format=json first"
  exit 1
fi

# One row per benchmark: "<function>::<id>\t<Ir>". The runner reports the
# callgrind total as either the new value alone (`Left`) or new and old
# (`Both`, when target/iai holds a previous run); the new value is what we
# judge, never the runner's own comparison with whatever the cache held.
measured="$(jq -rs '
  .[]
  | (.function_name + "::" + (.id // "-")) as $name
  | [ .profiles[] | select(.tool == "Callgrind")
      | .summaries.total.summary.Callgrind.Ir.metrics
      | (.Left // .Both[0]) | .Int ] as $ir
  | $name + "\t" + (($ir[0] // "null") | tostring)
' "$INPUT" | sort)"

count="$(printf '%s\n' "$measured" | sed '/^$/d' | wc -l | tr -d ' ')"
if [ "$count" -lt "$KPI_FLOOR" ]; then
  echo "::error::$INPUT holds $count benchmark(s); the floor is $KPI_FLOOR"
  exit 1
fi

tooling=0
while IFS=$'\t' read -r name ir; do
  if ! [[ "$ir" =~ ^[0-9]+$ ]]; then
    echo "::error title=no Ir::$name reported no instruction count ($ir)"
    echo "TOOLING     $name  no Ir"
    tooling=$((tooling + 1))
  fi
done <<<"$measured"
[ "$tooling" -eq 0 ] || exit 1

if [ "$MODE" = "record" ]; then
  {
    echo "# Instruction counts (callgrind Ir) for kernel/benches/kpi_ir.rs, the"
    echo "# baseline scripts/bench-regression.sh judges the nightly against (#151)."
    echo "# Re-pin with: gh workflow run tier3.yml --ref main -f baseline=record,"
    echo "# then commit the kpi-ir-baseline artifact with the reason for the move."
    echo "# recorded: $(date -u '+%Y-%m-%dT%H:%M:%SZ') ref=${GITHUB_SHA:-$(git rev-parse HEAD)}"
    echo "# $(rustc --version) | $(valgrind --version 2>/dev/null || echo 'valgrind ?') | iai-callgrind-runner $(iai-callgrind-runner --version 2>/dev/null | awk '{print $NF}' || echo '?')"
    printf '%s\n' "$measured"
  } > "$BASELINE"
  echo "benchmarks=$count recorded=$BASELINE"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    {
      echo "benchmarks=$count"
      echo "regressed=0"
      echo "faster=0"
    } >> "$GITHUB_OUTPUT"
  fi
  exit 0
fi

[ "$MODE" = "judge" ] || { echo "::error::MODE is judge or record, not $MODE"; exit 1; }
if [ ! -f "$BASELINE" ]; then
  echo "::error::$BASELINE does not exist; record it first (MODE=record)"
  exit 1
fi
pinned="$(grep -v '^#' "$BASELINE" | sed '/^$/d' | sort)"

: > "$REGRESSION_OUT"
ok=0
regressed=0
faster=0
while IFS=$'\t' read -r name ir; do
  old="$(awk -F'\t' -v n="$name" '$1 == n { print $2 }' <<<"$pinned")"
  if ! [[ "$old" =~ ^[0-9]+$ ]]; then
    echo "::error title=no baseline::$name is not in $BASELINE; re-record the baseline"
    echo "TOOLING     $name  Ir=$ir  no baseline"
    tooling=$((tooling + 1))
    continue
  fi
  # Percent to one decimal, in awk: the shell has no floats and the
  # workspace's integer-division lint is a habit worth keeping in scripts.
  pct="$(awk -v n="$ir" -v o="$old" 'BEGIN { printf "%+.1f", (n - o) * 100 / o }')"
  if awk -v n="$ir" -v o="$old" -v l="$LIMIT_PCT" 'BEGIN { exit !((n - o) * 100 > l * o) }'; then
    printf '%s\t%s\t%s\t%s\n' "$name" "$old" "$ir" "$pct" >> "$REGRESSION_OUT"
    echo "::warning title=benchmark regression::$name runs $ir instructions, baseline $old ($pct%)"
    echo "REGRESSION  $name  Ir=$ir  baseline=$old  ($pct%)"
    regressed=$((regressed + 1))
  elif awk -v n="$ir" -v o="$old" -v l="$LIMIT_PCT" 'BEGIN { exit !((o - n) * 100 > l * o) }'; then
    echo "FASTER      $name  Ir=$ir  baseline=$old  ($pct%)  the baseline is stale; re-pin it"
    faster=$((faster + 1))
  else
    echo "ok          $name  Ir=$ir  baseline=$old  ($pct%)"
    ok=$((ok + 1))
  fi
done <<<"$measured"

# A pinned benchmark that was not measured is a judgement that never happened.
while IFS=$'\t' read -r name _; do
  if ! grep -q "^$name	" <<<"$measured"; then
    echo "::error title=not measured::$name is pinned in $BASELINE but was not measured"
    echo "TOOLING     $name  pinned, not measured"
    tooling=$((tooling + 1))
  fi
done <<<"$pinned"

echo "benchmarks=$count ok=$ok regressed=$regressed faster=$faster tooling=$tooling limit=${LIMIT_PCT}%"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "benchmarks=$count"
    echo "regressed=$regressed"
    echo "faster=$faster"
    echo "tooling=$tooling"
  } >> "$GITHUB_OUTPUT"
fi
[ "$tooling" -eq 0 ]
