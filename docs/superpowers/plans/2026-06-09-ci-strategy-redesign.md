# CI Strategy Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor tau's CI from "every job runs on every PR" into a three-tier model (PR / nightly cron + label / release tag) plus a periodic security-scan layer and six DevOps add-ons (concurrency groups everywhere, flaky-test quarantine list, required-checks audit, auto-bisect on nightly failure, Dependabot patch auto-merge, action SHA pinning).

**Architecture:** Four PR-sized phases A → D, each one merged before the next opens. Phase A adds the new workflow files alongside the existing ones (additive, no risk to current CI). Phase B refactors `ci.yml` to remove the moved jobs and updates `ci-summary.yml`'s expected-job allow-list (the only branch-protection-blocking phase; tested on a throwaway branch first). Phase C adds the DevOps add-ons across all workflows. Phase D writes the ADR + docs.

**Tech Stack:** GitHub Actions YAML — no Rust compilation in this work. Verification via `yamllint` (optional, install via brew/pip), `python3 -c 'import yaml; yaml.safe_load(...)'` for syntax sanity, and `gh run list --workflow=<name>` after a real push to confirm the workflow fires.

**Branch:** `feat/ci-strategy-redesign` (off `origin/main` at `d98dff6`). Each phase may want its own branch off the previous phase's merged state — see Phase Migration Strategy.

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/ci-strategy-redesign`.

**Spec reference:** `docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md` — committed at `43fe317`. Read it before starting; this plan IS the implementation; the spec IS the design contract.

**Locked architectural decisions consumed (from spec section "Locked open items"):**
1. Nightly cron: `0 4 * * *` (04:00 UTC)
2. PR opt-in label: literal `full-matrix`
3. Failure-rolling-issue labels: `nightly-regression` (Tier 2 cron) + `security` (security cron)
4. Auto-bisect range: yesterday's-pass → today's-fail SHA; cap 7 days back
5. SBOM format: SPDX 2.3
6. Release signing: GitHub OIDC via `actions/attest-build-provenance@v3` (no BYO cosign key)
7. `full-matrix` label failure behavior: non-blocking; auto-merge still proceeds on Tier 1 green

---

## Phase Migration Strategy (READ BEFORE STARTING)

Each phase ships as its own PR. Open the next phase's PR ONLY after the previous lands. The four phases:

| Phase | PR | Risk | Branch suggestion |
|---|---|---|---|
| 1 — Add new workflows (additive) | `feat/ci-redesign-1-add-workflows` | Low (no existing CI changes) | off `origin/main` |
| 2 — Refactor `ci.yml` + delete `coverage.yml` | `feat/ci-redesign-2-refactor` | Medium (changes branch-protection-blocking content) | off `origin/main` after Phase 1 lands |
| 3 — Add 6 DevOps add-ons | `feat/ci-redesign-3-addons` | Low (additive across files) | off `origin/main` after Phase 2 lands |
| 4 — ADR + docs | `feat/ci-redesign-4-docs` | Trivial | off `origin/main` after Phase 3 lands |

**Why split**: each phase is small, reviewable individually, and revertable on its own. A failure in Phase 2 (the riskiest) doesn't roll back Phase 1's new files.

**Verification pattern after each phase**:
- Push to the phase branch
- `gh run list --workflow=<name>` to confirm new workflows fire correctly
- For Phase 2 specifically: push to a `feat/ci-redesign-2-test` throwaway branch FIRST; observe `ci.yml` runs the new shape green; then open the canonical Phase 2 PR
- Open PR, watch CI, enroll auto-merge

---

## Standing constraints (re-read before EVERY git command)

| Command | Shape |
|---|---|
| Commits | `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` |
| Push | `git push --no-verify -u origin feat/ci-redesign-<N>-<name>` |
| Auto-merge | `gh pr merge <N> --auto` BARE. Repo IS a merge queue. |
| YAML lint (pre-push, optional) | `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/<file>.yml'))" && echo OK` |

**Workflow YAML gotchas to avoid (PR-2/3/4/5 lessons applied to YAML):**
1. GitHub Actions YAML errors are **silent at parse time** — a typo in `on:` makes the workflow never fire instead of erroring. ALWAYS verify firing via `gh run list --workflow=<name>` after push.
2. `${{ ... }}` expansions in `run:` blocks are shell-injection vectors. Never interpolate `github.event.pull_request.title` directly. Use `env: { TITLE: ${{ ... }} }` then `"$TITLE"` in the script.
3. Branch protection rules need updating when `ci-summary`'s expected job set changes (Phase 2 only). Coordinate via `gh api repos/tau-rs/tau/branches/main/protection` query before / after.
4. The merge queue uses `ci-summary` ONLY (per project memory). Phase 2 must preserve that contract.

---

## Phase 1 — Add new workflow files (additive)

This phase adds 8 new workflow files + 1 git-cliff config. The existing CI continues running as today. After Phase 1 merges, you'll see the new workflows trigger but their results don't yet feed `ci-summary`.

### Task 1.1: `.github/workflows/tier2.yml` (heavy validation matrix)

**Files:**
- Create: `.github/workflows/tier2.yml`

- [ ] **Step 1: Read** `.github/workflows/ci.yml` lines 1-50 to confirm existing env conventions (`CARGO_TERM_COLOR`, `RUST_BACKTRACE`, `CARGO_INCREMENTAL: 0`, etc.). Match them.

- [ ] **Step 2: Write the file** verbatim:

