#!/usr/bin/env bash
# Tier 1 job 6 (docs/HANDOFF.md §8): the coverage ratchet.
#
# Line coverage may not drop by more than RATCHET_MAX_DROP points (default 0.5)
# relative to the commit being merged onto. A ratchet, not an absolute bar:
# absolute bars rot into gaming, and a percentage target invites tests written
# to the metric.
#
# There is no stored baseline. The base is checked out into a throwaway
# worktree and measured in the same run, so nothing is hand-maintained and
# nothing can go stale. The price is a second instrumented build of the
# workspace crates; the dependency graph is shared through CARGO_TARGET_DIR.
#
# This gate deliberately fails CLOSED:
#   * an empty test selection is nextest exit 4, not a green run over nothing;
#   * a side that counts zero lines is an instrumentation failure, not 0%;
#   * a base that cannot be resolved is an error, not a pass.
set -euo pipefail

RATCHET_MAX_DROP="${RATCHET_MAX_DROP:-0.5}"

# BASE_SHA: the commit the change is being merged onto. In CI, HEAD is the
# synthetic merge commit GitHub builds for pull_request and merge_group events
# (and, main being squash-only, a plain commit on push), so its first parent is
# exactly the merge target. The event payload's base sha can lag behind the
# merge ref when main moves; HEAD^1 cannot.
if [ -z "${BASE_SHA:-}" ]; then
  if ! BASE_SHA="$(git rev-parse -q --verify 'HEAD^1^{commit}')"; then
    echo "::error::HEAD has no parent; cannot resolve the coverage base (checkout needs fetch-depth: 0)"
    exit 1
  fi
fi
if ! git cat-file -e "${BASE_SHA}^{commit}" 2>/dev/null; then
  echo "::error::base commit ${BASE_SHA} is not present; checkout needs fetch-depth: 0"
  exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
target_dir="${CARGO_TARGET_DIR:-${repo_root}/target}"
report_dir="$(mktemp -d)"
base_dir="${report_dir}/base"
trap 'git -C "${repo_root}" worktree remove --force "${base_dir}" 2>/dev/null || true; rm -rf "${report_dir}"' EXIT

# --summary-only keeps the JSON to totals; --profile ci is the same timeout
# budget the test job runs under, so a slow test is red here for the same
# reason it is red there.
measure() {
  local manifest="$1" out="$2"
  CARGO_TARGET_DIR="${target_dir}" cargo llvm-cov nextest \
    --manifest-path "${manifest}" \
    --workspace --all-features --locked --profile ci \
    --json --summary-only > "${out}"
}

lines_pct()   { jq -r '.data[0].totals.lines.percent' "$1"; }
lines_count() { jq -r '.data[0].totals.lines.count'   "$1"; }

# Head first: a failing test should die before we pay for the base build.
echo "::group::coverage: head ($(git rev-parse --short HEAD))"
measure "${repo_root}/Cargo.toml" "${report_dir}/head.json"
echo "::endgroup::"

# The base worktree path is canonicalised on purpose. cargo-llvm-cov excludes
# each package's tests/ directory by a regex built from cargo metadata; if the
# worktree sits behind a symlink (macOS /tmp -> /private/tmp) the regex misses
# and the base is measured with its tests counted as covered lines.
echo "::group::coverage: base ($(git rev-parse --short "${BASE_SHA}"))"
git -C "${repo_root}" worktree add -q --detach "${base_dir}" "${BASE_SHA}"
base_dir="$(cd "${base_dir}" && pwd -P)"
measure "${base_dir}/Cargo.toml" "${report_dir}/base.json"
echo "::endgroup::"

head_count="$(lines_count "${report_dir}/head.json")"
base_count="$(lines_count "${report_dir}/base.json")"
if [ "${head_count}" -le 0 ] || [ "${base_count}" -le 0 ]; then
  echo "::error::coverage counted zero lines (head=${head_count} base=${base_count}); instrumentation is broken, not the code"
  exit 1
fi

head_pct="$(lines_pct "${report_dir}/head.json")"
base_pct="$(lines_pct "${report_dir}/base.json")"
delta="$(jq -n --argjson h "${head_pct}" --argjson b "${base_pct}" '$h - $b')"

printf 'line coverage: base %.2f%% (%s lines) -> head %.2f%% (%s lines), delta %+.2f points, floor -%s\n' \
  "${base_pct}" "${base_count}" "${head_pct}" "${head_count}" "${delta}" "${RATCHET_MAX_DROP}"

if jq -en --argjson d "${delta}" --argjson m "${RATCHET_MAX_DROP}" '$d < -$m' >/dev/null; then
  cat <<MSG
::error::line coverage dropped $(printf '%.2f' "${delta#-}") points, more than the ${RATCHET_MAX_DROP}-point ratchet allows.

Either the change removed tests without removing the code they covered, or it
added logic without a test that reaches it. Run 'just coverage' locally for the
same comparison, or 'cargo llvm-cov nextest --workspace --all-features --open'
to see which lines went dark.
MSG
  exit 1
fi

echo "coverage ratchet satisfied"
