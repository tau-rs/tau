#!/usr/bin/env bash
# Tier 3 item 3, second half (docs/HANDOFF.md §8, #205): scheduled advisories.
#
# `cargo deny check advisories` on the committed lockfile, on a schedule,
# because a RUSTSEC entry arrives independently of any commit and Tier 1's
# deny job only runs when someone pushes. Every advisory cargo-deny reports
# at warning or above (a vulnerability, an unmaintained or unsound crate, a
# yanked version) is appended to FINDINGS_OUT (tab-separated: id, crate,
# version, kind, title, url) and the workflow files it as an issue. An
# advisory is not a failure of this script: it is not the committer's
# fault. The ignore list in deny.toml is honoured; an ignored advisory is a
# note, and notes are not findings. Run from the repository root: deny.toml
# is found from the cwd.
#
# Fails CLOSED:
#   * fewer than CRATES_FLOOR crates gathered (an empty or half-read tree);
#   * cargo-deny exits non-zero without reporting a finding (the advisory
#     database could not be fetched, the config did not parse).
set -euo pipefail

CRATES_FLOOR="${CRATES_FLOOR:-50}"
FINDINGS_OUT="${FINDINGS_OUT:-advisories.tsv}"
WORK="${WORK:-target/advisories}"

rm -rf "$WORK"
mkdir -p "$WORK"
: > "$FINDINGS_OUT"

rc=0
# No --config: deny.toml is found from the cwd, the repo root (#186), and
# the flag's position moved between cargo-deny 0.19 and 0.20.
cargo deny -L info --format json check advisories > "$WORK/deny.json" 2>&1 || rc=$?

# The JSON is one object per line: logs and diagnostics. A line that is not
# JSON (a panic, a rustup notice) is kept for the artifact and skipped here.
jq -c 'select(type == "object")' "$WORK/deny.json" 2>/dev/null > "$WORK/lines.json" || true

# The floor is measured with cargo, not read from cargo-deny's log: 0.19
# logs "gathered N crates" at INFO and 0.20 does not.
crates="$(cargo metadata --locked --format-version 1 | jq '.packages | length')"
if [ -z "$crates" ] || [ "$crates" -lt "$CRATES_FLOOR" ]; then
  echo "::error title=advisories::cargo-deny gathered ${crates:-0} crate(s); the floor is $CRATES_FLOOR (an empty lane must not read green)"
  tail -c 2000 "$WORK/deny.json"
  exit 1
fi

jq -r '
  select(.type == "diagnostic")
  | .fields
  | select(.severity == "error" or .severity == "warning")
  | select(.code != "advisory-ignored" and .code != "advisory-not-detected")
  | [
      (.advisory.id // .code),
      (.graphs[0].Krate.name // .advisory.package // "?"),
      (.graphs[0].Krate.version // "?"),
      .code,
      (.advisory.title // .message // ""),
      (.advisory.url // "")
    ]
  | @tsv' "$WORK/lines.json" | sort -u > "$FINDINGS_OUT"

findings="$(wc -l < "$FINDINGS_OUT" | tr -d ' ')"
while IFS=$'\t' read -r id crate version kind title _; do
  echo "::warning title=advisory::$id: $crate $version ($kind) $title"
  echo "ADVISORY $id  $crate $version  $kind  $title"
done < "$FINDINGS_OUT"

if [ "$rc" -ne 0 ] && [ "$findings" -eq 0 ]; then
  echo "::error title=advisories::cargo-deny exited $rc and reported no finding; see $WORK/deny.json"
  tail -c 2000 "$WORK/deny.json"
  exit 1
fi

echo "crates=$crates findings=$findings"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "crates=$crates"
    echo "findings=$findings"
  } >> "$GITHUB_OUTPUT"
fi
