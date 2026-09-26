#!/usr/bin/env bash
# Tier 3 item 6 (docs/HANDOFF.md §8, #204): the flake hunter.
#
# Runs the whole workspace suite RUNS times under the `ci` nextest profile
# (the one Tier 1 runs), each time at a `--test-threads` width drawn at
# random, and compares the runs test by test from the JUnit report the
# profile writes. A test that passed in some runs and failed in others is a
# flake; a test that failed in every run is a plain failure Tier 1 missed.
# Neither is a failure of this script: each is appended to FLAKES_OUT
# (tab-separated: kind, test, passes, runs, widths it failed at, excerpt
# file) and the workflow files it as an issue, because a flake is not the
# committer's fault and a red build blames the wrong person. What *is* a
# failure here is anything that would let the job report green without
# having hunted.
#
# Fails CLOSED:
#   * fewer than TESTS_FLOOR tests in a run (an empty or half-selected
#     suite), or two runs that ran different sets of tests;
#   * a run that did not leave a JUnit report, or a nextest exit that is not
#     "ran" (0) or "ran, some failed" (100): a build error, a bad filter,
#     nextest exit 4 for an empty selection.
#
# WIDTHS="3 1 8" pins the widths (one per run) for a reproduction.
set -euo pipefail

RUNS="${RUNS:-5}"
TESTS_FLOOR="${TESTS_FLOOR:-400}"
PROFILE="${PROFILE:-ci}"
FLAKES_OUT="${FLAKES_OUT:-flakes.tsv}"
WORK="${WORK:-target/flake-hunt}"
# Widths are drawn from 1..=MAX_THREADS; twice the core count reaches the
# oversubscribed schedules a laptop under load produces (#182).
cores="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
MAX_THREADS="${MAX_THREADS:-$((cores * 2))}"
JUNIT="${JUNIT:-target/nextest/$PROFILE/junit.xml}"

command -v python3 > /dev/null || { echo "::error::python3 is needed to read the JUnit reports"; exit 1; }

rm -rf "$WORK"
mkdir -p "$WORK"
: > "$FLAKES_OUT"

if [ -n "${WIDTHS:-}" ]; then
  # shellcheck disable=SC2206 # splitting on whitespace is the point
  widths=($WIDTHS)
  [ "${#widths[@]}" -eq "$RUNS" ] || { echo "::error::WIDTHS names ${#widths[@]} width(s); RUNS is $RUNS"; exit 1; }
else
  widths=()
  for _ in $(seq 1 "$RUNS"); do
    widths+=("$((RANDOM % MAX_THREADS + 1))")
  done
fi
echo "widths: ${widths[*]} (drawn from 1..=$MAX_THREADS)"

# One TSV per run: classname, name, status (pass|fail|skip), from the
# JUnit report. The status of a failed test is whatever nextest put in the
# element (failure or error); the excerpt is its text, for the issue.
extract() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys, xml.etree.ElementTree as ET
junit, tsv, excerpts = sys.argv[1], sys.argv[2], sys.argv[3]
root = ET.parse(junit).getroot()
with open(tsv, "w") as out:
    for case in root.iter("testcase"):
        key = f'{case.get("classname")} {case.get("name")}'
        if case.find("skipped") is not None:
            status = "skip"
        elif case.find("failure") is not None or case.find("error") is not None:
            status = "fail"
            node = case.find("failure") if case.find("failure") is not None else case.find("error")
            text = (node.text or "").strip()
            err = case.find("system-err")
            if err is not None and err.text:
                text = text + "\n" + err.text.strip()
            with open(excerpts, "a") as ex:
                ex.write(f"=== {key}\n{text}\n")
        else:
            status = "pass"
        out.write(f"{key}\t{status}\n")
PY
}

