#!/usr/bin/env bash
# scripts/wasm-guest-size.sh — measure and publish the tau wasm-guest
# bundle size (EPIC 5.6).
#
# # What it measures
#
# The shipped browser artifact is the `tau-wasm-guest` WASI-0.2 *component*
# built for `wasm32-wasip2` against the committed empty-cap baseline world —
# the minimal guest floor, deterministic with no env vars (the same build CI's
# link-gate already runs). Two numbers are reported:
#
#   1. Shipped component size  — the real browser download. Measured with
#      `wasm-tools` (Binaryen cannot parse components; see binaryen#6728).
#   2. Tree-shaken code floor  — the core module extracted with
#      `wasm-tools component unbundle`, run through Binaryen `wasm-metadce`
#      (rooted at the component's real exports) then `wasm-opt -Oz`. This is
#      the "via wasm-metadce" number the roadmap (5.6) asked for: it shows how
#      much of the shipped bytes are custom sections + already-dead code the
#      browser loader never needs.
#
# The shipped size is checked against TAU_WASM_SIZE_BUDGET (a regression
# tripwire, not a tuning target).
#
# # Usage
#
#   env CARGO_TARGET_DIR=target/main scripts/wasm-guest-size.sh
#   env CARGO_TARGET_DIR=target/main scripts/wasm-guest-size.sh --github-summary
#   env CARGO_TARGET_DIR=target/main TAU_WASM_SIZE_BUDGET=32768 scripts/wasm-guest-size.sh
#
# The caller MUST supply CARGO_TARGET_DIR per the workspace CARGO RULES
# (CLAUDE.md). Requires: cargo, rustup target wasm32-wasip2, wasm-tools.
# Binaryen (wasm-metadce, wasm-opt) is optional — if absent, the tree-shaken
# floor is skipped with a warning and only the shipped size is reported/gated.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/wasm-guest-size}"
export CARGO_TARGET_DIR
export CARGO_INCREMENTAL=0
# Default budget: ~2x the baseline floor (15,686 B) — catches accidental bloat
# (e.g. std creeping back into the no_std guest) without flapping on churn.
BUDGET="${TAU_WASM_SIZE_BUDGET:-32768}"

EMIT_SUMMARY=0
EMIT_JSON=0
for arg in "$@"; do
  case "$arg" in
    --github-summary) EMIT_SUMMARY=1 ;;
    --json)           EMIT_JSON=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 64 ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || { echo "error: '$1' not found on PATH" >&2; exit 69; }; }
need cargo
need wasm-tools

echo "==> building tau-wasm-guest (wasm32-wasip2, release, empty-cap baseline world)"
cargo build -p tau-wasm-guest --target wasm32-wasip2 --release

WASM="$CARGO_TARGET_DIR/wasm32-wasip2/release/tau_wasm_guest.wasm"
[ -f "$WASM" ] || { echo "error: expected artifact not found: $WASM" >&2; exit 70; }

# 1. Shipped component size (wasm-tools understands components; Binaryen does not).
SHIPPED=$(wc -c < "$WASM" | tr -d ' ')

# 2. Tree-shaken code floor via Binaryen — operates on the extracted core module.
METADCE="n/a"
OZ="n/a"
if command -v wasm-metadce >/dev/null 2>&1 && command -v wasm-opt >/dev/null 2>&1; then
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  # Extract the embedded core module (--threshold 0 => every module).
  wasm-tools component unbundle "$WASM" --module-dir "$WORK" --threshold 0 \
    -o "$WORK/rewrapped.wasm" >/dev/null
  CORE="$(find "$WORK" -name 'unbundled-module*.wasm' | head -1)"
  if [ -n "$CORE" ]; then
    # Build a metadce reachability graph rooting every real core export, so
    # metadce keeps live code and removes only dead code + custom sections.
    GRAPH="$WORK/graph.json"
    {
      echo '['
      wasm-tools print "$CORE" \
        | sed -n 's/.*(export "\([^"]*\)".*/\1/p' \
        | awk 'NR>1{printf ",\n"} {printf "  {\"name\":\"%s\",\"root\":true,\"export\":\"%s\"}", $0, $0}'
      echo ''
      echo ']'
    } > "$GRAPH"
    wasm-metadce --graph-file "$GRAPH" "$CORE" -o "$WORK/metadce.wasm" >/dev/null
    METADCE=$(wc -c < "$WORK/metadce.wasm" | tr -d ' ')
    wasm-opt -Oz --strip-debug --strip-producers "$WORK/metadce.wasm" -o "$WORK/oz.wasm" >/dev/null
    OZ=$(wc -c < "$WORK/oz.wasm" | tr -d ' ')
  fi
else
  echo "note: Binaryen (wasm-metadce/wasm-opt) not on PATH — skipping tree-shaken floor" >&2
fi

kib() { awk "BEGIN{printf \"%.1f\", $1/1024}"; }

echo
echo "tau wasm-guest bundle size (wasm32-wasip2, release, empty-cap baseline):"
printf '  %-38s %8s B  (%s KiB)\n' "shipped component (browser download)" "$SHIPPED" "$(kib "$SHIPPED")"
if [ "$METADCE" != "n/a" ]; then
  printf '  %-38s %8s B  (%s KiB)\n' "core after wasm-metadce" "$METADCE" "$(kib "$METADCE")"
  printf '  %-38s %8s B  (%s KiB)\n' "  + wasm-opt -Oz" "$OZ" "$(kib "$OZ")"
fi
echo "  budget (shipped): $BUDGET B"

if [ "$EMIT_JSON" = 1 ]; then
  printf '{"shipped_bytes":%s,"metadce_bytes":"%s","oz_bytes":"%s","budget_bytes":%s}\n' \
    "$SHIPPED" "$METADCE" "$OZ" "$BUDGET"
fi

if [ "$EMIT_SUMMARY" = 1 ] && [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### tau wasm-guest bundle size"
    echo
    echo "\`wasm32-wasip2\`, release, empty-cap baseline world (minimal guest floor)."
    echo
    echo "| Measurement | Bytes | KiB | Tool |"
    echo "|---|--:|--:|---|"
    echo "| Shipped component (browser download) | $SHIPPED | $(kib "$SHIPPED") | wasm-tools |"
    if [ "$METADCE" != "n/a" ]; then
      echo "| Core after \`wasm-metadce\` | $METADCE | $(kib "$METADCE") | Binaryen |"
      echo "| + \`wasm-opt -Oz\` | $OZ | $(kib "$OZ") | Binaryen |"
    fi
    echo
    echo "Budget (shipped): $BUDGET B."
  } >> "$GITHUB_STEP_SUMMARY"
fi

if [ "$SHIPPED" -gt "$BUDGET" ]; then
  echo "error: shipped component $SHIPPED B exceeds budget $BUDGET B" >&2
  echo "  If this growth is intended, raise TAU_WASM_SIZE_BUDGET in ci.yml and note why." >&2
  exit 1
fi

echo "OK: shipped component within budget."
