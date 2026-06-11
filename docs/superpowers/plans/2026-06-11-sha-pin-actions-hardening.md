# SHA-Pin Actions Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the remaining mutable GitHub Actions refs immutable — SHA-pin the five tag-pinned actions in the two composite actions, the `anthropics/claude-code-action@beta` (×3), and the two direct `dtolnay/rust-toolchain@*` pins — then add bounded `timeout-minutes` to every job in `ci.yml`, `release.yml`, and `security-daily.yml`.

**Architecture:** Pure CI-config change. Every `uses:` keeps the SAME action version it has today, now expressed as a full 40-char commit SHA with a trailing `# <ref>` comment so Dependabot keeps bumping it. No source code changes. Verification is: workflows parse and the real CI run (the `ci` matrix + `ci-summary` gate) goes green with the new composite-action pins on the hot path.

**Tech Stack:** GitHub Actions YAML. `git ls-remote` was used to resolve every tag/branch to its commit SHA (recorded below — already verified to exist).

---

## Resolved SHA mapping (ground truth — verified via `git ls-remote` 2026-06-11)

| Action | Current ref | New pin (commit SHA + comment) | Source of SHA |
|---|---|---|---|
| `anthropics/claude-code-action` | `@beta` | `@28f83620103c48a57093dcc2837eec89e036bb9f  # beta` | `beta` annotated tag → commit |
| `dtolnay/rust-toolchain` (stable) | `@stable` | `@29eef336d9b2848a0b548edc03f92a220660cdb8  # stable` | `refs/heads/stable` |
| `dtolnay/rust-toolchain` (nightly) | `@nightly` | `@5b842231ba77f5c045dba54ac5560fed2db780e2  # nightly` | `refs/heads/nightly` |
| `rui314/setup-mold` | `@v1` | `@9c9c13bf4c3f1adef0cc596abc155580bcb04444  # v1` | `refs/tags/v1` |
| `mozilla-actions/sccache-action` | `@v0.0.10` | `@9e7fa8a12102821edf02ca5dbea1acd0f89a2696  # v0.0.10` | `refs/tags/v0.0.10^{}` (deref commit) |
| `taiki-e/install-action` | `@v2` | `@7a79fe8c3a13344501c80d99cae481c1c9085912  # v2` | `refs/tags/v2` |
| `Swatinem/rust-cache` | `@v2` | `@e18b497796c12c097a38f9edb9d0641fb99eee32  # v2` | **reused** — already in `mutants-scheduled.yml` |
| `actions/download-artifact` | `@v8` | `@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8` | **reused** — already in `docs-deploy.yml` |

### DECISION: claude-code-action pins to `@beta`'s current commit, NOT to v1.0.144

The brief says "pin to a specific released commit SHA, off the moving branch." The latest
release is `v1.0.144`, but its `action.yml` **renamed `direct_prompt` → `prompt` and dropped
several inputs** (verified by diffing the input schema at `beta` vs `v1.0.144`). Both
`claude-review.yml` jobs pass `direct_prompt`, so bumping to v1.0.144 would silently break the
auto-review and release-summary jobs — exactly the version-drift break the brief forbids
("keep every action at the SAME version, just expressed as a SHA").

The faithful pin is therefore the **commit that `@beta` resolves to right now**
(`28f83620103c48a57093dcc2837eec89e036bb9f` — `beta` is an annotated tag in this repo, not a
floating branch head). This freezes the exact code currently running, with zero behavior
change, while making the ref immutable. The `# beta` comment matches the repo's existing
non-semver pin convention (e.g. `taiki-e/install-action@fa8484446…  # nextest`,
`@492cad282…  # cargo-llvm-cov`) and keeps Dependabot tracking the beta line. Migrating the
input surface to v1.x is a separate, behavior-changing PR, out of scope here.

### Two-space comment convention

Match the existing style exactly: SHA, **two spaces**, `# <ref>`. Example already in the repo:
`actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6`.

---

## File Structure

Files modified (config only — no new files):

- `.github/workflows/claude.yml` — 1 claude-action pin (Task 1)
- `.github/workflows/claude-review.yml` — 2 claude-action pins (Task 1)
- `.github/actions/setup-rust/action.yml` — 5 composite-action pins (Task 2)
- `.github/actions/place-fixture-binaries/action.yml` — 1 download-artifact pin (Task 3)
- `.github/workflows/fuzz-nightly.yml` — 1 dtolnay nightly pin (Task 4)
- `.github/workflows/mutants-scheduled.yml` — 1 dtolnay stable pin (Task 4)
- `.github/workflows/ci.yml` — `timeout-minutes` on all 17 jobs (Task 5)
- `.github/workflows/release.yml` — `timeout-minutes` on the 6 step-based jobs (Task 6)
- `.github/workflows/security-daily.yml` — `timeout-minutes` on both jobs (Task 7)

