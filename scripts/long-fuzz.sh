#!/usr/bin/env bash
# Tier 3 item 2 (docs/HANDOFF.md §8, #207): the long fuzz, one target.
#
# Runs fuzz/fuzz_targets/TARGET for BUDGET seconds on nightly, growing the
# corpus in fuzz/corpus/TARGET from the committed seeds and from whatever
# the last run left there (the workflow restores and saves that directory
# between nights), then minimises it so it does not grow without bound.
# Tier 2's fuzz job is the same run at fifteen minutes with the corpus
# thrown away; this is the run that finds what fifteen minutes cannot.
#
# A finding is not a failure of this script: a crash, a timeout, an OOM or
# a leak lands as an artifact under fuzz/artifacts/TARGET/, and its kind,
# the reproducer's path and the run's last lines go to FINDINGS_OUT
# (tab-separated: target, kind, artifact, excerpt file) for the workflow to
# file as an issue; the input that found it is not the committer's fault.
#
# Fails CLOSED:
#   * nightly or cargo-fuzz missing, the target not building;
#   * a run that ends without libFuzzer's DONE line, or with fewer than
#     EXECS_FLOOR executions (a target that could not run is not a target
#     that ran clean);
#   * a non-zero exit with no artifact to show for it.
set -euo pipefail

TARGET="${TARGET:?the fuzz target, one of fuzz/fuzz_targets/}"
BUDGET="${BUDGET:-14400}"
EXECS_FLOOR="${EXECS_FLOOR:-100000}"
FINDINGS_OUT="${FINDINGS_OUT:-findings-fuzz.tsv}"
FUZZ_DIR="${FUZZ_DIR:-fuzz}"
WORK="${WORK:-target/long-fuzz}"

command -v cargo-fuzz > /dev/null || { echo "::error title=long fuzz::cargo-fuzz is not installed"; exit 1; }
cargo +nightly --version > /dev/null 2>&1 || { echo "::error title=long fuzz::nightly is not installed"; exit 1; }
[ -f "$FUZZ_DIR/fuzz_targets/${TARGET//-/_}.rs" ] || { echo "::error title=long fuzz::no such target $TARGET under $FUZZ_DIR/fuzz_targets/"; exit 1; }

mkdir -p "$WORK"
FINDINGS_OUT="$(cd "$(dirname "$FINDINGS_OUT")" && pwd)/$(basename "$FINDINGS_OUT")"
WORK="$(cd "$WORK" && pwd)"
: > "$FINDINGS_OUT"

cd "$FUZZ_DIR"
# cargo-fuzz defaults `--target` to the triple *it* was built for, and
# install-action ships a static musl binary; ASan does not exist for musl.
# Name the host triple so the build follows rustc.
host="$(rustc +nightly -vV | sed -n 's/^host: //p')"
mkdir -p "corpus/$TARGET" "artifacts/$TARGET"
before="$(find "corpus/$TARGET" -type f | wc -l | tr -d ' ')"
find "artifacts/$TARGET" -type f > "$WORK/artifacts-before.txt"

# libFuzzer's exit status is written to a file: `|| true` on the pipeline
# would reset PIPESTATUS, and the grep is only there to keep the log quiet.
SECONDS=0
{ cargo +nightly fuzz run --target "$host" "$TARGET" "corpus/$TARGET" "seeds/$TARGET" -- \
    -max_total_time="$BUDGET" -print_final_stats=1 2>&1; echo "$?" > "$WORK/rc"; } \
  | tee "$WORK/fuzz.log" | grep -E '^(#[0-9]+[[:space:]]+(DONE|INITED|NEW |REDUCE)|==|SUMMARY|stat::|panicked|thread )' || true
rc="$(cat "$WORK/rc")"
wall=$SECONDS
after="$(find "corpus/$TARGET" -type f | wc -l | tr -d ' ')"

# What the run left under artifacts/ that was not there before: the
# reproducer, named by what it reproduces.
find "artifacts/$TARGET" -type f | sort > "$WORK/artifacts-after.txt"
sort -o "$WORK/artifacts-before.txt" "$WORK/artifacts-before.txt"
new="$(comm -13 "$WORK/artifacts-before.txt" "$WORK/artifacts-after.txt" | grep -E '/(crash|timeout|oom|leak)-' | head -1 || true)"

if [ -n "$new" ]; then
  kind="$(basename "$new" | sed -E 's/-.*//')"
  # The panic or sanitizer report and libFuzzer's own summary, for the
  # issue; the log is an artifact.
  first="$(grep -nE '^(thread .* panicked|==[0-9]+==[[:space:]]*ERROR|SUMMARY|panicked at)' "$WORK/fuzz.log" | head -1 | cut -d: -f1)"
  if [ -n "$first" ]; then
    sed -n "${first},\$p" "$WORK/fuzz.log" | head -60 > "$WORK/excerpt.txt"
  else
    tail -60 "$WORK/fuzz.log" > "$WORK/excerpt.txt"
  fi
  printf '%s\t%s\t%s\t%s\n' "$TARGET" "$kind" "$FUZZ_DIR/$new" "$WORK/excerpt.txt" >> "$FINDINGS_OUT"
  echo "::warning title=fuzz $kind::$TARGET: $new"
  echo "FINDING  $TARGET  $kind  $new  after ${wall}s"
elif [ "$rc" -ne 0 ]; then
  echo "::error title=long fuzz::$TARGET exited $rc after ${wall}s with no artifact; see $WORK/fuzz.log"
  tail -30 "$WORK/fuzz.log"
  exit 1
else
  done_line="$(grep -E '^#[0-9]+[[:space:]]+DONE' "$WORK/fuzz.log" | tail -1 || true)"
  execs="$(sed -nE 's/^#([0-9]+)[[:space:]]+DONE.*/\1/p' <<<"$done_line")"
  if [ -z "$execs" ]; then
    echo "::error title=long fuzz::$TARGET ended without libFuzzer's DONE line; see $WORK/fuzz.log"
    tail -30 "$WORK/fuzz.log"
    exit 1
  fi
  if [ "$execs" -lt "$EXECS_FLOOR" ]; then
    echo "::error title=long fuzz::$TARGET ran $execs input(s) in ${wall}s; the floor is $EXECS_FLOOR (an empty lane must not read green)"
    exit 1
  fi
  echo "ok       $TARGET  $execs execs in ${wall}s  $done_line"
fi

# Keep the corpus small: the same coverage in fewer inputs. cmin needs the
# built target and runs for a minute or so; a cmin that fails leaves the
# corpus as it was, which is worse than nothing only in disk.
if ! cargo +nightly fuzz cmin --target "$host" "$TARGET" "corpus/$TARGET" > "$WORK/cmin.log" 2>&1; then
  echo "::warning title=long fuzz::cmin failed for $TARGET; the corpus is kept unminimised (see $WORK/cmin.log)"
fi
minimised="$(find "corpus/$TARGET" -type f | wc -l | tr -d ' ')"

echo "target=$TARGET wall=${wall}s corpus=$before->$after->$minimised (restored->grown->minimised) findings=$(wc -l < "$FINDINGS_OUT" | tr -d ' ')"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "wall=$wall"
    echo "corpus_before=$before"
    echo "corpus_after=$after"
    echo "corpus_minimised=$minimised"
    echo "findings=$(wc -l < "$FINDINGS_OUT" | tr -d ' ')"
  } >> "$GITHUB_OUTPUT"
fi