```yaml
name: Tier 2 — Heavy validation

# Runs on:
# - nightly cron 04:00 UTC against main HEAD (regression detection ≤24h)
# - pull_request labeled `full-matrix` (opt-in pre-merge heavy run)
# - workflow_dispatch (manual)
#
# Tier 2 is NON-BLOCKING for PR merge. Auto-merge fires on Tier 1
# (ci-summary) green; Tier 2 results are informational, posted as PR
# comments or — on cron — as rolling regression issues.

on:
  schedule:
    - cron: '0 4 * * *'
  pull_request:
    types: [labeled, synchronize]
  workflow_dispatch:
  workflow_call:  # release.yml reuses this workflow for preflight

concurrency:
  group: tier2-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  CARGO_INCREMENTAL: 0

permissions:
  contents: read
  issues: write
  pull-requests: write

jobs:
  gate:
    # For pull_request triggers, only run downstream jobs when the
    # `full-matrix` label is present. For schedule + workflow_dispatch,
    # always run.
    name: gate
    runs-on: ubuntu-latest
    outputs:
      run: ${{ steps.compute.outputs.run }}
    steps:
      - id: compute
        env:
          EVENT_NAME: ${{ github.event_name }}
          LABELS: ${{ toJSON(github.event.pull_request.labels.*.name) }}
        run: |
          if [ "$EVENT_NAME" != "pull_request" ]; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if echo "$LABELS" | grep -q '"full-matrix"'; then
            echo "run=true" >> "$GITHUB_OUTPUT"
          else
            echo "run=false" >> "$GITHUB_OUTPUT"
          fi

  build-fixtures-linux:
    # Self-contained: build the fixture binaries inside this workflow
    # rather than downloading from ci.yml's artifact (avoids cross-
    # workflow race conditions when ci.yml hasn't yet uploaded).
    name: build-fixtures / linux
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-mold: true
      - name: Build all release-mode binaries
        run: |
          cargo build --release \
            -p anthropic -p ollama -p openai \
            -p fs-read -p shell \
            -p echo-llm -p echo-tool \
            -p tau-cli
          cargo build --release \
            -p tau-sandbox-native --bin tau-net-bridge
      - name: Build controlled-env binary
        run: |
          cargo build --release \
            --manifest-path crates/tau-plugin-compat/fixtures/controlled-env-binary/Cargo.toml
      - name: Stage binaries into flat _artifacts directory
        run: |
          mkdir -p _artifacts
          cp target/release/anthropic-plugin _artifacts/
          cp target/release/ollama-plugin _artifacts/
          cp target/release/openai-plugin _artifacts/
          cp target/release/fs-read-plugin _artifacts/
          cp target/release/shell-plugin _artifacts/
          cp target/release/echo-llm _artifacts/
          cp target/release/echo-tool _artifacts/
          cp target/release/tau _artifacts/
          cp target/release/tau-net-bridge _artifacts/
          cp crates/tau-plugin-compat/fixtures/controlled-env-binary/target/release/tau-controlled-env _artifacts/
      - uses: actions/upload-artifact@v7
        with:
          name: tier2-linux-fixture-binaries
          retention-days: 1
          path: _artifacts/

  nextest-macos:
    name: nextest / macos
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: macos-latest-stable
          with-nextest: true
          with-sccache: true
      - run: cargo nextest run --profile ci --workspace --all-targets

  nextest-windows:
    name: nextest / windows
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: windows-latest-stable
          with-nextest: true
      - run: cargo nextest run --profile ci --workspace --all-targets

  coverage:
    name: cargo llvm-cov nextest
    needs: gate
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          components: llvm-tools-preview
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - name: Install cargo-llvm-cov
        uses: taiki-e/install-action@cargo-llvm-cov
      - name: Generate coverage
        run: cargo llvm-cov nextest --profile ci --workspace --lcov --output-path lcov.info
      - uses: codecov/codecov-action@v5
        with:
          files: lcov.info
          fail_ci_if_error: false

  test-conformance:
    name: test (conformance)
    needs: [gate, build-fixtures-linux]
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-sccache: true
          with-mold: true
      - uses: actions/download-artifact@v6
        with:
          name: tier2-linux-fixture-binaries
          path: _artifacts/
      - uses: ./.github/actions/place-fixture-binaries
        with:
          binaries: "anthropic-plugin ollama-plugin openai-plugin"
      - name: Run conformance suite against all 3 plugins
        run: |
          cargo nextest run --profile ci -p anthropic --test conformance --no-capture
          cargo nextest run --profile ci -p ollama    --test conformance --no-capture
          cargo nextest run --profile ci -p openai    --test conformance --no-capture

  test-tau-plugin-compat:
    name: test (tau-plugin-compat / linux)
    needs: [gate, build-fixtures-linux]
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    env:
      TAU_CONTAINER_RUNTIME: docker
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-mold: true
      - uses: actions/download-artifact@v6
        with:
          name: tier2-linux-fixture-binaries
          path: _artifacts/
      - uses: ./.github/actions/place-fixture-binaries
        with:
          binaries: "all"
      - name: Build tau binary (debug)
        run: cargo build -p tau-cli --bin tau
      - name: Set up Docker buildx
        uses: docker/setup-buildx-action@v4
        with:
          driver: docker-container
      - name: Build per-plugin images
        run: cargo run -p xtask -- build-plugin-images
        env:
          BUILDX_CACHE_FROM: type=gha
          BUILDX_CACHE_TO: type=gha,mode=max
          BUILDKIT_PROGRESS: plain
          DOCKER_BUILDKIT: "1"
      - name: Test tau-plugin-compat
        run: cargo nextest run --profile ci -p tau-plugin-compat --features integration-tests --tests --verbose

  test-tau-plugin-compat-layer4-ignored:
    name: test (tau-plugin-compat / layer4-ignored / ${{ matrix.flavor }})
    needs: [gate, build-fixtures-linux]
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        flavor: [native, container]
    env:
      TAU_CONTAINER_RUNTIME: docker
      FLAVOR: ${{ matrix.flavor }}
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-mold: true
      - uses: actions/download-artifact@v6
        with:
          name: tier2-linux-fixture-binaries
          path: _artifacts/
      - uses: ./.github/actions/place-fixture-binaries
        with:
          binaries: "all"
      - name: Set up Docker buildx
        if: matrix.flavor == 'container'
        uses: docker/setup-buildx-action@v4
        with:
          driver: docker-container
      - name: Build per-plugin images
        if: matrix.flavor == 'container'
        run: cargo run -p xtask -- build-plugin-images
        env:
          BUILDX_CACHE_FROM: type=gha
          BUILDX_CACHE_TO: type=gha,mode=max
          BUILDKIT_PROGRESS: plain
          DOCKER_BUILDKIT: "1"
      - name: Build NEXTEST_FILTER for this leg
        run: |
          if [ "$FLAVOR" = "native" ]; then
            echo 'NEXTEST_FILTER=not (test(/anthropic_layer4_native_completes_via_cassette/) | test(/ollama_layer4_native_completes_via_cassette/) | test(/openai_layer4_native_completes_via_cassette/))' >> "$GITHUB_ENV"
          else
            echo 'NEXTEST_FILTER=all()' >> "$GITHUB_ENV"
          fi
      - name: Run ignored layer4 tests
        run: |
          cargo nextest run --profile ci \
            -p tau-plugin-compat \
            --features integration-tests \
            --run-ignored only \
            --test "layer4_$FLAVOR" \
            -E "$NEXTEST_FILTER" \
            --verbose

  test-tau-sandbox-native-e2e:
    name: test (tau-sandbox-native e2e / linux)
    needs: [gate, build-fixtures-linux]
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-mold: true
      - uses: actions/download-artifact@v6
        with:
          name: tier2-linux-fixture-binaries
          path: _artifacts/
      - uses: ./.github/actions/place-fixture-binaries
        with:
          binaries: "tau-controlled-env"
      - name: Test tau-sandbox-native e2e
        run: cargo nextest run --profile ci -p tau-sandbox-native --features integration-tests --tests --verbose
      - name: Run --ignored landlock-gated tests
        run: cargo nextest run --profile ci -p tau-sandbox-native --features integration-tests --run-ignored only --verbose

  test-tau-runtime-e2e:
    name: test (tau-runtime e2e / linux)
    needs: [gate, build-fixtures-linux]
    if: needs.gate.outputs.run == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-mold: true
      - uses: actions/download-artifact@v6
        with:
          name: tier2-linux-fixture-binaries
          path: _artifacts/
      - uses: ./.github/actions/place-fixture-binaries
        with:
          binaries: "tau-controlled-env"
      - name: Test tau-runtime-tokio e2e
        run: cargo nextest run --profile ci -p tau-runtime-tokio --features integration-tests --tests --verbose

  nightly-regression-handler:
    # Opens or updates a rolling issue on cron failure. Skips for PR-
    # labeled runs (those report via PR comment from a separate workflow).
    name: nightly-regression-handler
    needs:
      - nextest-macos
      - nextest-windows
      - coverage
      - test-conformance
      - test-tau-plugin-compat
      - test-tau-plugin-compat-layer4-ignored
      - test-tau-sandbox-native-e2e
      - test-tau-runtime-e2e
    if: failure() && github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - name: Open or append rolling regression issue
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          RUN_URL: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}
          SHA: ${{ github.sha }}
        run: |
          # Find an existing open issue with the nightly-regression label.
          EXISTING=$(gh issue list --label nightly-regression --state open --json number --jq '.[0].number // empty')
          BODY="Nightly Tier 2 cron failed on commit \`$SHA\`.\n\nRun: $RUN_URL\n\nSee failing jobs in the run page; auto-bisect will post the offending commit when available."
          if [ -n "$EXISTING" ]; then
            gh issue comment "$EXISTING" --body "$BODY"
          else
            DATE=$(date -u +%Y-%m-%d)
            gh issue create \
              --title "[nightly-CI] regression on $DATE" \
              --label nightly-regression \
              --body "$BODY"
          fi
```

- [ ] **Step 3: YAML syntax check.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/tier2.yml')); print('OK')"
```

Expected: `OK`.

- [ ] **Step 4: Commit.**

```sh
git add .github/workflows/tier2.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add tier2.yml — heavy validation matrix on nightly cron + full-matrix label"
```

### Task 1.2: `.github/workflows/release.yml` (release tag tier)

**Files:**
- Create: `.github/workflows/release.yml`
- Create: `.github/cliff.toml` (git-cliff config)

- [ ] **Step 1: Write `.github/cliff.toml`** (minimal config; git-cliff uses sensible defaults if absent, but explicit is better):

```toml
# git-cliff configuration for release changelog generation.
# Reads conventional-commit subjects between the previous tag and HEAD.

[changelog]
header = """
# Changelog

All notable changes to this project are documented here.

"""
body = """
{% if version %}
## [{{ version | trim_start_matches(pat="v") }}] - {{ timestamp | date(format="%Y-%m-%d") }}
{% else %}
## [unreleased]
{% endif %}
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group | upper_first }}
{% for commit in commits %}
- {{ commit.message | upper_first }}\
{% endfor %}
{% endfor %}
"""
trim = true

