#!/usr/bin/env bash
#
# Reclaim `target/` directories across sibling Conductor worktrees.
#
# Conductor spawns 2-3 worktrees per lane and each one builds its own
# multi-GB `target/`. Nothing ever reclaims them, so the workspace grows
# without bound until the disk fills. On 2026-08-27 a session was hard-blocked
# mid-task by a full disk: `Bash` could not create a zero-byte file, so the
# agent could not even clean up after itself. A sweep across 42 worktrees
# reclaimed 199 GiB (issue #719).
#
# Deliberately written in bash, NOT as an `xtask` subcommand: this is the
# recovery tool for a full disk, and `cargo build` is exactly what does not
# work in that state. It must run with nothing but a shell and git.
#
# What it removes: `target/` directories only, and only ones that sit directly
# inside a git worktree under the sweep root. `target/` is build output --
# removing it costs a rebuild, never work. Source, git state, and uncommitted
# changes are never touched.
#
# What it deliberately does NOT remove: the sccache directory
# (`~/Library/Caches/Mozilla.sccache`, ~20G). It is large, but it is what makes
# the post-sweep rebuild warm instead of cold (~30-50 min cold). Dropping
# `target/` while keeping sccache is strictly the better trade, so this script
# never touches the cache and no flag exposes it.

set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: scripts/clean-worktrees.sh [--yes] [--root DIR]

Reclaims `target/` directories across sibling worktrees.

  --yes         Actually delete. Without this the script is a DRY RUN and
                only reports what it would reclaim.
  --root DIR    Sweep root (default: the parent of this repository, which is
                the Conductor workspaces directory).
  -h, --help    This message.

A worktree's `target/` is reclaimed when either:

  MERGED     its branch has a merged PR -- the lane is finished, so the whole
             directory is waste. Squash-merge means `git branch --merged`
             cannot detect this, so PR state is the only reliable signal.
  DUPLICATE  several worktrees share the branch -- that is ONE lane, so only
             the most recently built `target/` is kept and the rest go.

Everything else is KEPT. Refuses to run while a build is active.
USAGE
}

# --- argument parsing ------------------------------------------------------

APPLY=0
ROOT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --yes) APPLY=1; shift ;;
        --root) ROOT="${2:-}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "error: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# scripts/ lives at the repo root, so SELF is the worktree this script was
# invoked from and its parent holds all the sibling worktrees.
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
SELF=$(dirname -- "$script_dir")

if [ -z "$ROOT" ]; then
    ROOT=$(dirname -- "$SELF")
fi

# --- safety guards ---------------------------------------------------------

# This script deletes directories outside its own repository, so the sweep root
# is validated before anything is enumerated. A root of `/` or `$HOME` is
# always a mistake and would be catastrophic.
if [ ! -d "$ROOT" ]; then
    echo "error: sweep root is not a directory: $ROOT" >&2
    exit 2
fi
ROOT=$(CDPATH='' cd -- "$ROOT" && pwd)
case "$ROOT" in
    /|"$HOME") echo "error: refusing to sweep $ROOT" >&2; exit 2 ;;
esac

# A `target/` removed while cargo is mid-write leaves a corrupt tree that
# fails in confusing ways later. `TAU_CLEAN_SKIP_BUILD_GUARD` exists so the
# xtask test can run under `cargo test` (which would otherwise always trip
# this); it is not meant for interactive use.
if [ "${TAU_CLEAN_SKIP_BUILD_GUARD:-0}" != "1" ]; then
    if pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
        echo "error: a build is active (cargo/rustc running) -- refusing to sweep." >&2
        echo "       Deleting a target/ under a live build corrupts it. Wait, then retry." >&2
        exit 1
    fi
fi

# --- helpers ---------------------------------------------------------------

# Branch of a worktree, or empty if it is not a git checkout.
#
# `symbolic-ref` rather than `rev-parse --abbrev-ref HEAD`: it reports the
# branch on an unborn HEAD (a repo with no commits yet), which keeps the test
# fixture cheap. Detached HEAD has no branch and yields empty, which callers
# treat as "never group, never auto-remove".
worktree_branch() {
    git -C "$1" symbolic-ref --quiet --short HEAD 2>/dev/null || true
}

# Whether a branch has a merged PR. Requires `gh`; when it is missing or
# offline every branch is reported unmerged, so the sweep degrades to
# deduplication only rather than failing or, worse, guessing.
branch_is_merged() {
    [ "${TAU_CLEAN_SKIP_GH:-0}" = "1" ] && return 1
    command -v gh >/dev/null 2>&1 || return 1
    [ -n "$(gh pr list -R tau-rs/tau --head "$1" --state merged --limit 1 \
        --json number --jq '.[].number' 2>/dev/null)" ]
}

# Free bytes on the filesystem holding the sweep root, in KiB.
#
# Reclaim is reported from `df` deltas, never from summing `du`. On APFS,
# clone-block sharing makes per-directory `du` overcount badly -- during the
# #719 sweep `du -sch */target` claimed 426G inside a 208G workspace.
free_kib() { df -k "$ROOT" | tail -1 | awk '{print $4}'; }

human() { du -sh "$1" 2>/dev/null | cut -f1 | tr -d ' \t'; }

# Modification time of a path, in seconds since the epoch.
#
# GNU is tried FIRST and the result is validated as a number, because the two
# stat dialects do not fail cleanly for each other. On GNU coreutils `-f` means
# `--file-system`, so a BSD-style `stat -f '%m'` there does not error out -- it
# succeeds and prints something non-numeric. That silently gave every worktree
# an unusable timestamp, so no branch had an identifiable newest copy and
# *every* duplicate was marked for removal, keeper included. Exactly the
# mass-deletion behaviour of #719, reintroduced through a portability crack.
# A timestamp that cannot be read is therefore fatal, never a default.
mtime_of() {
    local t
    t=$(stat -c '%Y' "$1" 2>/dev/null) || t=$(stat -f '%m' "$1" 2>/dev/null) || t=""
    case "$t" in
        '' | *[!0-9]*)
            echo "error: cannot read a numeric mtime for $1 (got '${t}')" >&2
            exit 1
            ;;
    esac
    printf '%s' "$t"
}