**Out of scope (do NOT touch):** every workflow-level `uses:` that is already SHA-pinned;
`claude.yml`'s existing `concurrency` group; the reusable-workflow-call jobs in `release.yml`
(`preflight-tier1`, `preflight-tier2`) — GitHub forbids `timeout-minutes` on `uses:` jobs.

---

### Task 1: Pin `anthropics/claude-code-action@beta` (×3) — do this FIRST

**Files:**
- Modify: `.github/workflows/claude.yml:98`
- Modify: `.github/workflows/claude-review.yml:89,148`

- [ ] **Step 1: Replace the pin in `claude.yml`**

In `.github/workflows/claude.yml`, change line 98 from:

```yaml
      - uses: anthropics/claude-code-action@beta
```

to:

```yaml
      - uses: anthropics/claude-code-action@28f83620103c48a57093dcc2837eec89e036bb9f  # beta
```

- [ ] **Step 2: Replace both pins in `claude-review.yml`**

In `.github/workflows/claude-review.yml`, the string `uses: anthropics/claude-code-action@beta`
appears twice (the `review-pr` job at line 89 and the `release-summary` job at line 148).
Replace **both** occurrences with:

```yaml
      - uses: anthropics/claude-code-action@28f83620103c48a57093dcc2837eec89e036bb9f  # beta
```

