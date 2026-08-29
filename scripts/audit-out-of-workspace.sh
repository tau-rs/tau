#!/usr/bin/env bash
#
# cargo-audit coverage for the out-of-workspace Cargo projects (issue #733).
#
# Eight tracked manifests carry their own `[workspace]` table, which makes
# each one an independent cargo project. The workspace `Cargo.lock` does not
# contain their dependencies, so `cargo audit` at the repo root never sees
# them and no RustSec advisory against those deps would ever surface.
#
# This script emits one `cargo-audit-<pkg>.json` report per such project into
# the output directory (default `_security/`), alongside the workspace report
# written by the caller. `security-daily.yml` diffs each report against its
# same-named counterpart from yesterday's artifact.
#
# Usage: scripts/audit-out-of-workspace.sh [OUT_DIR]
#
# Requires `cargo audit` (cargo-audit) on PATH.
set -euo pipefail

OUT_DIR="${1:-_security}"
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"
mkdir -p "$OUT_DIR"

# Per repo CLAUDE.md cargo rules. `-p` is not applicable here: every manifest
# below is its own workspace root, so `--manifest-path` already scopes the
# invocation to exactly one project.
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/audit-out-of-workspace}"

# Hardcoded floor. Discovery below is a glob over tracked manifests, and a
# glob fails OPEN — a renamed or deleted manifest would silently drop out of
# the audited set and nobody would notice. Every entry here MUST still be
# discovered; if one is not, the script fails.
REQUIRED=(
  crates/landlock-exec-repro/Cargo.toml
  crates/tau-domain/fuzz/Cargo.toml
  crates/tau-mcp-tokio/tests/fixtures/mock-mcp-server/Cargo.toml
  crates/tau-pkg/fuzz/Cargo.toml
  crates/tau-plugin-compat/fixtures/controlled-env-binary/Cargo.toml
  crates/tau-plugin-protocol/fuzz/Cargo.toml
  crates/tau-wasm-host/tests/fixtures/fs-probe/Cargo.toml
  crates/tau-wasm-host/tests/fixtures/http-probe/Cargo.toml
)

# Discover: every tracked Cargo.toml other than the workspace root that
# declares its own `[workspace]` table.
DISCOVERED=()
while IFS= read -r manifest; do
  [ "$manifest" = "Cargo.toml" ] && continue
  grep -q '^\[workspace\]' "$manifest" || continue
  DISCOVERED+=("$manifest")
done < <(git ls-files '*Cargo.toml' | sort)

missing=()
for req in "${REQUIRED[@]}"; do
  found=0
  for got in "${DISCOVERED[@]}"; do
    [ "$req" = "$got" ] && found=1 && break
  done
  [ "$found" = 1 ] || missing+=("$req")
done

echo "Discovered ${#DISCOVERED[@]} out-of-workspace manifest(s):"
printf '  %s\n' "${DISCOVERED[@]}"

# Audit the discovered set (so a NEW manifest is covered the day it lands),
# then fail at the end if the required floor was breached — reports are still
# produced either way, so a drifted list never costs a day of coverage.
status=0
written=0
for manifest in "${DISCOVERED[@]}"; do
  dir="$(dirname "$manifest")"
  lock="$dir/Cargo.lock"

  # Package name makes a nicer report slug than the path; fall back to a
  # path slug if the manifest is shaped unexpectedly.
  slug="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$manifest" | head -1)"
  if [ -z "$slug" ]; then
    slug="$(echo "${dir#crates/}" | tr '/' '-')"
  fi

  generated=0
  if [ ! -f "$lock" ]; then
    echo "--- $manifest: no Cargo.lock, generating a throwaway one"
    cargo generate-lockfile --manifest-path "$manifest"
    generated=1
  else
    echo "--- $manifest: auditing committed $lock"
  fi

  # cargo-audit exits non-zero when it finds advisories; the report file is
  # the signal, so don't let that abort the loop.
  cargo audit --file "$lock" --json > "$OUT_DIR/cargo-audit-$slug.json" || true
  written=$((written + 1))

  # Never leave a generated lockfile behind: fs-probe's and mock-mcp-server's
  # are gitignored on purpose, and the fuzz projects resolve fresh per build.
  if [ "$generated" = 1 ]; then
    rm -f "$lock"
  fi
done

if [ ${#missing[@]} -gt 0 ]; then
  echo "ERROR: required out-of-workspace manifest(s) no longer discovered:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  echo "If a manifest moved or was deleted, update REQUIRED in $0." >&2
  status=1
fi

echo "Wrote $written per-manifest report(s) to $OUT_DIR/"
exit $status