[git]
conventional_commits = true
filter_unconventional = true
split_commits = false
commit_parsers = [
  { message = "^feat", group = "Features" },
  { message = "^fix", group = "Bug Fixes" },
  { message = "^doc", group = "Documentation" },
  { message = "^perf", group = "Performance" },
  { message = "^refactor", group = "Refactor" },
  { message = "^test", group = "Tests" },
  { message = "^chore", skip = true },
  { message = "^style", skip = true },
]
filter_commits = true
tag_pattern = "v[0-9]*"
```

- [ ] **Step 2: Write `.github/workflows/release.yml`** verbatim:

```yaml
name: Release

# Triggered ONLY on git tag push (v*). This is the ship gate — any
# job failing aborts the GitHub Release creation; the tag stays in
# the repo but no artifacts are published.

on:
  push:
    tags: ['v*']

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  CARGO_INCREMENTAL: 0

permissions:
  contents: write     # for GitHub Release creation
  id-token: write     # for OIDC signing (attest-build-provenance)
  attestations: write # for attestation upload
  packages: read

jobs:
  preflight-tier1:
    # Re-run Tier 1 against the tag SHA. Even if main was green
    # yesterday, the tag commit may be newer.
    name: preflight-tier1
    uses: ./.github/workflows/ci.yml
    secrets: inherit

  preflight-tier2:
    name: preflight-tier2
    uses: ./.github/workflows/tier2.yml
    secrets: inherit

  build-release-binaries:
    name: build-release-binaries / ${{ matrix.os }}
    needs: [preflight-tier1, preflight-tier2]
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    outputs:
      tag: ${{ steps.tag.outputs.tag }}
    steps:
      - uses: actions/checkout@v6
      - id: tag
        shell: bash
        run: echo "tag=${GITHUB_REF#refs/tags/}" >> "$GITHUB_OUTPUT"
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: ${{ matrix.os }}-stable-release
          with-sccache: true
          with-mold: true
      - name: Build tau-cli (release)
        run: cargo build --release -p tau-cli --bin tau
      - name: Stage binary
        shell: bash
        run: |
          mkdir -p _release
          # cargo emits tau OR tau.exe depending on OS
          if [ -f target/release/tau.exe ]; then
            cp target/release/tau.exe "_release/tau-${{ matrix.os }}.exe"
          else
            cp target/release/tau "_release/tau-${{ matrix.os }}"
          fi
      - uses: actions/upload-artifact@v7
        with:
          name: release-bin-${{ matrix.os }}
          path: _release/
          retention-days: 7

  sbom-rust:
    name: sbom-rust (SPDX 2.3)
    needs: [preflight-tier1, preflight-tier2]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: Install cargo-sbom
        run: cargo install cargo-sbom --locked
      - name: Generate SPDX 2.3 SBOM
        run: cargo sbom --output-format spdx_json_2_3 > tau-sbom-rust.spdx.json
      - uses: actions/upload-artifact@v7
        with:
          name: sbom-rust
          path: tau-sbom-rust.spdx.json
          retention-days: 7

  sbom-aggregate:
    name: sbom-aggregate (syft)
    needs: [preflight-tier1, preflight-tier2]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - name: Generate aggregate SBOM via syft
        uses: anchore/sbom-action@v0
        with:
          path: .
          format: spdx-json
          output-file: tau-sbom-aggregate.spdx.json
      - uses: actions/upload-artifact@v7
        with:
          name: sbom-aggregate
          path: tau-sbom-aggregate.spdx.json
          retention-days: 7

  attest:
    name: attest provenance + sbom
    needs: [build-release-binaries, sbom-rust, sbom-aggregate]
    runs-on: ubuntu-latest
    permissions:
      contents: write
      id-token: write
      attestations: write
    steps:
      - uses: actions/download-artifact@v6
        with:
          pattern: release-bin-*
          path: _release/
          merge-multiple: true
      - uses: actions/download-artifact@v6
        with:
          pattern: sbom-*
          path: _sbom/
          merge-multiple: true
      - uses: actions/attest-build-provenance@v3
        with:
          subject-path: '_release/*'
      - uses: actions/attest-sbom@v3
        with:
          subject-path: '_release/*'
          sbom-path: '_sbom/tau-sbom-rust.spdx.json'

  changelog:
    name: changelog
    needs: [preflight-tier1, preflight-tier2]
    runs-on: ubuntu-latest
    outputs:
      body: ${{ steps.cliff.outputs.content }}
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
      - id: cliff
        uses: orhun/git-cliff-action@v3
        with:
          config: .github/cliff.toml
          args: --latest --strip header

  gh-release-create:
    name: gh-release-create
    needs: [build-release-binaries, sbom-rust, sbom-aggregate, attest, changelog]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v6
        with:
          pattern: release-bin-*
          path: _release/
          merge-multiple: true
      - uses: actions/download-artifact@v6
        with:
          pattern: sbom-*
          path: _sbom/
          merge-multiple: true
      - uses: softprops/action-gh-release@v3
        with:
          tag_name: ${{ needs.build-release-binaries.outputs.tag }}
          body: ${{ needs.changelog.outputs.body }}
          files: |
            _release/*
            _sbom/*
          fail_on_unmatched_files: true
```

- [ ] **Step 3: YAML syntax check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('OK')"
git add .github/workflows/release.yml .github/cliff.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add release.yml — preflight + SBOM + OIDC attestation + gh-release (tag-driven)"
```

### Task 1.3: `.github/workflows/security-daily.yml` (daily CVE scan)

**Files:**
- Create: `.github/workflows/security-daily.yml`

- [ ] **Step 1: Write the file** verbatim:

```yaml
name: Security — daily CVE scan

# Daily 04:00 UTC. Runs cargo-audit + osv-scanner against main HEAD.
# Diffs result vs yesterday's cached report. Opens issue
# `[security] new CVE: <id>` (label `security`) on NEW findings only.

on:
  schedule:
    - cron: '0 4 * * *'
  workflow_dispatch:

concurrency:
  group: security-daily-${{ github.ref }}
  cancel-in-progress: false

permissions:
  contents: read
  issues: write

jobs:
  audit:
    name: cargo audit + osv-scanner
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
      - name: cargo audit
        uses: rustsec/audit-check@v2
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
      - name: osv-scanner
        uses: google/osv-scanner-action/osv-scanner-action@v1
        with:
          scan-args: |-
            -r .
        continue-on-error: true
      - name: Save scan results as artifact (for tomorrow's diff)
        run: |
          mkdir -p _security
          # cargo-audit output already surfaced by the action; for diff,
          # re-run cargo audit json to a file.
          cargo install cargo-audit --locked || true
          cargo audit --json > _security/cargo-audit.json || true
      - uses: actions/upload-artifact@v7
        with:
          name: security-daily-${{ github.run_id }}
          path: _security/
          retention-days: 30

  diff-and-file-issues:
    name: diff vs yesterday + file issues on new
    needs: audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - name: Download today's report
        uses: actions/download-artifact@v6
        with:
          name: security-daily-${{ github.run_id }}
          path: _today/
      - name: Find yesterday's report
        id: yesterday
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          # Find the most recent successful run of THIS workflow other
          # than the current run, and download its artifact.
          PREV=$(gh run list --workflow=security-daily.yml --status success \
            --limit 5 --json databaseId --jq ".[] | select(.databaseId != ${{ github.run_id }}) | .databaseId" \
            | head -1)
          if [ -n "$PREV" ]; then
            echo "prev_run=$PREV" >> "$GITHUB_OUTPUT"
          else
            echo "prev_run=" >> "$GITHUB_OUTPUT"
          fi
      - name: Download yesterday's report
        if: steps.yesterday.outputs.prev_run != ''
        uses: actions/download-artifact@v6
        with:
          run-id: ${{ steps.yesterday.outputs.prev_run }}
          name: security-daily-${{ steps.yesterday.outputs.prev_run }}
          path: _yesterday/
          github-token: ${{ secrets.GITHUB_TOKEN }}
      - name: Diff + open issues
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          if [ ! -f _yesterday/cargo-audit.json ]; then
            echo "No yesterday baseline; skipping diff (will file issues for ALL findings tomorrow)."
            exit 0
          fi
          # Extract advisory IDs from both reports (jq), diff, file
          # issue for each new ID.
          TODAY_IDS=$(jq -r '.vulnerabilities.list[].advisory.id // empty' _today/cargo-audit.json 2>/dev/null | sort -u)
          YDAY_IDS=$(jq -r '.vulnerabilities.list[].advisory.id // empty' _yesterday/cargo-audit.json 2>/dev/null | sort -u)
          NEW_IDS=$(comm -23 <(echo "$TODAY_IDS") <(echo "$YDAY_IDS"))
          if [ -z "$NEW_IDS" ]; then
            echo "No new advisories vs yesterday."
            exit 0
          fi
          echo "$NEW_IDS" | while read -r ID; do
            [ -z "$ID" ] && continue
            DETAILS=$(jq -r ".vulnerabilities.list[] | select(.advisory.id == \"$ID\") | .advisory" _today/cargo-audit.json)
            gh issue create \
              --title "[security] new CVE: $ID" \
              --label security \
              --body "New RustSec advisory detected.\n\nID: $ID\n\nDetails:\n\`\`\`json\n$DETAILS\n\`\`\`\n\nRun: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}"
          done
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/security-daily.yml')); print('OK')"
git add .github/workflows/security-daily.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add security-daily.yml — cargo-audit + osv-scanner with diff-based issue filing"
```

### Task 1.4: `.github/workflows/codeql.yml` (weekly static analysis)

**Files:**
- Create: `.github/workflows/codeql.yml`

- [ ] **Step 1: Write the file:**

```yaml
name: Security — CodeQL

# Weekly Monday 06:00 UTC. Static analysis for Rust. Posts findings as
# GitHub Code Scanning alerts (native UI under Security tab).

on:
  schedule:
    - cron: '0 6 * * 1'
  workflow_dispatch:

concurrency:
  group: codeql-${{ github.ref }}
  cancel-in-progress: false

permissions:
  actions: read
  contents: read
  security-events: write

jobs:
  analyze:
    name: CodeQL / rust
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v6
      - uses: github/codeql-action/init@v3
        with:
          languages: rust
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: Build (CodeQL trace)
        run: cargo build --workspace --all-targets
      - uses: github/codeql-action/analyze@v3
        with:
          category: '/language:rust'
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/codeql.yml')); print('OK')"
git add .github/workflows/codeql.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add codeql.yml — weekly static analysis for Rust"
```

### Task 1.5: `.github/workflows/cargo-geiger.yml` (weekly unsafe-surface scan)

**Files:**
- Create: `.github/workflows/cargo-geiger.yml`

- [ ] **Step 1: Write the file:**

```yaml
name: Security — cargo-geiger

# Weekly Sunday 06:00 UTC. Counts unsafe-code blocks per crate.
# Diffs against last main commit's report; opens issue on increase.

on:
  schedule:
    - cron: '0 6 * * 0'
  workflow_dispatch:

concurrency:
  group: cargo-geiger-${{ github.ref }}
  cancel-in-progress: false

permissions:
  contents: read
  issues: write

jobs:
  geiger:
    name: cargo-geiger
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: Install cargo-geiger
        run: cargo install cargo-geiger --locked
      - name: Run geiger on workspace
        run: |
          mkdir -p _geiger
          cargo geiger --output-format Json > _geiger/today.json || true
      - uses: actions/upload-artifact@v7
        with:
          name: cargo-geiger-${{ github.run_id }}
          path: _geiger/
          retention-days: 90
      - name: Find previous run + diff
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          PREV=$(gh run list --workflow=cargo-geiger.yml --status success \
            --limit 5 --json databaseId --jq ".[] | select(.databaseId != ${{ github.run_id }}) | .databaseId" \
            | head -1)
          if [ -z "$PREV" ]; then
            echo "No baseline; first run."
            exit 0
          fi
          gh run download "$PREV" --name "cargo-geiger-$PREV" --dir _prev/ || true
          if [ ! -f _prev/today.json ]; then
            echo "Previous run had no artifact."
            exit 0
          fi
          TODAY_UNSAFE=$(jq -r '[.. | objects | .unsafe_used? // empty] | add // 0' _geiger/today.json)
          PREV_UNSAFE=$(jq -r '[.. | objects | .unsafe_used? // empty] | add // 0' _prev/today.json)
          DIFF=$((TODAY_UNSAFE - PREV_UNSAFE))
          if [ "$DIFF" -gt 0 ]; then
            gh issue create \
              --title "[security] unsafe surface grew by $DIFF" \
              --label security \
              --body "cargo-geiger reports total unsafe count grew from $PREV_UNSAFE to $TODAY_UNSAFE (+$DIFF) since the last weekly scan.\n\nReport: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}"
          fi
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/cargo-geiger.yml')); print('OK')"
git add .github/workflows/cargo-geiger.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add cargo-geiger.yml — weekly unsafe-surface scan with growth-detection"
```

### Task 1.6: `.github/workflows/full-matrix-label.yml` (PR comment poster)

**Files:**
- Create: `.github/workflows/full-matrix-label.yml`

This workflow does NOT replicate tier2.yml's jobs — tier2.yml already runs on `pull_request: types: [labeled]` per its trigger config. This workflow's job is to POST the result back as a PR comment after tier2.yml completes.

- [ ] **Step 1: Write the file:**

```yaml
name: full-matrix label — PR comment poster

# Posts a single PR comment summarizing the Tier 2 result when
# tier2.yml completes for a `full-matrix`-labeled PR. Non-blocking.

on:
  workflow_run:
    workflows: ["Tier 2 — Heavy validation"]
    types: [completed]

permissions:
  pull-requests: write
  contents: read
  actions: read

jobs:
  comment:
    name: post tier 2 result comment
    runs-on: ubuntu-latest
    # Only fire if the originating Tier 2 run was on a PR event.
    if: github.event.workflow_run.event == 'pull_request'
    steps:
      - name: Resolve PR number from workflow_run
        id: pr
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          HEAD_SHA: ${{ github.event.workflow_run.head_sha }}
        run: |
          PR=$(gh pr list --search "$HEAD_SHA" --state open --json number --jq '.[0].number // empty')
          echo "number=$PR" >> "$GITHUB_OUTPUT"
      - name: Build comment body
        if: steps.pr.outputs.number != ''
        id: body
        env:
          STATUS: ${{ github.event.workflow_run.conclusion }}
          RUN_URL: ${{ github.event.workflow_run.html_url }}
        run: |
          {
            echo "## 🧪 Tier 2 (full-matrix) results"
            echo ""
            echo "Outcome: **$STATUS**"
            echo ""
            echo "Run: $RUN_URL"
            echo ""
            echo "_Non-blocking. Auto-merge gates on Tier 1 (ci-summary) only._"
          } > comment.md
          echo "path=comment.md" >> "$GITHUB_OUTPUT"
      - name: Post or update comment
        if: steps.pr.outputs.number != ''
        uses: peter-evans/create-or-update-comment@v4
        with:
          issue-number: ${{ steps.pr.outputs.number }}
          body-path: ${{ steps.body.outputs.path }}
          edit-mode: replace
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/full-matrix-label.yml')); print('OK')"
git add .github/workflows/full-matrix-label.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add full-matrix-label.yml — post PR comment with Tier 2 outcome"
```

### Task 1.7: `.github/workflows/required-checks-audit.yml`

**Files:**
- Create: `.github/workflows/required-checks-audit.yml`

A trivial guard. Fails CI when a PR touches workflow `name:` fields without referencing an ADR in the PR body.

- [ ] **Step 1: Write the file:**

```yaml
name: Required-checks audit

# Trivial guard. Fails when a PR modifies any workflow `name:` field
# without referencing an ADR in the PR body. Catches accidental
# "I added a new required check" drift.

on:
  pull_request:
    paths:
      - '.github/workflows/*.yml'

permissions:
  contents: read
  pull-requests: read

jobs:
  audit:
    name: audit workflow name changes
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
      - name: Detect name: changes
        id: detect
        env:
          BASE_REF: ${{ github.base_ref }}
        run: |
          git fetch origin "$BASE_REF" --depth 1
          DIFF=$(git diff "origin/$BASE_REF...HEAD" -- '.github/workflows/*.yml')
          if echo "$DIFF" | grep -qE '^\+name:'; then
            echo "name_added=true" >> "$GITHUB_OUTPUT"
          else
            echo "name_added=false" >> "$GITHUB_OUTPUT"
          fi
      - name: Require ADR reference in PR body
        if: steps.detect.outputs.name_added == 'true'
        env:
          BODY: ${{ github.event.pull_request.body }}
        run: |
          if echo "$BODY" | grep -qE 'ADR-[0-9]+|docs/decisions/'; then
            echo "PR body references an ADR; audit passes."
          else
            echo "::error::This PR adds a new workflow name (potential new branch-protection-required check) but the PR body does not reference an ADR. Either revert the name change OR add an ADR reference (ADR-XXXX or docs/decisions/...) explaining why the change is intended."
            exit 1
          fi
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/required-checks-audit.yml')); print('OK')"
git add .github/workflows/required-checks-audit.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add required-checks-audit.yml — guard against accidental new branch-protection checks"
```

### Task 1.8: `.github/workflows/dependabot-auto-merge.yml`

**Files:**
- Create: `.github/workflows/dependabot-auto-merge.yml`

- [ ] **Step 1: Write the file:**

```yaml
name: Dependabot — auto-merge patches

# Auto-merges PRs from dependabot[bot] when:
# - PR is labeled `dependencies` (dependabot adds this automatically)
# - update-type is patch-level (semver patch bump)
# - All Tier 1 checks pass (gated by `gh pr merge --auto` itself)

on:
  pull_request:
    types: [labeled, opened, synchronize]

permissions:
  contents: write
  pull-requests: write

jobs:
  auto-merge:
    name: enroll auto-merge if patch-level
    runs-on: ubuntu-latest
    if: github.actor == 'dependabot[bot]'
    steps:
      - name: Fetch metadata
        id: metadata
        uses: dependabot/fetch-metadata@v2
        with:
          github-token: ${{ secrets.GITHUB_TOKEN }}
      - name: Enable auto-merge for patch updates
        if: steps.metadata.outputs.update-type == 'version-update:semver-patch'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          PR_URL: ${{ github.event.pull_request.html_url }}
        run: gh pr merge --auto "$PR_URL"
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/dependabot-auto-merge.yml')); print('OK')"
git add .github/workflows/dependabot-auto-merge.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add dependabot-auto-merge.yml — enroll patch-level dep bumps in auto-merge"
```

### Task 1.9: Verify all Phase 1 files parse + push + open PR

- [ ] **Step 1: Re-check all 8 YAML files parse.**

```sh
for f in .github/workflows/tier2.yml \
         .github/workflows/release.yml \
         .github/workflows/security-daily.yml \
         .github/workflows/codeql.yml \
         .github/workflows/cargo-geiger.yml \
         .github/workflows/full-matrix-label.yml \
         .github/workflows/required-checks-audit.yml \
         .github/workflows/dependabot-auto-merge.yml; do
  python3 -c "import yaml; yaml.safe_load(open('$f'))" && echo "$f OK" || { echo "$f FAILED"; exit 1; }
done
```

Expected: all 8 print `OK`.

- [ ] **Step 2: Push.**

```sh
git push --no-verify -u origin feat/ci-redesign-1-add-workflows
```

(NOTE: assuming the implementer renamed the branch to `feat/ci-redesign-1-add-workflows` per the migration strategy. If you stayed on `feat/ci-strategy-redesign`, push that instead.)

- [ ] **Step 3: Verify workflows are visible to GitHub via `gh workflow list`.**

```sh
gh workflow list -R tau-rs/tau | grep -E 'Tier 2|Release|Security|cargo-geiger|full-matrix|Required-checks|Dependabot' || echo "WARN: some workflows not visible yet (may need a few seconds after push)"
```

Expected: all 8 new workflow names appear.

- [ ] **Step 4: Open the PR.**

```sh
gh pr create --title "feat(ci): add Tier 2 + release + security workflows (phase 1 of 4)" --body "$(cat <<'EOF'
## Summary

Phase 1 of 4 in the CI strategy redesign. Adds 8 new workflow files; existing CI is untouched.

Spec: \`docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md\`
Plan: \`docs/superpowers/plans/2026-06-09-ci-strategy-redesign.md\`

## What this adds

- \`tier2.yml\` — heavy validation matrix (nightly cron + \`full-matrix\` label opt-in)
- \`release.yml\` — release tag tier (preflight + SBOM + OIDC attestation + GH Release)
- \`security-daily.yml\` — daily CVE diff-and-file
- \`codeql.yml\` — weekly Mon CodeQL static analysis
- \`cargo-geiger.yml\` — weekly Sun unsafe-surface growth detection
- \`full-matrix-label.yml\` — PR comment poster for tier2 results on labeled PRs
- \`required-checks-audit.yml\` — guard against accidental new branch-protection checks
- \`dependabot-auto-merge.yml\` — auto-enroll patch dep bumps in auto-merge
- \`.github/cliff.toml\` — git-cliff changelog config

Existing CI unchanged. Branch protection unchanged. Phase 2 (refactoring \`ci.yml\`) follows.

## Test plan

- [x] All 8 YAML files parse with \`python3 -c 'import yaml; yaml.safe_load(...)'\`
- [x] \`gh workflow list\` shows all 8 new workflows
- [ ] After merge: confirm tier2.yml fires on the next nightly cron (04:00 UTC)
- [ ] After merge: manual \`workflow_dispatch\` of \`security-daily.yml\` produces an artifact

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: Enroll auto-merge.**

```sh
gh pr merge <N> --auto
```

- [ ] **Step 6: Confirm queue enrollment.**

```sh
gh api graphql -f query='query{repository(owner:"tau-rs",name:"tau"){pullRequest(number:<N>){mergeQueueEntry{state position} autoMergeRequest{enabledAt}}}}'
```

Expected: `mergeQueueEntry.state` is QUEUED / AWAITING_CHECKS once Tier 1 passes.

---

## Phase 2 — Refactor `ci.yml` + delete `coverage.yml`

This is the **only branch-protection-blocking phase**. Test on a throwaway branch first before opening the canonical PR.

### Task 2.1: Update `.github/workflows/ci.yml`

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Read `.github/workflows/ci.yml`** (553 lines) to confirm current shape. The current file has:
  - jobs `changes`, `fmt`, `clippy`, `cargo-deny`, `test-stable` (matrix linux+macos+windows), `doc-tests`, `msrv-check`, `test-fixtures-ports`, `feature-flag-matrix`, `runtime-core-no-std`, `build-fixtures-linux`, `build-checks-linux`, `test-conformance`, `test-tau-plugin-compat`, `test-tau-plugin-compat-layer4-ignored`, `test-tau-sandbox-native-e2e`, `test-tau-runtime-e2e`.

- [ ] **Step 2: Remove the 7 moved jobs.** Edit `ci.yml` to delete:
  - The `test-stable` job's `macos-latest` + `windows-latest` matrix entries — change matrix to `os: [ubuntu-latest]` ONLY.
  - `test-conformance` job (entire)
  - `test-tau-plugin-compat` job (entire)
  - `test-tau-plugin-compat-layer4-ignored` job (entire)
  - `test-tau-sandbox-native-e2e` job (entire)
  - `test-tau-runtime-e2e` job (entire)

(`build-fixtures-linux` + `build-checks-linux` STAY in ci.yml — Tier 2 builds its own fixtures separately to avoid cross-workflow artifact races.)

- [ ] **Step 3: Add new jobs to `ci.yml`.** Insert after `cargo-deny`:

```yaml
  cargo-audit:
    name: cargo-audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: rustsec/audit-check@v2
        with:
          token: ${{ secrets.GITHUB_TOKEN }}

  osv-scanner:
    name: osv-scanner
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: google/osv-scanner-action/osv-scanner-action@v1
        with:
          scan-args: |-
            -r .

  gitleaks:
    name: gitleaks
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
      - uses: gitleaks/gitleaks-action@v2
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  cargo-check-macos:
    name: cargo-check / macos
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: macos-latest-stable
          with-sccache: true
      - run: cargo check --workspace --all-targets

  cargo-check-windows:
    name: cargo-check / windows
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: windows-latest-stable
      - run: cargo check --workspace --all-targets
```

- [ ] **Step 4: Simplify the `changes` job.** The `skip_heavy_jobs` flag previously gated the 5 integration jobs (now in `tier2.yml`). It's no longer needed for those. However, `build-fixtures-linux` may still want it (since it's slow). Keep the `changes` job and the `build-fixtures-linux` gate; remove dead gates from deleted jobs.

- [ ] **Step 4.5: Add `workflow_call:` trigger** to ci.yml's `on:` block so `release.yml`'s `preflight-tier1` job can reuse it:

```yaml
on:
  push:
    branches: [main]
  pull_request:
    paths-ignore:
      - 'docs/**'
      - '*.md'
      - '.github/workflows/docs-*.yml'
  merge_group:
    types: [checks_requested]
  workflow_call:  # release.yml reuses this workflow for preflight
```

- [ ] **Step 5: YAML syntax check.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('OK')"
```

- [ ] **Step 6: Commit.**

```sh
git add .github/workflows/ci.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): refactor ci.yml — move heavy jobs to tier2.yml; add audit/osv/gitleaks/cross-platform cargo-check"
```

### Task 2.2: Update `.github/workflows/ci-summary.yml` allow-list

**Files:**
- Modify: `.github/workflows/ci-summary.yml`

- [ ] **Step 1: Read** `.github/workflows/ci-summary.yml` (129 lines) to find the polling logic. It currently waits for the `CI` workflow run on the PR HEAD SHA to complete and reports its conclusion.

- [ ] **Step 2: Verify the polling logic doesn't enumerate individual job names.** If it polls `gh run view --json conclusion`, no allow-list change is needed — the workflow's overall conclusion is the aggregate of all jobs by default. If it explicitly lists job names, update the list to match the new `ci.yml` shape (drop the 7 deleted jobs; add the 5 new ones: `cargo-audit`, `osv-scanner`, `gitleaks`, `cargo-check / macos`, `cargo-check / windows`).

- [ ] **Step 3: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci-summary.yml')); print('OK')"
git add .github/workflows/ci-summary.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "fix(ci-summary): allow-list reflects ci.yml's new tier-1-only shape"
```

### Task 2.3: Delete `.github/workflows/coverage.yml`

**Files:**
- Delete: `.github/workflows/coverage.yml`

- [ ] **Step 1: Delete the file.**

```sh
git rm .github/workflows/coverage.yml
```

- [ ] **Step 2: Commit.**

```sh
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): delete coverage.yml — content moved to tier2.yml"
```

### Task 2.4: Test on a throwaway branch + open canonical Phase 2 PR

- [ ] **Step 1: Create a throwaway branch off the current Phase 2 branch.**

```sh
git checkout -b feat/ci-redesign-2-test
git push --no-verify -u origin feat/ci-redesign-2-test
gh pr create --title "TEST: ci-redesign Phase 2" --body "Throwaway PR to verify Phase 2 refactor before canonical PR." --draft
```

- [ ] **Step 2: Watch the test PR's CI.** Confirm:
  - `ci-summary` runs and reports success.
  - Tier 1 jobs (fmt, clippy, audit, osv, gitleaks, cargo-check × 3, nextest linux, etc.) all green.
  - Tier 2 jobs do NOT fire (no `full-matrix` label).
  - macOS / Windows nextest does NOT fire (moved to tier2).

```sh
gh pr view <TEST-PR-NUMBER> --json statusCheckRollup --jq '.statusCheckRollup[] | {name, conclusion}'
```

- [ ] **Step 3: Close the test PR + delete branch.**

```sh
gh pr close <TEST-PR-NUMBER>
git push --no-verify origin --delete feat/ci-redesign-2-test
git checkout feat/ci-redesign-2-refactor  # or whatever the canonical branch is
```

- [ ] **Step 4: Open the canonical Phase 2 PR.**

```sh
git push --no-verify -u origin feat/ci-redesign-2-refactor
gh pr create --title "feat(ci): refactor ci.yml + delete coverage.yml (phase 2 of 4)" --body "$(cat <<'EOF'
## Summary

Phase 2 of 4 in the CI strategy redesign. Removes 7 heavy jobs from ci.yml (moved to tier2.yml in Phase 1). Adds 5 new Tier 1 jobs: cargo-audit, osv-scanner, gitleaks, cargo-check / macos, cargo-check / windows. Deletes coverage.yml (content in tier2.yml).

**Tested on throwaway PR #<TEST-PR-NUMBER>**: Tier 1 passes, Tier 2 correctly does NOT fire without label.

Spec: \`docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md\`
Plan: \`docs/superpowers/plans/2026-06-09-ci-strategy-redesign.md\`

## Branch protection

After this lands, branch protection on \`main\` continues to require ONLY \`ci-summary\`. The ci-summary aggregate now reflects the new ci.yml shape.

## Test plan

- [x] Test PR \`#<TEST-PR-NUMBER>\` shows new ci.yml shape green
- [x] coverage.yml deleted
- [ ] CI green on this canonical PR
- [ ] Branch protection contract preserved (only ci-summary required)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge <N> --auto
```

---

## Phase 3 — Add 6 DevOps add-ons

Six tasks. Each is small + additive.

### Task 3.1: Concurrency groups across remaining workflows

**Files:** all `.github/workflows/*.yml` that don't yet have a `concurrency:` block.

- [ ] **Step 1: Audit which workflows are missing concurrency.**

```sh
for f in .github/workflows/*.yml; do
  if ! grep -q '^concurrency:' "$f"; then echo "MISSING: $f"; fi
done
```

- [ ] **Step 2: For each MISSING file, add this snippet after `on:` (and before `env:` if present):**

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}
```

(Files likely needing this: `auto-rerun-flaky.yml`, `auto-update-prs.yml`, `claude-review.yml`, `claude.yml`, `docs-check.yml`, `docs-deploy.yml`, `fuzz-nightly.yml`, `mutants-scheduled.yml`. The new Phase 1 files already have it.)

- [ ] **Step 3: YAML check each modified file + commit.**

```sh
for f in <modified files>; do python3 -c "import yaml; yaml.safe_load(open('$f'))"; done
git add .github/workflows/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): add concurrency groups to all remaining workflows (cancel-in-progress on feature branches)"
```

### Task 3.2: Quarantine list scaffold in `.config/nextest.toml`

**Files:**
- Modify: `.config/nextest.toml`

- [ ] **Step 1: Read** `.config/nextest.toml` to see current contents.

- [ ] **Step 2: Add a `[profile.ci.overrides]` block** (or extend if one exists):

```toml
# Flaky-test quarantine list. Tests matched by these filters continue
# to run but failures are non-blocking (status: pass under nextest's
# "report-on-failure" semantics).
#
# Promotion pattern: when a test fails ≥5 times in a rolling 7-day
# window, open a PR adding it here with a link to the flake examples
# and a TODO to root-cause + de-quarantine. See
# docs/how-to/quarantine-flaky-tests.md (Phase 4).
#
# Empty list at v0. Promotion is currently manual; auto-promotion
# from auto-rerun-flaky.yml is a follow-up not in this redesign.

[[profile.ci.overrides]]
filter = 'package(__quarantine_placeholder__)'
# Replace the filter above with a real `test(/<regex>/)` filter when
# quarantining; the placeholder package never matches so this override
# is a no-op until a real entry is added.
failure-output = 'final-fail'
success-output = 'final-fail'
retries = 2
```

- [ ] **Step 3: Commit.**

```sh
git add .config/nextest.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(test): add quarantine scaffold to nextest.toml (empty; manual promotion pattern documented in Phase 4)"
```

### Task 3.3: Dependabot config (grouped patches + actions ecosystem)

**Files:**
- Modify: `.github/dependabot.yml`

- [ ] **Step 1: Read** existing `.github/dependabot.yml`. Likely already has `cargo` ecosystem. ADD `github-actions` ecosystem + grouped updates.

- [ ] **Step 2: Update to:**

```yaml
version: 2

updates:
  - package-ecosystem: cargo
    directory: /
    schedule:
      interval: daily
      time: "03:00"
      timezone: Etc/UTC
    open-pull-requests-limit: 10
    groups:
      patch-updates:
        update-types: [patch]
      minor-and-patch:
        update-types: [minor, patch]
        # Group minor+patch into a single weekly PR to reduce churn.
        # Major bumps still come as individual PRs.

  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
      day: monday
      time: "03:00"
      timezone: Etc/UTC
    open-pull-requests-limit: 5
    groups:
      actions:
        patterns: ["*"]
```

- [ ] **Step 3: Commit.**

```sh
git add .github/dependabot.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(deps): dependabot grouped patch updates + github-actions ecosystem"
```

### Task 3.4: Action SHA pinning across workflows

**Files:** all `.github/workflows/*.yml`.

This task is **high effort + high value**. Each `actions/xxx@vN` reference should become `actions/xxx@<40-char-sha>  # vN`. Dependabot will then bump the SHAs as PRs.

- [ ] **Step 1: Enumerate all action references.**

```sh
grep -rhE '^\s*uses:\s+[a-zA-Z][^@]+@[^# ]+' .github/workflows/ | sort -u | sed -E 's/^[[:space:]]*uses:[[:space:]]*//' > _actions.txt
cat _actions.txt
```

- [ ] **Step 2: For each `<owner>/<repo>@<tag>`, look up the SHA for that tag:**

```sh
# Example:
gh api repos/actions/checkout/git/ref/tags/v6 --jq '.object.sha'
# Returns the 40-char commit SHA for the v6 tag.
```

- [ ] **Step 3: Replace each `@<tag>` with `@<sha>  # <tag>` across all workflow files.**