(Preserve each line's existing indentation — both are `      - uses:`.)

- [ ] **Step 3: Verify no `@beta` ref remains**

Run: `grep -rn 'claude-code-action@beta' .github/`
Expected: no output (exit 1).

Run: `grep -rcn 'claude-code-action@28f83620103c48a57093dcc2837eec89e036bb9f  # beta' .github/workflows/claude.yml .github/workflows/claude-review.yml`
Expected: `claude.yml:1`, `claude-review.yml:2`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/claude.yml .github/workflows/claude-review.yml
git commit -m "ci: SHA-pin anthropics/claude-code-action off the @beta branch (G2)"
```

---

### Task 2: SHA-pin the five tag-pinned actions in `setup-rust/action.yml`

**Files:**
- Modify: `.github/actions/setup-rust/action.yml:56,63,75,111,116`

`setup-rust` is on the hot path of nearly every CI job, so a mistyped SHA fails fast and
loudly — good. Replace each tag ref with its SHA from the mapping table. Leave every
surrounding comment block and the `with:` inputs untouched.

- [ ] **Step 1: Pin `dtolnay/rust-toolchain` (line 56)**

Change:

```yaml
      uses: dtolnay/rust-toolchain@stable
```

to:

```yaml
      uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8  # stable
```

(The multi-line comment above it on lines 50-55 already explains the `@stable` choice — leave
that comment as-is; it still accurately describes why we track the `stable` line.)

- [ ] **Step 2: Pin `rui314/setup-mold` (line 63)**

Change:

```yaml
      uses: rui314/setup-mold@v1
```

to:

```yaml
      uses: rui314/setup-mold@9c9c13bf4c3f1adef0cc596abc155580bcb04444  # v1
```

- [ ] **Step 3: Pin `mozilla-actions/sccache-action` (line 75)**

Change:

```yaml
      uses: mozilla-actions/sccache-action@v0.0.10
```

to:

```yaml
      uses: mozilla-actions/sccache-action@9e7fa8a12102821edf02ca5dbea1acd0f89a2696  # v0.0.10
```

(The comment block on lines 65-72 references `@v0.0.10` by name — it stays accurate, leave it.)

- [ ] **Step 4: Pin `taiki-e/install-action` (line 111)**

Change:

```yaml
      uses: taiki-e/install-action@v2
```

to:

```yaml
      uses: taiki-e/install-action@7a79fe8c3a13344501c80d99cae481c1c9085912  # v2
```

- [ ] **Step 5: Pin `Swatinem/rust-cache` (line 116)**

Change:

```yaml
      uses: Swatinem/rust-cache@v2
```

to:

```yaml
      uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32  # v2
```

(This is the identical SHA already pinned in `mutants-scheduled.yml:92` — same v2, so the
`save-if`/`shared-key` behavior is provably unchanged.)

- [ ] **Step 6: Verify no tag refs remain in the composite action**

Run: `grep -nE 'uses: (dtolnay/rust-toolchain|rui314/setup-mold|mozilla-actions/sccache-action|taiki-e/install-action|Swatinem/rust-cache)@[a-z0-9.]+$' .github/actions/setup-rust/action.yml`
Expected: no output (exit 1 — every ref now ends in a 40-hex SHA + comment, not a bare tag).

Run: `grep -cE 'uses: [^ ]+@[0-9a-f]{40}  # ' .github/actions/setup-rust/action.yml`
Expected: `5`.

- [ ] **Step 7: Commit**

```bash
git add .github/actions/setup-rust/action.yml
git commit -m "ci: SHA-pin the 5 third-party actions in setup-rust composite (G2)"
```

---

### Task 3: SHA-pin `actions/download-artifact` in `place-fixture-binaries/action.yml`

**Files:**
- Modify: `.github/actions/place-fixture-binaries/action.yml:21`

- [ ] **Step 1: Pin `actions/download-artifact` (line 21)**

Change:

```yaml
      uses: actions/download-artifact@v8
```

to:

```yaml
      uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8
```

(Identical SHA + version to `docs-deploy.yml:279` — provably the same v8.)

- [ ] **Step 2: Verify**

Run: `grep -nE 'download-artifact@v8$' .github/actions/place-fixture-binaries/action.yml`
Expected: no output (exit 1).

Run: `grep -n 'download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8' .github/actions/place-fixture-binaries/action.yml`
Expected: line 21 matches.

- [ ] **Step 3: Commit**

```bash
git add .github/actions/place-fixture-binaries/action.yml
git commit -m "ci: SHA-pin actions/download-artifact in place-fixture-binaries composite (G2)"
```

---

### Task 4: SHA-pin the two direct `dtolnay/rust-toolchain@*` refs

**Files:**
- Modify: `.github/workflows/fuzz-nightly.yml:79`
- Modify: `.github/workflows/mutants-scheduled.yml:89`

These two workflows use the action directly (not via `setup-rust`).

- [ ] **Step 1: Pin `@nightly` in `fuzz-nightly.yml` (line 79)**

Change:

```yaml
        uses: dtolnay/rust-toolchain@nightly
```

to:

```yaml
        uses: dtolnay/rust-toolchain@5b842231ba77f5c045dba54ac5560fed2db780e2  # nightly
```

- [ ] **Step 2: Pin `@stable` in `mutants-scheduled.yml` (line 89)**

Change:

```yaml
      - uses: dtolnay/rust-toolchain@stable
```

to:

```yaml
      - uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8  # stable
```

(Same `stable`-branch SHA as Task 2 Step 1 — consistent across the repo.)

- [ ] **Step 3: Verify no bare dtolnay tag refs remain anywhere**

Run: `grep -rnE 'dtolnay/rust-toolchain@(stable|nightly|master)$' .github/`
Expected: no output (exit 1).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/fuzz-nightly.yml .github/workflows/mutants-scheduled.yml
git commit -m "ci: SHA-pin direct dtolnay/rust-toolchain refs in fuzz + mutants workflows (G2)"
```

---

### Task 5: Add `timeout-minutes` to every job in `ci.yml`

**Files:**
- Modify: `.github/workflows/ci.yml` (17 jobs)

`ci.yml` currently has ZERO job-level `timeout-minutes`, so every job rides GitHub's 6-hour
default. Add a `timeout-minutes:` line to each job, placed **immediately after that job's
`runs-on:` line** (matching where the existing timeouts sit in `tier2.yml`/`codeql.yml`), using
the same indentation as `runs-on:` (4 spaces). Values are generous-but-bounded: well above each
job's real duration (Tier 1 totals ~5-7 min warm; cold-cache compile jobs run longer), far
below the 6h default. Reference precedents already in the repo: `ci-summary` 90, `codeql` 60,
`tier2` coverage 30.

Per-job values:

| Job | `runs-on` line | `timeout-minutes` |
|---|---|---|
| `changes` | `ci.yml:73` | `10` |
| `fmt` | `ci.yml:113` | `15` |
| `clippy` | `ci.yml:125` | `30` |
| `cargo-deny` | `ci.yml:138` | `15` |
| `cargo-audit` | `ci.yml:151` | `15` |
| `osv-scanner` | `ci.yml:162` | `15` |
| `gitleaks` | `ci.yml:172` | `15` |
| `cargo-check-macos` | `ci.yml:186` | `30` |
| `cargo-check-windows` | `ci.yml:198` | `30` |
| `test-stable` | `ci.yml:209` | `45` |
| `doc-tests` | `ci.yml:227` | `30` |
| `msrv-check` | `ci.yml:248` | `30` |
| `test-fixtures-ports` | `ci.yml:261` | `20` |
| `feature-flag-matrix` | `ci.yml:280` | `30` |
| `runtime-core-no-std` | `ci.yml:299` | `20` |
| `build-fixtures-linux` | `ci.yml:336` | `30` |
| `build-checks-linux` | `ci.yml:390` | `30` |

- [ ] **Step 1: Insert `timeout-minutes` after each job's `runs-on:`**

For each job, insert the line directly below its `runs-on:`. Example — `changes` becomes:

```yaml
    name: detect changes
    runs-on: ubuntu-latest
    timeout-minutes: 10
    outputs:
```

`fmt` becomes:

```yaml
  fmt:
    name: rustfmt
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
```

…and so on for all 17 jobs per the table above. Where a job has `strategy:` after `runs-on:`
(`test-stable`, `build-fixtures-linux` has `needs:` after `runs-on:`), still place
`timeout-minutes:` on the line immediately following `runs-on:`. For `build-fixtures-linux` the
order is `needs: changes` (line 334) → `if:` (335) → `runs-on:` (336); insert after `runs-on:`:

```yaml
    needs: changes
    if: needs.changes.outputs.skip_heavy_jobs != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
```

- [ ] **Step 2: Verify every job got a timeout**

Run: `grep -c 'timeout-minutes:' .github/workflows/ci.yml`
Expected: `17`.

Run: `grep -c '  runs-on:' .github/workflows/ci.yml`
Expected: `17` (one `runs-on` per job → one timeout per job; counts match).

- [ ] **Step 3: Validate YAML parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('OK')"`
Expected: `OK` (if PyYAML is absent, skip — CI parse is authoritative).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: bound every ci.yml job with timeout-minutes (G7)"
```

---

### Task 6: Add `timeout-minutes` to the step-based jobs in `release.yml`

**Files:**
- Modify: `.github/workflows/release.yml` (6 jobs)

**Do NOT add `timeout-minutes` to `preflight-tier1` (line 27) or `preflight-tier2` (line 34)** —
they are reusable-workflow-call jobs (`uses: ./.github/workflows/…`); GitHub rejects
`timeout-minutes` on those (their timeouts come from inside the called workflows, where Task 5
just added them to `ci.yml`, and `tier2.yml` already has its own).

Per-job values for the 6 step-based jobs:

| Job | `runs-on` line | `timeout-minutes` |
|---|---|---|
| `build-release-binaries` | `release.yml:42` | `45` |
| `sbom-rust` | `release.yml:81` | `30` |
| `sbom-aggregate` | `release.yml:102` | `20` |
| `attest` | `release.yml:120` | `15` |
| `changelog` | `release.yml:147` | `10` |
| `gh-release-create` | `release.yml:163` | `10` |

- [ ] **Step 1: Insert `timeout-minutes` after each step-based job's `runs-on:`**

Example — `build-release-binaries` (note `runs-on:` uses a matrix expression; the timeout line
goes right after it, before `strategy:`):

```yaml
    needs: [preflight-tier1, preflight-tier2]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 45
    strategy:
```

`sbom-rust`:

```yaml
    needs: [preflight-tier1, preflight-tier2]
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
```

Repeat for `sbom-aggregate` (20), `attest` (15), `changelog` (10), `gh-release-create` (10),
each inserted immediately after its `runs-on:` line.

- [ ] **Step 2: Verify**

Run: `grep -c 'timeout-minutes:' .github/workflows/release.yml`
Expected: `6` (the two preflight jobs are intentionally excluded).

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('OK')"`
Expected: `OK` (skip if PyYAML absent).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: bound release.yml step jobs with timeout-minutes (G7)"
```

---

### Task 7: Add `timeout-minutes` to both jobs in `security-daily.yml`

**Files:**
- Modify: `.github/workflows/security-daily.yml` (2 jobs)

| Job | `runs-on` line | `timeout-minutes` |
|---|---|---|
| `audit` | `security-daily.yml:23` | `30` |
| `diff-and-file-issues` | `security-daily.yml:55` | `15` |

- [ ] **Step 1: Insert the timeouts**

`audit`:

```yaml
  audit:
    name: cargo audit + osv-scanner
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
```

`diff-and-file-issues`:

```yaml
  diff-and-file-issues:
    name: diff vs yesterday + file issues on new
    needs: audit
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
```

- [ ] **Step 2: Verify**

Run: `grep -c 'timeout-minutes:' .github/workflows/security-daily.yml`
Expected: `2`.

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/security-daily.yml')); print('OK')"`
Expected: `OK` (skip if PyYAML absent).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/security-daily.yml
git commit -m "ci: bound security-daily.yml jobs with timeout-minutes (G7)"
```

---

### Task 8: Repo-wide invariant check (no remaining mutable third-party refs)

**Files:** none (verification only)

- [ ] **Step 1: Confirm every third-party `uses:` is now a SHA**

Run:

```bash
grep -rhoE 'uses: [^ ]+@[^ ]+' .github/workflows .github/actions \
  | grep -vE 'uses: \./' \
  | grep -vE '@[0-9a-f]{40}$' | sort -u
```

Expected: no output. (Filters out local `./…` composite refs, which are correctly NOT pinned;
flags any remaining bare-tag third-party ref.)

- [ ] **Step 2: Confirm the eight target refs resolve to the expected SHAs**

Run: `grep -rn -E '(claude-code-action@28f83620|rust-toolchain@29eef336|rust-toolchain@5b842231|setup-mold@9c9c13bf|sccache-action@9e7fa8a1|install-action@7a79fe8c|rust-cache@e18b4977|download-artifact@3e5f45b2)' .github/ | wc -l`
Expected: `12` (claude ×3, dtolnay-stable ×2, dtolnay-nightly ×1, setup-mold ×1, sccache ×1, install-action ×1, rust-cache ×1, download-artifact ×1, plus the pre-existing reused pins in `mutants-scheduled.yml` and `docs-deploy.yml` for rust-cache/download-artifact). Inspect the output to confirm each is intentional.

---

## Verification (REQUIRED SUB-SKILL: superpowers:verification-before-completion)

Local greps + YAML parse are necessary but NOT sufficient. The brief mandates a real CI run.

- [ ] **Push the branch and open the PR (see "Finishing" below), then capture the live run.**

- [ ] **Confirm CI parses and runs green** — the `ci` matrix jobs and the `ci-summary` gate must
  pass. Because `setup-rust` (now carrying 5 freshly-pinned SHAs) is on the hot path of nearly
  every job, a bad SHA fails fast. Capture with:

```bash
gh pr checks <PR#> --watch
gh run view <run-id>            # confirm ci-summary = success
```

- [ ] **Confirm the timeouts are applied** by inspecting the rendered workflow on the PR head:

```bash
grep -c 'timeout-minutes:' .github/workflows/ci.yml            # 17
grep -c 'timeout-minutes:' .github/workflows/release.yml       # 6
grep -c 'timeout-minutes:' .github/workflows/security-daily.yml # 2
```

Evidence before assertions — do not claim done until the run is green.

## Code review (REQUIRED SUB-SKILL: superpowers:requesting-code-review)

Focus the review on: (a) no pin silently changed an action's MAJOR version — each new SHA maps
to the same version the ref resolved to before (cross-check the mapping table); (b) the
`@beta` replacement is a real released commit and the DECISION note (no v1.0.144 bump because it
breaks `direct_prompt`) holds; (c) the two reusable-workflow-call jobs in `release.yml` were
correctly left without `timeout-minutes`.

## Finishing (REQUIRED SUB-SKILL: superpowers:finishing-a-development-branch)

```bash
git push -u origin sha-pin-actions-hardening
gh pr create -R tau-rs/tau --base main \
  --title "ci: SHA-pin remaining actions + add timeout-minutes coverage (G2/G7)" \
  --body "<cite G2/G7, summarize the 8-row mapping table and the timeout coverage; note the claude-action @beta DECISION>"
```

STOP after opening the PR — no merge.

---

## Self-Review (completed during planning)

1. **Spec coverage** — brief items 1 (composite actions ×6: 5 in setup-rust + 1 in
   place-fixture-binaries) → Tasks 2+3; item 2 (claude-action ×3) → Task 1 (done first);
   item 3 (dtolnay ×2 in fuzz/mutants) → Task 4; item 4 (timeouts in ci/release/security-daily)
   → Tasks 5/6/7. All four covered.
2. **Placeholder scan** — every pin is a concrete 40-hex SHA from the verified mapping table;
   every timeout is a concrete integer; no TBD/TODO.
3. **Consistency** — reused SHAs (`rust-cache@e18b4977`, `download-artifact@3e5f45b2`) are
   byte-identical to refs already in the repo, guaranteeing same-version. `dtolnay@stable`
   SHA is identical in Task 2 and Task 4. Comment style is two-space `# <ref>` throughout.
