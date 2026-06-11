# Root `justfile` DevOps Wrapper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a root `justfile` exposing canonical verbs (`fmt / lint / test /
deny / ci / heavy / fix`) as the single source of truth for tau's dev commands,
then refactor the matching `ci.yml` `run:` blocks and `lefthook.yml` pre-commit
run-strings to call `just <verb>` so each command string lives in exactly one place.

**Architecture:** Each recipe carries ONLY the bare cargo command string (exactly
as it appears in CI today). The execution environment — `CARGO_TARGET_DIR`,
`CARGO_INCREMENTAL` — is supplied by the *caller's* environment (CI via the
workflow-level `env:` block, lefthook via its per-command `env …` prefix). `just`
inherits the parent environment and passes it through to recipe shells, so the
executed cargo invocation + env stays byte-equivalent to today. The `test` recipe
takes `*args` so callers can append flags (e.g. lefthook's `--target-dir`).

**Tech Stack:** [`just`](https://github.com/casey/just) command runner;
`taiki-e/install-action` to install `just` on CI runners; existing
`cargo fmt / clippy / nextest / deny / run -p xtask`.

---

## Design decisions (locked)

1. **Recipes hold only the command string, never `CARGO_TARGET_DIR`.** Baking a
   target dir into a recipe would clobber lefthook's per-command isolation
   (`target/lefthook/{fmt,clippy,test}`) and the main-agent vs sub-agent dirs from
   CLAUDE.md Rule 1. The caller owns the env; the recipe owns the flags.

2. **`test` recipe uses `--profile ci` (per the brief's verb table) and is routed
   into the CI `test-stable` job — but NOT into lefthook's `test-native`.**
   `.config/nextest.toml` deliberately splits profiles: `ci` = `retries = 0`
   (a flake is signal), `default` = `retries = 2` (keep the inner loop snappy).
   lefthook's pre-commit hook intentionally runs the default profile AND a
   nested-cargo `unset CARGO_TARGET_DIR GIT_*` wrapper (documented at length in
   `lefthook.yml`). Routing it through `just test` (`--profile ci`) would strip
   local retries — a real behavioral regression to the hook — so `test-native`
   stays as-is. This is the one lefthook command intentionally not single-sourced;
   it is a genuinely different command, not a duplicate.

3. **`deny` and `heavy` are new local-only verbs with no call-site to refactor.**
   The CI `cargo-deny` job is a `uses:` action (EmbarkStudios/cargo-deny-action),
   not a `run:` block, so it is not touched (the brief scopes the refactor to
   `run:` blocks). **Deny flag-order correction:** the action runs
   `cargo-deny --all-features check` (its `arguments` input is emitted *before*
   `command` — verified against the pinned action.yml). The brief's literal
   `cargo deny check --all-features` is the pre-0.14 subcommand position and is
   rejected by cargo-deny 0.18.x ("unexpected argument"); the recipe uses the
   global position so it both works locally and is byte-for-byte CI. `heavy` mirrors `tier2.yml` / the deep-gate locally but does
   NOT modify those workflows (per the "do not bundle in the heavy-tier work"
   constraint) — it wraps `xtask`, never reimplements image logic.

4. **CI runners need `just`.** ubuntu-latest does not ship `just`. Add a
   `with-just` boolean to the shared `setup-rust` composite action (mirroring
   `with-nextest` / `with-mold` / `with-sccache`), installing via
   `taiki-e/install-action` (same action already used for nextest, same `@v2`
   tag convention — no SHA-pin work). Set `with-just: true` on the three
   refactored jobs only.

## File structure

- **Create** `justfile` (repo root) — the seven canonical recipes + `default`.
- **Modify** `.github/actions/setup-rust/action.yml` — add `with-just` input + install step.
- **Modify** `.github/workflows/ci.yml` — `fmt`, `clippy`, `test-stable` jobs: `run:` → `just`, add `with-just: true`.
- **Modify** `lefthook.yml` — `fmt`, `clippy` pre-commit run-strings → `just`; `test-native` and `check-linux-x86` unchanged.

---

### Task 1: Create the root `justfile`

**Files:**
- Create: `justfile`

- [ ] **Step 1: Write the justfile**

```just
# tau workspace task runner — canonical verbs shared with CI and lefthook so
# "local == CI" and the same muscle memory works across the sibling repos.
#
# Each recipe carries ONLY the cargo command string (identical to the matching
# CI job). The execution environment is supplied by the CALLER:
#   - CI            sets CARGO_INCREMENTAL=0 at the workflow `env:` level.
#   - lefthook      sets CARGO_INCREMENTAL=0 / CARGO_TARGET_DIR per command.
#   - agents (this  per CLAUDE.md "CARGO RULES", prefix with an isolated dir:
#     workspace)      env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main just test
# `just` passes the inherited environment through to recipe shells, so the
# executed cargo invocation + env is byte-equivalent to running the command
# directly. Do NOT bake CARGO_TARGET_DIR into a recipe — it would clobber
# lefthook's per-command target dirs and agents' isolated dirs.

