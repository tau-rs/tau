#!/bin/bash
# scripts/agent-push.sh — run the opt-in deep gate as a local pre-flight,
# then push.
#
# # Why this exists
#
# The deep gate (every Linux CI job mirrored in a Podman container) is
# NO LONGER an automatic pre-push hook — CI runs those exact jobs on
# every PR, so blocking each `git push` on a ~3-4 min (warm) / ~15-20 min
# (cold) container was pure latency, and it hard-failed in agent runtimes
# and environments without the podman socket. Plain `git push` is now
# safe and fast (no hook attached).
#
# This script is a convenience for the cases where you DO want the full
# local Linux pre-flight before pushing — e.g. cutting a release tag, or
# a large Rust change you want validated locally before CI. It runs the
# gate as a top-level command (whose Podman container outlives this
# shell, so even if the runtime kills the script mid-gate the container
# keeps running and you see the result on the next invocation), then
# pushes.
#
# For ordinary pushes you do NOT need this script — just `git push`.
#
# # Usage
#
#   scripts/agent-push.sh                       # gate, then push current branch
#   scripts/agent-push.sh -u origin <branch>    # passes through to git push
#
# Any args you pass are forwarded to the final `git push`. The gate runs
# first; if it fails, the push is aborted.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# 1. Run the opt-in deep gate as a top-level command. Output is streamed
#    to stdout so progress is visible; exit code is captured. The gate
#    spawns the Podman container; that container's lifetime is
#    independent of this script's shell.
echo "==> Running deep gate (opt-in pre-flight) as standalone command..."
if ! lefthook run deep-gate; then
  echo "==> Gate FAILED. Push aborted." >&2
  exit 1
fi

# 2. Gate passed — push. No pre-push hook exists anymore, so this is a
#    plain network-only step.
echo "==> Gate passed. Pushing..."
git push "$@"
