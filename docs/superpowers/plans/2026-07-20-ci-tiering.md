# CI Tiering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Shrink pre-merge CI to a thin ~10-job Tier 0 gate and make the nightly `tier2.yml` run the authority for trunk health, per `docs/superpowers/specs/2026-07-20-ci-tiering-design.md`.

**Architecture:** Pure GitHub Actions workflow surgery — no Rust changes. Jobs move verbatim from `ci.yml` into `tier2.yml` (gaining the `gate` guard), the standalone `coverage.yml` is deleted (tier2 already has a coverage job), and an ADR records the decision superseding the per-PR-full-matrix part of ADR-0039.

**Tech Stack:** GitHub Actions YAML, mdBook for docs validation.

## Global Constraints

- No local cargo runs are needed; if any diagnostic cargo command is run, follow repo CLAUDE.md: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo ...` with `-p <crate>`.
- Validate every edited workflow file with `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1]))" <file>` (actionlint is not installed).
- Commits: conventional, imperative, scoped. Guard identity per repo CLAUDE.md: `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit ...`.
- Docs: any new page must be added to `docs/SUMMARY.md`; run `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` before the PR; `rm -rf docs/book` afterwards.
- Branch: work on the current `tripoli` branch; PR base is `main`.

## Two approved deviations from the spec (rationale recorded here)

1. **`changes` job is deleted, not kept.** Its only consumer (`build-fixtures-linux`, via `skip_heavy_jobs`) leaves `ci.yml`, making it dead code. (Spec said "keep as plumbing" before this was known.)
2. **`ports-semver` stays in Tier 0, not tier2.** It runs `cargo semver-checks --baseline-rev origin/main`; on a nightly run of main HEAD the baseline equals the checked-out tree, so the check compares main to itself and can never fail — moving it would silently kill its signal. Pre-merge (PR/merge-queue) is the only place the baseline is meaningful.

End-state Tier 0 job set in `ci.yml` (10 jobs): `fmt`, `clippy`, `cargo-deny`, `gitleaks`, `cargo-check-macos`, `cargo-check-windows`, `test-stable`, `doc-tests`, `runtime-core-no-std`, `ports-semver`.

---

### Task 1: Slim `ci.yml` to Tier 0

**Files:**
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: a `CI` workflow containing exactly the 10 Tier 0 jobs above, same triggers (`push: main`, `pull_request`, `merge_group`, `workflow_call`), same concurrency/env/permissions blocks.
- Consumed by: `ci-summary.yml` (polls whole-run conclusion — name-insensitive, no change needed) and `release.yml` `preflight-tier1` (`uses: ./.github/workflows/ci.yml` — no change needed).

- [ ] **Step 1: Delete the moved/dead jobs from `ci.yml`**

