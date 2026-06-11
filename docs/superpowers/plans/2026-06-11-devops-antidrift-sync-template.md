# DevOps Anti-Drift Sync Template (SOURCE side) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the SOURCE side of the cross-repo CI template sync in tau, so drift in the canonical `.github/workflows/*` + `.github/actions/*` surface becomes a VISIBLE open PR in the sibling repos (cairn / cairn-ui / tau-ui) instead of silent rot.

**Architecture:** Add a declarative `.github/sync.yml` (target repos + file globs) plus a `.github/workflows/sync-template.yml` workflow that runs `BetaHuhn/repo-file-sync-action` (SHA-pinned). On push to `main` touching the template surface — or on manual dispatch — the action opens PRs into the three siblings. Each sibling keeps its FULL self-contained `ci.yml` (the synced PR is a real file copy reviewed by the target repo's owner), so there is NO runtime `workflow_call` to a central moving-tag workflow — the model's core constraint is preserved.

**Tech Stack:** GitHub Actions, `BetaHuhn/repo-file-sync-action@v1.21.1`, YAML.

---

## Mechanism decision (why repo-file-sync-action, not Renovate or multi-gitter)

The brief asks to pick ONE of three and justify it. The gap the audit names is
**file distribution** ("no `repo-file-sync-action` config, no sync workflow"),
not pin freshness.

| Mechanism | Distributes shared files cross-repo? | Source config lives in tau? | Verdict |
|---|---|---|---|
| **`BetaHuhn/repo-file-sync-action`** | **Yes — purpose-built.** Reads `.github/sync.yml`, opens PRs in targets. | **Yes** — workflow + `sync.yml` in tau. | **CHOSEN** |
| Renovate | No. Renovate updates *dependencies* / SHA pins; it has no "copy these files to repos B/C/D" primitive. | A `renovate.json` would live in each *target*, not centrally in tau. | Rejected — does not solve the file-sync gap; pin freshness is already handled by `.github/dependabot.yml`. |
| multi-gitter | Yes, but as an **ad-hoc local CLI** a human runs from a laptop. | No persistent in-repo source config that auto-opens PRs. | Rejected — no operationalized "tau = source of truth" artifact. |

`repo-file-sync-action` is the only option that (a) actually copies files and
(b) leaves a committed SOURCE artifact in tau that auto-opens drift PRs. It does
NOT introduce a runtime `workflow_call`: the action copies file *bytes* into the
target's own tree, so each sibling's `ci.yml` stays fully self-contained.

---

## File structure

- **Create `.github/sync.yml`** — declarative config: one `group` listing the
  three target repos (`tau-rs/cairn`, `tau-rs/cairn-ui`, `tau-rs/tau-ui`) and the
  canonical template file mappings (`.github/workflows/`, `.github/actions/`,
  `deny.toml`, `lefthook.yml`; `justfile` commented until brief 70 lands). The
  sync-meta workflow itself is EXCLUDED to avoid turning siblings into sync bots.
- **Create `.github/workflows/sync-template.yml`** — the workflow that runs the
  action. Triggers: `push` to `main` filtered to the template paths, plus
  `workflow_dispatch` with a `dry_run` input (default `true`) for safe
  verification. Pins all actions by SHA, matching repo convention.
- **Create `scripts/verify-sync-config.py`** — a local resolver that parses
  `sync.yml`, expands the directory globs against the real filesystem, applies
  excludes, and prints the file→repo mapping. This is the honest dry-run
  evidence (the real action needs a token + Actions runtime). Kept in-tree so the
  config can be re-verified after any future template change.

---

### Task 1: Declarative sync config (`.github/sync.yml`)

**Files:**
- Create: `.github/sync.yml`

- [ ] **Step 1: Write `.github/sync.yml`**

```yaml
# Cross-repo CI template sync — SOURCE config (tau = canonical source).
#
# Consumed by .github/workflows/sync-template.yml via
# BetaHuhn/repo-file-sync-action. On push to `main` (or manual dispatch) the
# action opens a PR in each target repo carrying the files below. Drift in the
# canonical CI surface therefore shows up as a VISIBLE open PR, never silent rot.
#
# Anti-drift model "B+C" (audit/devops.md §3):
#   B — each repo keeps its FULL self-contained ci.yml. This action copies file
#       BYTES into the target's own tree; there is NO runtime workflow_call to a
#       central moving-tag workflow (explicitly REJECTED for blast radius).
#   C — thin SHA-pinned composite actions (.github/actions/*) are synced too, so
#       the stable atomic layer stays byte-identical across all four repos.
#
# The target repo OWNS the merge decision on each sync PR — this is a proposal,
# not a force-push (SKIP_PR is not set; OVERWRITE_EXISTING_PR keeps one PR fresh).

group:
  # The three sibling repos that converge to tau's canonical CI template.
  repos: |
    tau-rs/cairn
    tau-rs/cairn-ui
    tau-rs/tau-ui

  files:
    # Full workflows directory = the canonical CI template surface. The brief
    # mandates the glob `.github/workflows/*` (not a named heavy file): the
    # heavy/release `v*` anchor lives in release.yml + tier2.yml, and the fast
    # gate in ci.yml + ci-summary.yml. deleteOrphaned is left at its default
    # (false) so a sibling's repo-specific workflows are never deleted — sync is
    # additive; convergence is reviewed per-PR by the target owner.
    - source: .github/workflows/
      dest: .github/workflows/
      exclude: |
        # The sync mechanism itself must NOT propagate — a sibling carrying
        # sync-template.yml would become a second sync source (recursion / a
        # second writer to the same three targets). Source-only by design.
        sync-template.yml

    # Thin composite actions (model layer C) — setup-rust + place-fixture-binaries.
    - source: .github/actions/
      dest: .github/actions/

    # cargo-deny policy — shared supply-chain / license gate.
    - source: deny.toml
      dest: deny.toml

    # Local git-hook definitions — keeps the lightweight pre-commit gate identical.
    - source: lefthook.yml
      dest: lefthook.yml

    # The universal `just` verb wrapper (brief 70). Uncomment once the justfile
    # lands in tau so local == CI verbs stay byte-identical across all four repos.
    # - source: justfile
    #   dest: justfile
```

- [ ] **Step 2: Validate it parses as YAML**

Run: `python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/sync.yml')); print('OK', list(d.keys()))"`
Expected: `OK ['group']`

- [ ] **Step 3: Commit**

```bash
git add .github/sync.yml
git commit -m "ci(sync): declare canonical CI template + target repos (audit devops §3)"
```

---

### Task 2: Sync workflow (`.github/workflows/sync-template.yml`)

**Files:**
- Create: `.github/workflows/sync-template.yml`

- [ ] **Step 1: Write the workflow**

```yaml
# Sync the canonical CI template (.github/sync.yml) into the sibling repos.
#
# Operationalizes "tau = source of truth" (audit/devops.md §3, Diagram 1): on
# any change to the template surface, BetaHuhn/repo-file-sync-action opens a PR
# in each target repo. Drift = a visible open PR, reviewed and merged by the
# target repo's owner.
#
# This is NOT a runtime workflow_call to a central workflow — the action copies
# file bytes into each target's own tree, so every sibling's ci.yml stays fully
# self-contained (model constraint B). No moving-tag SPOF.
#
# REQUIRED SECRET: REPO_FILE_SYNC_TOKEN — a PAT (classic: `repo` + `workflow`
# scopes; or fine-grained with Contents:write + Pull requests:write + Workflows
# on the three target repos). Until it is configured, the action step fails
# loudly (visible), never silently. Run the workflow manually with dry_run=true
# (the default) first to verify the file→repo mapping before granting write.
name: sync-template

on:
  push:
    branches: [main]
    # Only run when the canonical template surface actually changes.
    paths:
      - .github/workflows/**
      - .github/actions/**
      - deny.toml
      - lefthook.yml
      - .github/sync.yml
      # - justfile   # uncomment together with the sync.yml entry (brief 70)
  workflow_dispatch:
    inputs:
      dry_run:
        description: "Resolve and log the sync plan without opening PRs"
        type: boolean
        default: true

# Never let two syncs race; cancel the older queued run.
concurrency:
  group: sync-template
  cancel-in-progress: true

permissions:
  contents: read  # cross-repo writes use REPO_FILE_SYNC_TOKEN, not GITHUB_TOKEN

jobs:
  sync:
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6
      - name: Sync template to sibling repos
        uses: BetaHuhn/repo-file-sync-action@8b92be3375cf1d1b0cd579af488a9255572e4619  # v1.21.1
        with:
          GH_PAT: ${{ secrets.REPO_FILE_SYNC_TOKEN }}
          CONFIG_PATH: .github/sync.yml
          # push events run a real sync; manual runs honour the dry_run toggle.
          DRY_RUN: ${{ github.event_name == 'workflow_dispatch' && inputs.dry_run }}
          PR_LABELS: |
            sync
            ci-template
          COMMIT_PREFIX: "ci(sync):"
          PR_BODY: |
            Automated CI template sync from
            [`tau-rs/tau`](https://github.com/tau-rs/tau) (`.github/sync.yml`).

            This PR mirrors the canonical `.github/workflows/*` + `.github/actions/*`
            surface so this repo stays converged with the source. Review and merge
            (or close) — this repo owns the final decision. Drift is intentionally
            surfaced as a PR, never force-pushed.
```

- [ ] **Step 2: Validate it parses as YAML**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/sync-template.yml')); print('OK')"`
Expected: `OK`

- [ ] **Step 3: Confirm no hardcoded secret / no central workflow_call**

Run: `grep -nE 'workflow_call|ghp_|github_pat_|REPO_FILE_SYNC_TOKEN' .github/workflows/sync-template.yml`
Expected: only the `secrets.REPO_FILE_SYNC_TOKEN` reference appears; no `workflow_call`, no literal token.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/sync-template.yml
git commit -m "ci(sync): add sync-template workflow (SHA-pinned repo-file-sync-action)"
```

---

### Task 3: Dry-run verification resolver (`scripts/verify-sync-config.py`)

**Files:**
- Create: `scripts/verify-sync-config.py`

- [ ] **Step 1: Write the resolver**

```python
#!/usr/bin/env python3
"""Resolve .github/sync.yml against the working tree and print the file->repo
plan. Honest local stand-in for the action's DRY_RUN (which needs a token + an
Actions runtime). Exits non-zero if a declared source is missing or if the sync
workflow would propagate itself."""
import sys
from pathlib import Path
import yaml

REPO = Path(__file__).resolve().parent.parent
cfg = yaml.safe_load((REPO / ".github/sync.yml").read_text())

groups = cfg.get("group", [])
if isinstance(groups, dict):
    groups = [groups]

problems = []
print("=== sync.yml resolved plan ===\n")
for g in groups:
    repos = [r.strip() for r in str(g["repos"]).splitlines() if r.strip()]
    for entry in g["files"]:
        src = entry["source"] if isinstance(entry, dict) else entry
        excludes = set()
        if isinstance(entry, dict) and entry.get("exclude"):
            excludes = {e.strip() for e in str(entry["exclude"]).splitlines()
                        if e.strip() and not e.strip().startswith("#")}
        p = REPO / src
        if not p.exists():
            problems.append(f"MISSING source: {src}")
            continue
        if p.is_dir():
            files = sorted(f.relative_to(p).as_posix()
                           for f in p.rglob("*") if f.is_file())
            kept = [f for f in files if f not in excludes]
            skipped = [f for f in files if f in excludes]
        else:
            kept, skipped = [src], []
        print(f"[{src}] -> {len(kept)} file(s)")
        for f in kept:
            print(f"    + {f}")
        for f in skipped:
            print(f"    - {f}  (EXCLUDED)")
        print()

print("target repos:")
for g in groups:
    for r in [r.strip() for r in str(g["repos"]).splitlines() if r.strip()]:
        print(f"    -> {r}")

# Guard: the sync workflow must never propagate itself.
for g in groups:
    for entry in g["files"]:
        src = entry["source"] if isinstance(entry, dict) else entry
        p = REPO / src
        if p.is_dir():
            ex = set()
            if isinstance(entry, dict) and entry.get("exclude"):
                ex = {e.strip() for e in str(entry["exclude"]).splitlines() if e.strip()}
            for f in p.rglob("*"):
                if f.is_file() and f.name == "sync-template.yml" \
                        and f.relative_to(p).as_posix() not in ex:
                    problems.append("sync-template.yml is NOT excluded — recursion risk")

print()
if problems:
    print("FAIL:")
    for pb in problems:
        print(f"  - {pb}")
    sys.exit(1)
print("OK: all declared sources exist; sync-template.yml is excluded.")
```

- [ ] **Step 2: Run it and capture the dry-run output**

Run: `python3 scripts/verify-sync-config.py`
Expected: a per-source file listing, the three target repos, `sync-template.yml` shown as `EXCLUDED`, and a final `OK:` line; exit 0.

- [ ] **Step 3: Commit**

```bash
git add scripts/verify-sync-config.py
git commit -m "ci(sync): add local resolver to dry-run-verify sync.yml surface"
```

---

## Self-review

- **Spec coverage:** mechanism chosen + justified (decision table); SOURCE config
  in tau only (sync.yml + workflow); target list + globs explicit and commented;
  `.github/workflows/*` glob honored; no `workflow_call`; SHA-pinned action; secret
  documented not hardcoded; dry-run evidence via resolver. Does not touch siblings.
- **No central SPOF:** the action copies bytes; no runtime `workflow_call`.
- **Recursion guard:** `sync-template.yml` excluded; resolver asserts it.
- **justfile:** not yet in repo (brief 70) — left commented in both files so it is
  a one-line uncomment once it lands.