# List the available recipes (default when `just` is run with no arguments).
default:
    @just --list

# Format check — mirrors the `rustfmt` CI job.
fmt:
    cargo fmt --all -- --check

# Lint — mirrors the `clippy` CI job.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Test — mirrors the `test-stable` CI job. Extra args are forwarded so callers
# can append flags (e.g. lefthook appends `--target-dir target/lefthook/test`).
test *args:
    cargo nextest run --profile ci --workspace --all-targets {{args}}

# Dependency / license / advisory audit — mirrors the `cargo-deny` CI job.
# `--all-features` is a GLOBAL flag (before the subcommand) in cargo-deny 0.14+,
# and the cargo-deny-action passes it that way (arguments → command), so this is
# byte-for-byte what CI runs: `cargo-deny --all-features check`.
deny:
    cargo deny --all-features check

# Full local gate: everything a PR must pass. Same set the CI fast tier runs.
ci: fmt lint test deny

# Auto-fix: apply rustfmt + machine-applicable clippy suggestions in place.
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged

# Heavy tier: build per-plugin container images via xtask, then run the
# integration-tests e2e suites. Mirrors `tier2.yml` / the lefthook deep-gate
# for a local pre-flight. WRAPS xtask — never reimplements the image logic.
# Requires a container runtime (podman/docker) on PATH.
heavy:
    cargo run -p xtask -- build-plugin-images
    cargo nextest run --profile ci -p tau-runtime-tokio    --features integration-tests --tests
    cargo nextest run --profile ci -p tau-sandbox-native   --features integration-tests --tests
    cargo nextest run --profile ci -p tau-plugin-compat    --features integration-tests --tests
```

- [ ] **Step 2: Verify the justfile parses and lists**

Run: `just --list`
Expected: a recipe listing including `fmt`, `lint`, `test`, `deny`, `ci`, `fix`, `heavy`.

---

### Task 2: Add `with-just` to the shared setup-rust action

**Files:**
- Modify: `.github/actions/setup-rust/action.yml`

- [ ] **Step 1: Add the `with-just` input** (after the `with-mold` input block):

```yaml
  with-just:
    description: Install the `just` command runner (true / false)
    required: false
    default: "false"
```

- [ ] **Step 2: Add the install step** (after the `Install cargo-nextest` step,
  before `Cache cargo registry…`):

```yaml
    - name: Install just
      if: inputs.with-just == 'true'
      uses: taiki-e/install-action@v2
      with:
        tool: just
```

---

### Task 3: Refactor the three CI `run:` blocks

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1:** `fmt` job — add `with-just: true` to its `setup-rust` `with:`
  block, and change `- run: cargo fmt --all -- --check` to `- run: just fmt`.

- [ ] **Step 2:** `clippy` job — add `with-just: true`, change
  `- run: cargo clippy --workspace --all-targets -- -D warnings` to `- run: just lint`.

- [ ] **Step 3:** `test-stable` job — add `with-just: true`, change
  `- run: cargo nextest run --profile ci --workspace --all-targets` to `- run: just test`.

---

### Task 4: Refactor lefthook fmt + clippy run-strings

**Files:**
- Modify: `lefthook.yml`

- [ ] **Step 1:** `fmt` command:
  `run: env CARGO_TARGET_DIR=target/lefthook/fmt cargo fmt --all -- --check`
  → `run: env CARGO_TARGET_DIR=target/lefthook/fmt just fmt`

- [ ] **Step 2:** `clippy` command:
  `run: env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/lefthook/clippy cargo clippy --workspace --all-targets -- -D warnings`
  → `run: env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/lefthook/clippy just lint`

- [ ] **Step 3:** Leave `test-native` and `check-linux-x86` unchanged (decision 2/3).

---

### Task 5: Verify (verification-before-completion)

- [ ] `just fmt` → green (real output captured)
- [ ] `just lint` → green
- [ ] `just test` → green (or note long-running; confirm it dispatches the right command)
- [ ] `just deny` → green
- [ ] `just ci` → runs all four in order
- [ ] `just heavy` → confirm it dispatches `cargo run -p xtask -- build-plugin-images` (full image build may be skipped for time)
- [ ] Confirm each refactored CI `run:`/lefthook string executes the identical cargo invocation as before (no lost flags, e.g. `--locked`, `--all-targets`, `--profile ci`).
- [ ] `git diff` review against the byte-equivalence checklist.

### Task 6: Commit, push, PR (cite G4) — STOP, no merge.