Delete these 13 job blocks in their entirety (job key through last step, including each job's leading comment lines). Line numbers are from the current file; delete bottom-up so earlier ranges stay valid:

| Job key | Current lines |
|---|---|
| `schema-conformance` | 537–550 |
| `build-checks-linux` | 514–535 |
| `build-fixtures-linux` | 454–512 |
| `mock-sandbox-prod-gate` | 358–388 |
| `feature-flag-matrix` | 329–356 |
| `test-credential-chain` | 312–327 |
| `conformance` | 294–310 |
| `test-fixtures-ports` | 274–292 |
| `msrv-check` | 254–272 |
| `osv-scanner` | 165–176 |
| `cargo-audit` | 153–163 |
| `changes` | 51–110 |
| `wit-host-drift` | 552–565 |

Do NOT delete: `fmt`, `clippy`, `cargo-deny`, `gitleaks`, `cargo-check-macos`, `cargo-check-windows`, `test-stable`, `doc-tests`, `runtime-core-no-std`, `ports-semver`, or the trailing `ci-summary moved to its own workflow` comment.

Add a short comment at the top of the `jobs:` block explaining the tier:

```yaml
jobs:
  # Tier 0 — the thin pre-merge gate (trunk-based CI, ADR-00NN).
  # Full validation is the nightly tier2.yml run (authority for trunk
  # health, auto-bisect on red). `full-matrix` label pulls tier2 onto
  # a PR on demand; release.yml re-runs both tiers on tags.
```

(Replace `00NN` with the ADR number chosen in Task 4.)

- [ ] **Step 2: Verify no dangling references**

Run:
```bash
grep -n "needs:" .github/workflows/ci.yml
grep -rn "skip_heavy_jobs\|linux-fixture-binaries" .github/workflows/ docs/ --include="*.yml" --include="*.md" | grep -v tier2
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('yaml ok')"
```
Expected: no `needs:` lines remain in ci.yml (all 10 surviving jobs are independent); no references to `skip_heavy_jobs` or the ci.yml artifact `linux-fixture-binaries` outside tier2 (tier2 uses its own `tier2-linux-fixture-binaries`); `yaml ok`.

- [ ] **Step 3: Verify surviving job set**

Run: `grep -E '^  [a-z][a-z0-9_-]+:$' .github/workflows/ci.yml`
Expected output — exactly these 10 job keys (plus `push:`/`pull_request:`/`merge_group:` trigger keys near the top):
`fmt clippy cargo-deny gitleaks cargo-check-macos cargo-check-windows test-stable doc-tests runtime-core-no-std ports-semver`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "ci: slim ci.yml to a thin tier-0 pre-merge gate"
```

---

### Task 2: Absorb the moved jobs into nightly `tier2.yml`

**Files:**
- Modify: `.github/workflows/tier2.yml`

**Interfaces:**
- Consumes: tier2's existing `gate` job (`outputs.run`), its `env` block, and `./.github/actions/setup-rust`.
- Produces: 9 new tier2 jobs guarded by `needs: gate` + `if: needs.gate.outputs.run == 'true'`, all listed in `nightly-regression-handler.needs` so a nightly red on any of them opens the rolling issue and triggers auto-bisect.

- [ ] **Step 1: Insert the 9 moved jobs**

Insert the following YAML after the `test-tau-runtime-e2e` job (ends at line 324, right before `nightly-regression-handler`). These are the ci.yml blocks verbatim, each with the two gate lines added:

```yaml
  msrv-check:
    # MSRV is a rustc-version property, not an OS property. Moved from
    # per-PR ci.yml to nightly (ADR-00NN): breaks are rare and bisect
    # finds them trivially. release.yml re-asserts MSRV on tags.
    name: msrv-check / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: "1.91"
          shared-key: linux-1.91
          with-sccache: true
          with-mold: true
      - run: cargo check --workspace --all-targets --locked

  test-fixtures-ports:
    name: test-fixtures-ports / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - name: Test tau-ports (test-fixtures feature only)
        run: cargo nextest run --profile ci -p tau-ports --features test-fixtures
      - name: Doctests tau-ports (test-fixtures feature only)
        # Workspace-level `cargo test --workspace --doc` (ci.yml doc-tests)
        # runs doctests without features; the fixtures module is gated
        # behind `test-fixtures`, so its doctests only compile here.
        run: cargo test --doc -p tau-ports --features test-fixtures

  conformance:
    # β.6 conformance gate on the tau-conformance crate. Distinct from
    # test-conformance above (plugin conformance suites).
    name: conformance / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - name: Run β.6 conformance gate (dev profile)
        # The #[ignore]'d wasm-profile test is skipped by nextest until
        # β.7.5 ships `tau build wasm`. See ADR-0048.
        run: cargo nextest run --profile ci -p tau-conformance

  test-credential-chain:
    name: test-credential-chain / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - name: Credential port tests (tau-ports)
        run: cargo nextest run --profile ci -p tau-ports -E 'test(/credential/)'
      - name: Credential provider tests (tau-runtime-tokio)
        run: cargo nextest run --profile ci -p tau-runtime-tokio -E 'test(/credentials/)'

  feature-flag-matrix:
    name: feature-flag-matrix / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: Check each crate with --no-default-features
        run: |
          set -e
          for crate in tau-domain tau-ports tau-pkg tau-runtime-tokio tau-cli tau-plugin-protocol tau-plugin-sdk tau-mcp tau-observe tau-native-tools; do
            echo "::group::$crate"
            cargo check -p "$crate" --no-default-features
            echo "::endgroup::"
          done
      - name: tau-ports feature quadrants (serde / process independence)
        # `process = ["serde?/std"]` in crates/tau-ports/Cargo.toml makes serde an
        # OPTIONAL dep of process. Verify each feature builds WITHOUT the other so
        # a consumer that enables only one never hits a missing-item error that the
        # default (both-on) matrix would mask.
        run: |
          set -e
          cargo check -p tau-ports --no-default-features --features serde
          cargo check -p tau-ports --no-default-features --features process

  mock-sandbox-prod-gate:
    name: mock-sandbox absent from default tau binary / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: MockSandbox must not be reachable in a default tau build
        # The `mock-sandbox` feature (tau-runtime-tokio) makes the
        # `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` capability-gate bypass reachable.
        # It is enabled ONLY via dev-dependencies, so under resolver v2 a
        # default `cargo build` of the `tau` binary must contain neither the
        # mock code path nor the env-var string literal. A hit here means the
        # feature leaked into the production dependency graph (e.g. moved from a
        # dev-dep to a normal dep) or the
        # `#[cfg(any(feature = "mock-sandbox", test))]` gates in
        # process_gate::resolver were dropped.
        run: |
          set -euo pipefail
          cargo build -p tau-cli
          BIN=target/debug/tau
          if strings "$BIN" | grep -q 'TAU_TESTING_ALLOW_MOCK_SANDBOX'; then
            echo "::error::mock-sandbox leaked into the default tau binary — the TAU_TESTING_ALLOW_MOCK_SANDBOX gate is reachable in production" >&2
            strings "$BIN" | grep 'TAU_TESTING_ALLOW_MOCK_SANDBOX' >&2 || true
            exit 1
          fi
          echo "OK: no mock-sandbox env gate in the default tau binary"

  build-checks-linux:
    name: build-checks / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - name: Build tau-plugin-test-support
        run: cargo build -p tau-plugin-test-support
      - name: Test tau-plugin-test-support
        run: cargo nextest run --profile ci -p tau-plugin-test-support --all-targets
      - name: Build tau-plugin-conformance
        run: cargo build -p tau-plugin-conformance
      - name: Build tau-plugin-compat
        run: cargo build -p tau-plugin-compat
      - name: Build tau-plugin-compat (integration-tests feature)
        run: cargo build -p tau-plugin-compat --features integration-tests --tests

  schema-conformance:
    name: IR schema (drift + conformance)
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: Drift check + conformance kit validation
        run: cargo test -p tau-ir --features schema --test schema_export --test schema_conformance

  wit-host-drift:
    name: WIT host world (drift + freeze)
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0  # v7.0.0
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: WIT host world drift + freeze test
        run: cargo test -p tau-wasm-host --test wit_host_drift
```

(Replace `00NN` in the msrv-check comment with the ADR number chosen in Task 4.)

- [ ] **Step 2: Extend `nightly-regression-handler.needs`**

In the `nightly-regression-handler` job, replace the `needs:` list with:

```yaml
    needs:
      - nextest-macos
      - nextest-windows
      - coverage
      - test-conformance
      - test-tau-plugin-compat
      - test-tau-plugin-compat-layer4-ignored
      - test-tau-sandbox-native-e2e
      - test-tau-runtime-e2e
      - msrv-check
      - test-fixtures-ports
      - conformance
      - test-credential-chain
      - feature-flag-matrix
      - mock-sandbox-prod-gate
      - build-checks-linux
      - schema-conformance
      - wit-host-drift
```

- [ ] **Step 3: Validate**

Run:
```bash
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/tier2.yml')); print(sorted(d['jobs'].keys()))"
```
Expected: job list includes all 9 new keys plus the 10 pre-existing ones (`gate`, `build-fixtures-linux`, `nextest-macos`, `nextest-windows`, `coverage`, `test-conformance`, `test-tau-plugin-compat`, `test-tau-plugin-compat-layer4-ignored`, `test-tau-sandbox-native-e2e`, `test-tau-runtime-e2e`, `nightly-regression-handler`, `auto-bisect`) — 21 total; no YAML error.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/tier2.yml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "ci: absorb moved tier-1 jobs into nightly tier2 (authority for trunk health)"
```

---

### Task 3: Delete standalone `coverage.yml`

**Files:**
- Delete: `.github/workflows/coverage.yml`
- Modify: `.github/workflows/release.yml` (comment only, lines 20–22)

**Interfaces:**
- Consumes: nothing. tier2's existing `coverage` job (nightly + label + release preflight) is the sole remaining coverage lane; codecov upload continues from there.

- [ ] **Step 1: Delete the workflow and fix the stale comment**

```bash
git rm .github/workflows/coverage.yml
```

In `.github/workflows/release.yml`, replace:

```
# Coverage is NOT duplicated here — it is owned by the separate
# coverage.yml lane (measurement-not-gating); preflight-tier2 also
# re-runs it against the tag.
```

with:

```
# Coverage is NOT duplicated here — it is owned by tier2.yml's
# nightly coverage job (measurement-not-gating); preflight-tier2
# re-runs it against the tag.
```

- [ ] **Step 2: Verify no remaining references**

Run: `grep -rn "coverage.yml" .github/ docs/ README.md 2>/dev/null | grep -v superpowers`
Expected: no hits (spec/plan files under `docs/superpowers/` are allowed to mention it historically).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "ci: delete standalone coverage workflow (nightly tier2 owns coverage)"
```

---

### Task 4: ADR + docs

**Files:**
- Create: `docs/decisions/00NN-ci-tiering-nightly-authority.md` (NN chosen in Step 1)
- Modify: `docs/decisions/0039-ci-strategy.md` (status header)
- Modify: `docs/SUMMARY.md` (one line after the ADR-0060 entry, line 139)
- Modify: `.github/workflows/ci.yml`, `.github/workflows/tier2.yml` (replace the `00NN` placeholders from Tasks 1–2)

**Interfaces:**
- Consumes: ADR template at `docs/decisions/template.md`.

- [ ] **Step 1: Pick the ADR number**

```bash
git fetch origin main
git ls-tree origin/main docs/decisions/ --name-only | sort | tail -3
gh pr list --state open --json number --jq '.[].number' | \
  xargs -I{} sh -c 'gh pr diff {} --name-only 2>/dev/null | grep -o "docs/decisions/00[0-9]*" || true' | sort -u
```
Take the lowest number not present on main AND not claimed by any open PR (expected: 0061 or 0062). Use it everywhere `00NN` appears below and in Tasks 1–2.

- [ ] **Step 2: Write the ADR**

Create `docs/decisions/00NN-ci-tiering-nightly-authority.md` (follow the structure of `docs/decisions/0039-ci-strategy.md`):

```markdown
# ADR-00NN: CI tiering — thin pre-merge gate, nightly authority for trunk health

**Status:** Accepted
**Date:** 2026-07-20
**Supersedes:** ADR-0039 (in part — the per-PR job set of Tier 1)

## Context

ADR-0039's three-tier model still ran the full 23-job matrix plus a
15–30 min standalone coverage workflow on every PR push, again in the
merge queue, and again on push to main — School-1 ("pre-merge
authority") cost, while the repo already owned School-2 ("post-merge
authority") machinery: nightly tier2 with auto-bisect,
auto-rerun-flaky, security-daily, a merge queue, and release preflight
that re-runs both tiers via workflow_call.

## Decision

Adopt trunk-based post-merge authority:

- **Tier 0 (pre-merge + merge queue + main push)** — `ci.yml` shrinks
  to 10 fast jobs: fmt, clippy, cargo-deny, gitleaks, cargo-check
  (macos + windows), test-stable, doc-tests, runtime-core-no-std,
  ports-semver. Target ≤ ~8 min wall clock. `ci-summary` remains the
  only required check.
- **Nightly (authority)** — the remaining ex-Tier-1 jobs move into
  `tier2.yml` (msrv-check, test-fixtures-ports, conformance,
  test-credential-chain, feature-flag-matrix, mock-sandbox-prod-gate,
  build-checks-linux, schema-conformance, wit-host-drift), covered by
  the nightly cron, the `full-matrix` PR label, and release preflight.
  The standalone coverage workflow is deleted; tier2's coverage job is
  the only coverage lane.
- **Contract** — a red nightly on main is a stop-the-line event: the
  first action next session is fix or revert. `auto-bisect` posts the
  culprit commit to the rolling regression issue.

Deliberate placements:

- `runtime-core-no-std` stays pre-merge: the feature-graph/no_std-link
  regression class fires often and hard-blocks the wasm epic.
- `ports-semver` stays pre-merge: its `--baseline-rev origin/main`
  compares main to itself on a nightly run and can never fail there —
  the check only has signal before merge.
- `cargo-audit`/`osv-scanner` are deleted from the PR path outright:
  advisory risk is time-triggered and `security-daily.yml` already
  runs both every day.

## Consequences

- PR feedback drops from ~23 jobs + coverage × (push + queue + main)
  to 10 fast jobs per event.
- Platform-specific *test* failures (macOS/Windows nextest) and the
  moved contract gates are now discovered nightly, not pre-merge;
  cargo-check on both platforms stays pre-merge to catch the
  compile-level cfg-root class. Risky PRs can opt back in with the
  `full-matrix` label.
- Releases are unaffected: `release.yml` preflight re-runs ci.yml and
  tier2.yml (now including the moved jobs) on the tag.
```

- [ ] **Step 3: Mark ADR-0039 superseded-in-part and index the new ADR**

In `docs/decisions/0039-ci-strategy.md`, change the header block:

```markdown
**Status:** Accepted (superseded in part by ADR-00NN — per-PR Tier 1 job set)
**Date:** 2026-06-09
**Supersedes:** —
```

In `docs/SUMMARY.md`, after the ADR-0060 line (line 139), add:

```markdown
- [ADR-00NN — CI tiering: thin pre-merge gate, nightly authority](decisions/00NN-ci-tiering-nightly-authority.md)
```

- [ ] **Step 4: Replace the `00NN` placeholders left in Tasks 1–2**

```bash
grep -rn "00NN" .github/workflows/ci.yml .github/workflows/tier2.yml
```
Edit both hits to the chosen number. Expected after: `grep -rn "00NN" .github/workflows/` → no hits.

- [ ] **Step 5: Build the book**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book
```
Expected: only `[INFO]` lines; linkcheck passes (warning-policy = error).

- [ ] **Step 6: Commit**

```bash
git add docs/decisions/ docs/SUMMARY.md .github/workflows/ci.yml .github/workflows/tier2.yml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(adr): ADR-00NN CI tiering — thin pre-merge gate, nightly authority"
```

Also commit the spec + this plan if not yet committed:

```bash
git add docs/superpowers/specs/2026-07-20-ci-tiering-design.md docs/superpowers/plans/2026-07-20-ci-tiering.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(superpowers): CI tiering spec + plan"
```

---

### Task 5: PR + live validation

**Files:** none (GitHub operations).

**Interfaces:**
- Consumes: the three workflow commits; `ci-summary` required check; tier2 `full-matrix` label path.

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin tripoli
gh pr create --base main \
  --title "ci: trunk-based tiering — thin tier-0 gate, nightly tier2 authority (ADR-00NN)" \
  --body "$(cat <<'EOF'
Implements docs/superpowers/specs/2026-07-20-ci-tiering-design.md.

- ci.yml → 10-job Tier 0 (fmt, clippy, cargo-deny, gitleaks, check-macos/windows, test-stable, doc-tests, runtime-core-no-std, ports-semver)
- 9 jobs moved into nightly tier2.yml (covered by cron + full-matrix label + release preflight); nightly-regression-handler needs-list extended so auto-bisect covers them
- standalone coverage.yml deleted (tier2 coverage job is the only lane)
- cargo-audit/osv-scanner dropped from the PR path (security-daily already runs both daily)
- ADR-00NN; ADR-0039 superseded in part

Contract: red nightly on main = fix-or-revert first thing next session.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: Validate tier2's new jobs on this PR via the label**

```bash
gh pr edit --add-label full-matrix
```
Expected: a "Tier 2 — Heavy validation" run starts on the PR (label trigger) and the 9 new jobs appear in it. This proves the moved jobs are green in their new home *before* merge — no post-merge `workflow_dispatch` needed.

- [ ] **Step 3: Watch checks**

```bash
gh pr checks --watch
```
Expected: `ci-summary` green (Tier 0 passed); tier2 run green including the 9 new jobs. If a moved job fails here, fix in place — it ran identically pre-move, so a failure is a porting error (missing setup-rust input, indentation).

- [ ] **Step 4: Enrol auto-merge**

```bash
gh pr merge --squash --auto
```
Expected: merges when checks pass and branch is up-to-date (`gh pr update-branch` if BEHIND).

- [ ] **Step 5: Post-merge sanity (next nightly)**

After the next 04:00 UTC cron: confirm the tier2 run on main includes and passes the 9 new jobs (`gh run list --workflow tier2.yml --limit 1`). This is the first authoritative nightly under the new contract.
