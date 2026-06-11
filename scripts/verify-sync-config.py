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