# --- enumerate worktrees ---------------------------------------------------

# Parallel arrays: worktree path, its branch, its target/ mtime.
paths=()
branches=()
mtimes=()

for dir in "$ROOT"/*/; do
    dir="${dir%/}"
    [ -d "$dir/target" ] || continue
    # `.git` is a directory in a normal clone and a file in a linked worktree.
    [ -e "$dir/.git" ] || continue
    br=$(worktree_branch "$dir")
    paths+=("$dir")
    branches+=("$br")
    mtimes+=("$(mtime_of "$dir/target")")
done

if [ "${#paths[@]}" -eq 0 ]; then
    echo "No worktree target/ directories under $ROOT -- nothing to do."
    exit 0
fi

# --- classify --------------------------------------------------------------

# The one copy of a branch worth keeping: its warm cache.
#
# Normally that is whichever was built most recently, but the worktree the
# sweep was invoked FROM always wins if it is on this branch. Its owner is
# demonstrably mid-session, and in the real workspace the directory you are
# working in is very often the *older* copy of a duplicated lane -- so plain
# newest-wins would delete the caller's own cache out from under them.
# Preferring SELF (rather than merely exempting it) keeps deduplication
# effective in exactly that common case, instead of leaving both copies.
newest_for_branch() {
    local want="$1" best="" best_m=-1 i
    for i in "${!paths[@]}"; do
        [ "${branches[$i]}" = "$want" ] || continue
        [ "${paths[$i]}" = "$SELF" ] && { printf '%s' "$SELF"; return; }
        if [ "${mtimes[$i]}" -gt "$best_m" ]; then
            best_m="${mtimes[$i]}"; best="${paths[$i]}"
        fi
    done
    printf '%s' "$best"
}

count_for_branch() {
    local want="$1" n=0 i
    for i in "${!paths[@]}"; do
        [ "${branches[$i]}" = "$want" ] && n=$((n + 1))
    done
    printf '%s' "$n"
}

verdicts=()
for i in "${!paths[@]}"; do
    br="${branches[$i]}"
    if [ "${paths[$i]}" = "$SELF" ]; then
        # Never reclaim the worktree the sweep is being run from. It is the one
        # cache whose owner is demonstrably mid-session, and deleting it means
        # `just clean-worktrees --yes` costs the caller their own rebuild --
        # a surprising way for a cleanup command to behave. Being conservative
        # here can leave one duplicate copy behind; that is the cheaper mistake.
        verdicts+=("KEEP")
    elif [ -z "$br" ]; then
        # Detached HEAD: no branch to group or query, so never auto-remove.
        verdicts+=("KEEP")
    elif branch_is_merged "$br"; then
        verdicts+=("MERGED")
    elif keeper=$(newest_for_branch "$br"); [ -n "$keeper" ] \
        && [ "$(count_for_branch "$br")" -gt 1 ] \
        && [ "$keeper" != "${paths[$i]}" ]; then
        # `-n "$keeper"` is load-bearing, not a tidy-up. Without it, any future
        # failure to identify the newest copy degrades into "no path matches
        # the keeper", which marks EVERY copy of the branch DUPLICATE and
        # deletes the lot. The verdict must fail closed: no identifiable
        # keeper means keep everything.
        verdicts+=("DUPLICATE")
    else
        verdicts+=("KEEP")
    fi
done

# --- report ----------------------------------------------------------------

printf '%-40s %-36s %-10s %s\n' WORKTREE BRANCH VERDICT SIZE
reclaim_count=0
for i in "${!paths[@]}"; do
    printf '%-40s %-36s %-10s %s\n' \
        "$(basename "${paths[$i]}")" "${branches[$i]:-(detached)}" \
        "${verdicts[$i]}" "$(human "${paths[$i]}/target")"
    [ "${verdicts[$i]}" != "KEEP" ] && reclaim_count=$((reclaim_count + 1))
done
echo

if [ "$reclaim_count" -eq 0 ]; then
    echo "Nothing to reclaim: every worktree target/ is the newest copy of an unmerged lane."
    exit 0
fi

if [ "$APPLY" -ne 1 ]; then
    echo "DRY RUN: would reclaim $reclaim_count target/ director$([ "$reclaim_count" -eq 1 ] && echo y || echo ies)."
    echo "Re-run with --yes to apply."
    exit 0
fi

# --- apply -----------------------------------------------------------------

before=$(free_kib)
for i in "${!paths[@]}"; do
    [ "${verdicts[$i]}" = "KEEP" ] && continue
    victim="${paths[$i]}/target"
    # Re-check immediately before removing rather than trusting the verdict
    # computed above. An ad-hoc sweep during #719 printed "keeping <worktree>"
    # per branch and then removed every target/ anyway -- harmless that time
    # (no work lost, just a wider rebuild) but precisely the class of bug this
    # guard and the xtask test exist to prevent.
    case "$victim" in
        "$ROOT"/*/target) ;;
        *) echo "error: refusing to remove unexpected path: $victim" >&2; exit 1 ;;
    esac
    [ -d "$victim" ] || continue
    echo "  removing $victim (${verdicts[$i]})"
    rm -rf -- "$victim"
done
after=$(free_kib)

echo
echo "Reclaimed $(( (after - before) / 1024 / 1024 )) GiB (df delta); $(( after / 1024 / 1024 )) GiB now free."