ran=0
for i in $(seq 1 "$RUNS"); do
  # shellcheck disable=SC2004 # arrays index arithmetically
  width="${widths[$((i - 1))]}"
  rm -f "$JUNIT"
  rc=0
  cargo nextest run --locked --workspace --all-features --profile "$PROFILE" \
    --no-fail-fast --test-threads "$width" > "$WORK/run-$i.log" 2>&1 || rc=$?
  # 0: every test passed. 100: the suite ran and some failed, which is the
  # finding this script exists for. Anything else did not run the suite.
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 100 ]; then
    echo "::error title=flake hunter::run $i (width $width): nextest exited $rc, which is not a test result; see $WORK/run-$i.log"
    tail -40 "$WORK/run-$i.log"
    exit 1
  fi
  if [ ! -f "$JUNIT" ]; then
    echo "::error title=flake hunter::run $i (width $width): no JUnit report at $JUNIT; does the $PROFILE profile set [profile.$PROFILE.junit]?"
    exit 1
  fi
  extract "$JUNIT" "$WORK/run-$i.tsv" "$WORK/run-$i.excerpts"
  sort -o "$WORK/run-$i.tsv" "$WORK/run-$i.tsv"
  tests="$(grep -cvE $'\tskip$' "$WORK/run-$i.tsv" || true)"
  failed="$(grep -cE $'\tfail$' "$WORK/run-$i.tsv" || true)"
  echo "run $i  width $width  tests $tests  failed $failed  exit $rc"
  if [ "$tests" -lt "$TESTS_FLOOR" ]; then
    echo "::error title=flake hunter::run $i ran $tests test(s); the floor is $TESTS_FLOOR (an empty lane must not read green)"
    exit 1
  fi
  ran=$((ran + 1))
done

# Every run must have run the same set: a run that selected differently is
# not comparable, and a diff of test names says which side is missing what.
cut -f1 "$WORK/run-1.tsv" > "$WORK/names"
for i in $(seq 2 "$RUNS"); do
  if ! cut -f1 "$WORK/run-$i.tsv" | diff -q - "$WORK/names" > /dev/null; then
    echo "::error title=flake hunter::run $i ran a different set of tests than run 1"
    cut -f1 "$WORK/run-$i.tsv" | diff - "$WORK/names" | head -20
    exit 1
  fi
done

# Aggregate: for each test, how many runs passed it and at which widths it
# failed. `paste` lines the runs up; the names were checked equal above.
paste "$WORK"/run-*.tsv > "$WORK/all.tsv"
flaky=0
failing=0
n=0
while IFS=$'\t' read -r -a cols; do
  name="${cols[0]}"
  passes=0
  fails=0
  failed_widths=""
  for i in $(seq 1 "$RUNS"); do
    # Column 2i-1 holds run i's status (0-based: 2i-1).
    status="${cols[$((2 * i - 1))]}"
    case "$status" in
      pass) passes=$((passes + 1)) ;;
      fail) fails=$((fails + 1)); failed_widths="$failed_widths ${widths[$((i - 1))]}" ;;
    esac
  done
  [ "$fails" -gt 0 ] || continue
  n=$((n + 1))
  excerpt="$WORK/excerpt-$n.txt"
  # The first failing run's excerpt for this test, 60 lines at most.
  for i in $(seq 1 "$RUNS"); do
    if [ -f "$WORK/run-$i.excerpts" ] && grep -qF "=== $name" "$WORK/run-$i.excerpts"; then
      awk -v k="=== $name" '$0 == k {p=1; next} /^=== / {p=0} p' "$WORK/run-$i.excerpts" | head -60 > "$excerpt"
      break
    fi
  done
  [ -s "$excerpt" ] || echo "(nextest recorded no output for this failure)" > "$excerpt"
  if [ "$passes" -gt 0 ]; then
    kind=flaky
    flaky=$((flaky + 1))
    echo "::warning title=flaky test::$name passed $passes/$ran; failed at width(s)${failed_widths}"
    echo "FLAKY    $name  passed $passes/$ran  failed at width(s)${failed_widths}"
  else
    kind=failing
    failing=$((failing + 1))
    echo "::warning title=failing test::$name failed in every run"
    echo "FAILING  $name  passed 0/$ran"
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$kind" "$name" "$passes" "$ran" "${failed_widths# }" "$excerpt" >> "$FLAKES_OUT"
done < "$WORK/all.tsv"

tests="$(grep -cvE $'\tskip$' "$WORK/run-1.tsv" || true)"
echo "runs=$ran tests=$tests widths=${widths[*]} flaky=$flaky failing=$failing"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "runs=$ran"
    echo "tests=$tests"
    echo "widths=${widths[*]}"
    echo "flaky=$flaky"
    echo "failing=$failing"
  } >> "$GITHUB_OUTPUT"
fi
[ "$ran" -eq "$RUNS" ]