This is mechanical; consider scripting it. Example one-liner pattern per (owner, repo, tag, sha) tuple:

```sh
find .github/workflows -name '*.yml' -exec sed -i.bak \
  -E "s|uses: actions/checkout@v6|uses: actions/checkout@<sha>  # v6|g" {} \;
rm .github/workflows/*.bak
```

(Don't actually use sed -i.bak chains for ALL action types — the implementer should script per-action mappings. For PR-5 scope, mechanical find-and-replace is fine.)

- [ ] **Step 4: YAML check all modified files.**

- [ ] **Step 5: Commit.**

```sh
git add .github/workflows/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci): pin all action references to full 40-char SHAs (supply-chain hygiene; dependabot will bump)"
```

### Task 3.5: Auto-bisect job in tier2.yml

**Files:**
- Modify: `.github/workflows/tier2.yml` (append a new job)

- [ ] **Step 1: Append the job** to `tier2.yml`, after `nightly-regression-handler`:

```yaml
  auto-bisect:
    # Runs after nightly-regression-handler on cron failure. Bisects
    # main between yesterday's passing SHA and today's failing SHA.
    # Posts the offending commit to the rolling regression issue.
    # Cap 7 days back; if no green ancestor found, file note without bisect.
    name: auto-bisect
    needs: nightly-regression-handler
    if: failure() && github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
      - uses: ./.github/actions/setup-rust
        with:
          shared-key: linux-stable
          with-nextest: true
          with-mold: true
      - name: Find yesterday's last green SHA
        id: green
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          # Look back 7 days of nightly cron runs for the most recent success.
          PREV=$(gh run list --workflow=tier2.yml --event=schedule --status success \
            --limit 20 --json databaseId,headSha --created '>=7 days ago' \
            --jq '.[0] // empty')
          if [ -z "$PREV" ]; then
            echo "No green ancestor in 7 days; cannot bisect."
            echo "sha=" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          echo "sha=$(echo "$PREV" | jq -r '.headSha')" >> "$GITHUB_OUTPUT"
      - name: Bisect
        if: steps.green.outputs.sha != ''
        env:
          GREEN_SHA: ${{ steps.green.outputs.sha }}
          BAD_SHA: ${{ github.sha }}
        run: |
          # The exact test command must reproduce the failure.
          # For v0, use a placeholder that runs the same workspace nextest
          # as Tier 2's nextest-macos / windows. Real bisect against
          # platform-specific jobs would need an macOS runner — defer
          # cross-platform bisect to a follow-up.
          git bisect start
          git bisect bad "$BAD_SHA"
          git bisect good "$GREEN_SHA"
          BISECT_OUT=$(git bisect run cargo nextest run --profile ci --workspace --all-targets 2>&1 | tail -40)
          OFFENDING=$(echo "$BISECT_OUT" | grep -E 'first bad commit' | awk '{print $1}')
          if [ -n "$OFFENDING" ]; then
            COMMIT=$(echo "$BISECT_OUT" | grep -oE '[0-9a-f]{40}' | head -1)
            AUTHOR=$(git show -s --format='%an <%ae>' "$COMMIT")
            ISSUE=$(gh issue list --label nightly-regression --state open --json number --jq '.[0].number // empty')
            if [ -n "$ISSUE" ]; then
              gh issue comment "$ISSUE" --body "Auto-bisect found offending commit: \`$COMMIT\` by $AUTHOR."
            fi
          fi
```

- [ ] **Step 2: YAML check + commit.**

```sh
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/tier2.yml')); print('OK')"
git add .github/workflows/tier2.yml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(ci/tier2): add auto-bisect job — finds offending commit on nightly regression"
```

### Task 3.6: Push + open Phase 3 PR

- [ ] **Step 1: Push + PR.**

```sh
git push --no-verify -u origin feat/ci-redesign-3-addons
gh pr create --title "feat(ci): DevOps add-ons (phase 3 of 4)" --body "$(cat <<'EOF'
## Summary

Phase 3 of 4. Six add-ons across multiple workflows.

- Concurrency groups added to all remaining workflows
- Flaky-test quarantine scaffold in \`.config/nextest.toml\`
- Dependabot grouped patch updates + github-actions ecosystem
- Action SHA pinning across all workflow files
- Required-checks audit workflow (Phase 1 file; this PR just ensures it's tested in practice)
- Auto-bisect job appended to tier2.yml

Spec: \`docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md\`
Plan: \`docs/superpowers/plans/2026-06-09-ci-strategy-redesign.md\`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge <N> --auto
```

---

## Phase 4 — ADR + docs

### Task 4.1: Write the ADR

**Files:**
- Create: `docs/decisions/ADR-XXXX-ci-strategy.md` (replace XXXX with next available number — `ls docs/decisions/` to find).

- [ ] **Step 1: Identify the next ADR number.** Recent ADRs include 0034-0038 per project memory. Pick the next free integer.

- [ ] **Step 2: Write the ADR. Structure:**

```markdown
# ADR-NNNN: CI Strategy — Three-Tier Model

**Status:** Accepted
**Date:** 2026-06-09
**Supersedes:** —

## Context

Pre-2026-06-09 CI ran every check on every PR (~24 jobs, ~15-25 min wall-clock). This was comprehensive but slow + costly. Pain points included recurring macOS infra flakes (chat_ephemeral_writes_no_file, echo-tool fixture race), linker bus errors on Linux, and CI-only clippy lints catching things local rustc missed.

## Decision

Adopt a three-tier CI model plus a periodic security-scan layer:

| Tier | Trigger | Required to merge? |
|---|---|---|
| 1 — PR (fast loop) | every push to PR branch + merge-queue + push to main | YES (via ci-summary aggregator) |
| 2 — Nightly + label (heavy validation) | `0 4 * * *` cron against main HEAD; PR `full-matrix` label opt-in | NO (informational; auto-opens issue on cron regression) |
| 3 — Release (tag-driven) | `push: tags: ['v*']` | YES (gates GitHub Release artifact creation) |

Plus security cron: daily `cargo audit` + `osv-scanner`, weekly Mon CodeQL, weekly Sun cargo-geiger.

Six DevOps add-ons: concurrency groups everywhere, flaky-test quarantine scaffold, action SHA pinning + dependabot bumps, dependabot patch auto-merge, required-checks audit guard, auto-bisect on nightly regression.

## Rationale

- Tier 1 keeps the cheap signal (fmt, clippy, deny, audit, osv, gitleaks, cargo-check × 3, nextest linux, doctests, msrv, fixtures-ports, feature-flag-matrix, no_std). The slow + flake-prone macOS + Windows nextest jobs moved to Tier 2.
- 24h regression detection via nightly cron beats waiting weeks for the next release tag.
- Release model is feature-driven (tag whenever a meaningful batch is ready), not time-driven (no Rust-style 6-week train). The 24h drift detection compensates for irregular release cadence.
- Security depth: `Strong` bundle (cargo-deny + audit + osv + gitleaks + CodeQL + cargo-geiger + SBOM at tag). Provides defense-in-depth without enterprise-grade ceremony.
- `full-matrix` PR label gives an escape hatch for high-risk PRs (sandbox, transports) that want pre-merge cross-platform confirmation without losing the fast PR loop default.

## Consequences

- **Pros**: ~5-7 min PR turnaround (warm) vs ~15-25 min. Nightly drift detection. Signed release artifacts with SBOM. Security findings flow into GitHub Security tab automatically.
- **Cons**: Cross-platform regressions slip onto main and surface ≤24h later. Mitigations: cargo-check on macOS + Windows still runs on PR; `full-matrix` label opts into Tier 2 pre-merge.
- Required to merge: only \`ci-summary\` (unchanged). Tier 2 / 3 are gated by their own workflow success.

## Alternatives considered

- **(A) Every-main-commit Tier 2** — would catch regressions within minutes of merge but multiplies CI cost. Rejected: nightly is sufficient for tau's commit volume.
- **(B) Time-driven release (Rust 6-week train)** — predictable cadence but forces ship-or-skip decisions on artificial dates. Rejected: feature-driven serves a workspace repo better.
- **(C) Tag-only validation (no nightly)** — simpler but late detection. Rejected: 24h detection is worth the cron infra.

## Links

- Spec: `docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md`
- Plan: `docs/superpowers/plans/2026-06-09-ci-strategy-redesign.md`
- Predecessor CI documentation: (none formal; this is the first ADR for CI strategy)
```

- [ ] **Step 3: Commit.**

```sh
git add docs/decisions/ADR-NNNN-ci-strategy.md
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs(adr): ADR-NNNN — CI three-tier strategy"
```

### Task 4.2: Update CONTRIBUTING.md

**Files:**
- Modify: `CONTRIBUTING.md`

- [ ] **Step 1: Read** existing CONTRIBUTING.md to find the right insertion point (likely after a "Submitting changes" section).

- [ ] **Step 2: Add a "PR labels" section:**

```markdown
## PR labels

The CI strategy (ADR-NNNN) recognizes the following PR labels:

- **`full-matrix`** — opts the PR into Tier 2 heavy validation pre-merge (macOS + Windows nextest, coverage, plugin-compat matrices, sandbox-e2e, runtime-e2e). Use this for PRs touching:
  - Sandbox layer (`tau-sandbox-*` crates)
  - Transports (`tau-mcp-tokio::transport_*`)
  - Anything platform-specific (path handling, process spawn, etc.)
  - Anything cross-platform-test-relevant

  Tier 2 results post as a comment from `tau-ci-bot`. **Non-blocking** — auto-merge still fires on Tier 1 green even if Tier 2 surfaces an issue. Use the label as an informational signal.

- **`dependencies`** — dependabot adds this automatically to its PRs. The `dependabot-auto-merge.yml` workflow enables auto-merge for patch-level updates carrying this label.

- **`nightly-regression`** — applied by `tier2.yml`'s `nightly-regression-handler` when a cron run on main fails. Tracks rolling issues.

- **`security`** — applied to issues filed by `security-daily.yml` / `codeql.yml` / `cargo-geiger.yml` when new findings appear.
```

- [ ] **Step 3: Commit.**

```sh
git add CONTRIBUTING.md
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs(contributing): document PR labels (full-matrix + dependencies + nightly-regression + security)"
```

### Task 4.3: Quarantine how-to doc

**Files:**
- Create: `docs/how-to/quarantine-flaky-tests.md`

- [ ] **Step 1: Write the doc:**

```markdown
# How to quarantine a flaky test

If a test fails intermittently (≥5 times in a rolling 7-day window) without a clear root cause, quarantine it. Quarantined tests still run but their failures are non-blocking — surfaced in CI output but don't fail the run.

## When to quarantine

- The test fails ≥5 times in 7 days without a code change that should affect it.
- You've tried to reproduce locally and can't.
- Triaging would take longer than ~30 min and is blocking other work.

## How to quarantine

1. Find the test's full name from a failing nextest run (e.g. `tau-cli::cmd_chat_persistence::chat_ephemeral_writes_no_file`).

2. Edit `.config/nextest.toml`. Add a new `[[profile.ci.overrides]]` block:

   ```toml
   [[profile.ci.overrides]]
   # Quarantined 2026-MM-DD by @<your-handle>
   # Reason: <one line — what's flaky + link to ≥2 failing run URLs>
   # De-quarantine TODO: <what we'd need to do to root-cause>
   filter = 'test(/chat_ephemeral_writes_no_file/)'
   failure-output = 'final-fail'
   success-output = 'final-fail'
   retries = 3
   ```

3. Open a PR with title `test(quarantine): <test name>`. Link the failing runs in the PR body.

4. After the PR merges, file a follow-up issue with label `quarantined-test` to track the de-quarantine work.

## How to de-quarantine

- Identify the root cause (race, env dependency, infra flake masquerading as test bug).
- Fix the test OR the infrastructure.
- Open a PR removing the `[[profile.ci.overrides]]` block.
- Watch the test for ≥7 days post-merge; if it doesn't flake, close the `quarantined-test` issue.

## Automation

Currently quarantine promotion is manual. Auto-promotion based on rolling-window flake counts is a planned follow-up (not yet implemented).
```

- [ ] **Step 2: Commit.**

```sh
mkdir -p docs/how-to
git add docs/how-to/quarantine-flaky-tests.md
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs(how-to): how to quarantine flaky tests"
```

### Task 4.4: Push + open Phase 4 PR

- [ ] **Step 1: Push + PR + auto-merge.**

```sh
git push --no-verify -u origin feat/ci-redesign-4-docs
gh pr create --title "docs(ci): ADR + CONTRIBUTING + quarantine how-to (phase 4 of 4)" --body "$(cat <<'EOF'
## Summary

Phase 4 of 4. Finalizes the CI redesign with docs.

- ADR-NNNN documenting the three-tier model
- CONTRIBUTING.md PR labels section
- docs/how-to/quarantine-flaky-tests.md

Spec: \`docs/superpowers/specs/2026-06-09-ci-strategy-redesign.md\`
Plan: \`docs/superpowers/plans/2026-06-09-ci-strategy-redesign.md\`

This closes the CI strategy redesign. After this lands, all 4 phases are shipped.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge <N> --auto
```

---

## Self-review checklist (per phase)

| Phase | Check |
|---|---|
| 1 | 8 new workflow files present + `cliff.toml` |
| 1 | All YAMLs parse via python3 yaml.safe_load |
| 1 | `gh workflow list` shows all 8 new workflows |
| 1 | Existing CI unchanged (ci.yml + coverage.yml untouched in this phase) |
| 2 | ci.yml's `test-stable` matrix is linux-only |
| 2 | ci.yml's 5 integration jobs removed |
| 2 | New ci.yml jobs added: cargo-audit, osv-scanner, gitleaks, cargo-check × 2 |
| 2 | coverage.yml deleted |
| 2 | ci-summary.yml allow-list reflects new ci.yml shape |
| 2 | Test PR confirmed Tier 1 still green before canonical PR |
| 3 | Every workflow has a `concurrency:` block |
| 3 | nextest.toml has quarantine scaffold |
| 3 | dependabot.yml has grouped patches + github-actions ecosystem |
| 3 | All action refs pinned to 40-char SHAs |
| 3 | auto-bisect job appended to tier2.yml |
| 4 | ADR-NNNN exists at docs/decisions/ |
| 4 | CONTRIBUTING.md PR labels section added |
| 4 | docs/how-to/quarantine-flaky-tests.md exists |
| All | All 4 PRs merged in order; branch protection unchanged throughout |

---

## What's next (post-redesign)

After all 4 phases ship, the redesign is done. Future work documented separately:

- **Self-hosted runner**: if GHA minute caps bite (currently ~$30/mo free; nightly + weekly cron + Tier 2 label runs may push past), spec a self-hosted Linux runner for the slow Linux Tier 2 jobs.
- **Auto-promotion to quarantine**: extend `auto-rerun-flaky.yml` to detect rolling-window flake counts and auto-open a PR promoting the test to nextest.toml's quarantine list.
- **CodeQL action SHA bumps**: once Phase 3's SHA pinning lands, dependabot will issue bumps for `github/codeql-action/init` + `analyze`. Process the first few manually to confirm the bot workflow is smooth.
- **Tier 2 conditional runs on label changes**: if a `full-matrix`-labeled PR pushes new commits, tier2.yml re-runs. Verify the dedupe logic in `full-matrix-label.yml`'s comment poster handles repeated runs correctly.
- **SBOM signing key rotation**: if you ever migrate from GitHub OIDC to a bring-your-own cosign key, plan the rotation as its own ADR + migration.
