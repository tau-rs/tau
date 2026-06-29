# EPIC 2.4 — Contract Compat/Versioning Policy + cargo-semver-checks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close EPIC 2 — give `tau-ports` an independent version line enforced by a `cargo-semver-checks` CI lane, and ship the operator-facing compat/versioning policy doc that consolidates the model + all deferred resolutions.

**Architecture:** Three small, independent deliverables: (1) `tau-ports` leaves the workspace `0.0.0` for its own `version = "0.1.0"`; (2) a gated CI job runs `cargo semver-checks` comparing the PR's `tau-ports` API against `origin/main`, failing on an undeclared break; (3) a Diátaxis explanation page documenting the policy. No production code logic.

**Tech Stack:** Cargo, `cargo-semver-checks` (CI only, via `taiki-e/install-action`), GitHub Actions, mdbook.

## Global Constraints

- **CARGO RULES (repo CLAUDE.md):** every cargo command `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate> ...`. Never bare/`--workspace`. check 180s.
- **`tau-ports` version is exactly `0.1.0`** (its own line, NOT `version.workspace`). No other crate's version changes.
- **The WIT package is `tau:run` (not `tau:host`)** — ADR-0056's `tau:host` was illustrative. The policy doc must say `tau:run`.
- **"Generated from ports" (ADR-0055) means "provably non-drifting"** — there is no Rust-trait→WIT generator; the drift tests deliver the guarantee.
- **The three enforcement guards already shipped:** IR schema drift/conformance (`tau-ir` `schema_export`+`schema_conformance`, 2.2), WIT freeze (`tau-wasm-host/tests/wit_host_drift.rs`, 2.3); `tau-ports` `cargo-semver-checks` is added here.
- **CI gating:** `ci-summary.yml` aggregates the CI workflow conclusion dynamically — do NOT edit it; a new job in `ci.yml` is auto-gated.
- **Commit identity:** `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit` (use `--no-verify` only if the lefthook corrupts git identity — documented CLAUDE.md behavior; docs/CI-only changes may use it).
- **Branch:** `feat/epic-2-4-contract-compat-policy` (already checked out). Do not rename.

---

### Task 1: give `tau-ports` an independent `0.1.0` version

**Files:**
- Modify: `crates/tau-ports/Cargo.toml` (line 4: `version.workspace = true` → `version = "0.1.0"`)

**Interfaces:**
- Produces: `tau-ports@0.1.0`. Task 2's `cargo-semver-checks` baseline compares against this once merged.

- [ ] **Step 1: Change the version line**

In `crates/tau-ports/Cargo.toml`, replace:

```toml
version.workspace      = true
```

with:

```toml
version                = "0.1.0"
```

(Keep `edition.workspace`, `rust-version.workspace`, `license.workspace`, `repository.workspace`, `authors.workspace` as-is — only the version line changes.)

- [ ] **Step 2: Verify the workspace still builds**

Path-dep consumers (`tau-ports = { workspace = true }`) ignore the version field, so nothing downstream breaks. Confirm:

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
```

Expected: both compile (tau-ir is a representative downstream consumer of tau-ports). `Cargo.lock` may update `tau-ports`'s version entry — that is expected.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-ports/Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.4): give tau-ports an independent 0.1.0 version line"
```

---

### Task 2: `cargo-semver-checks` CI lane for `tau-ports`

**Files:**
- Modify: `.github/workflows/ci.yml` (add a `ports-semver` job after the `wit-host-drift` job, before the `ci-summary` comment block)

**Interfaces:**
- Consumes: Task 1 (`tau-ports@0.1.0`).
- Produces: a required CI lane that fails when `tau-ports` has a breaking API change not matched by a version bump.

**Verification note:** `cargo-semver-checks` needs a nightly toolchain (rustdoc JSON) and the `tau-ports` baseline from `origin/main` (it is unpublished, so the crates.io baseline does not apply — use `--baseline-rev`). The agent likely cannot `cargo install cargo-semver-checks` locally (denied per CLAUDE.md), so **the first CI run on the PR is this task's verification**. If you CAN install it locally, run the Step 2 command to pre-validate; otherwise wire CI and rely on the PR's CI run.

- [ ] **Step 1: Add the CI job**

In `.github/workflows/ci.yml`, immediately after the `wit-host-drift` job (it ends with `run: cargo test -p tau-wasm-host --test wit_host_drift`) and before the `# ci-summary moved to its own workflow` comment, insert:

```yaml
  ports-semver:
    name: tau-ports ABI (cargo-semver-checks)
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6
        with:
          fetch-depth: 0  # need origin/main as the semver baseline
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: linux-stable
          with-sccache: true
          with-mold: true
      - name: Install nightly (rustdoc JSON for cargo-semver-checks)
        run: rustup toolchain install nightly --profile minimal
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-semver-checks
      - name: tau-ports ABI must not break without a version bump
        run: cargo semver-checks --baseline-rev origin/main --package tau-ports
```

Match the `actions/checkout` SHA and the `setup-rust` `with:` keys to the neighboring `wit-host-drift` job exactly (the snippet above already mirrors them; reconcile any difference in favour of what that job uses). Pin `taiki-e/install-action` to the SHA/version used elsewhere in the workflow if one is already present (search `taiki-e/install-action` in `.github/`); otherwise `@v2` is acceptable.

- [ ] **Step 2: (If `cargo-semver-checks` is installable locally) pre-validate**

```bash
git fetch origin main
cargo semver-checks --baseline-rev origin/main --package tau-ports
```

Expected: PASS — this PR makes NO API change to `tau-ports` (only the version-line move), so semver-checks reports no breaking change and an adequate version. If `cargo-semver-checks` is not installed and cannot be installed, SKIP this step and note it in your report; CI is the gate.

- [ ] **Step 3: Validate the workflow YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('ci.yml OK')"
```

Expected: `ci.yml OK`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "ci(epic-2.4): cargo-semver-checks lane gating tau-ports ABI breaks"
```

---

### Task 3: the compat/versioning policy doc

**Files:**
- Create: `docs/explanation/contract-compatibility.md`
- Modify: `docs/SUMMARY.md` (add under the explanation section)

**Interfaces:**
- Consumes: Tasks 1–2 (the `0.1.0` line + the semver lane it documents).
- Produces: the operator-facing policy page; closes EPIC 2's DoD.

- [ ] **Step 1: Write the policy page**

Create `docs/explanation/contract-compatibility.md`:

```markdown
# Contract compatibility & versioning

tau's public stability surface is the **two contracts** of
[ADR-0056](../decisions/0056-contract-versioning-stability-surface.md): the
authoring/IR schema and the WIT host world. This page is the operator-facing
companion to that ADR — the *how it works in practice*. The ADR holds the
normative breaking-change rules; this page does not restate them, it maps them to
what ships.

## The three version surfaces

| surface | versioned by | who tracks it |
|---|---|---|
| Authoring (IR JSON schema) | the IR `ir_format` field (e.g. `v2.3.0`) | frontend / SDK authors |
| Embedding (WIT host world) | the WIT package version `package tau:run@x.y.z` | wasm-guest embedders (any language) |
| `tau-ports` (the embedding contract's Rust binding) | `tau-ports` crate semver (`0.1.0`) | no_std Rust embedders |

The two *published, conformance-kitted* contracts are the IR schema and the WIT
world. `tau-ports` is the **Rust-native binding** of the embedding contract, not a
third published contract — its stability is delivered by ordinary crate semver.

## What guards each surface today

| surface | guard | where |
|---|---|---|
| IR schema | byte-equal drift test + conformance kit | `tau-ir` `schema_export` / `schema_conformance` |
| WIT host world | parse-based freeze/drift test (frozen 3-function surface) | `tau-wasm-host/tests/wit_host_drift.rs` |
| `tau-ports` ABI | `cargo-semver-checks` vs `origin/main` (break ⇒ version bump) | CI job `ports-semver` |

## `tau-ports` is the one independently-versioned crate

The workspace is `0.0.0`; `tau-ports` deliberately carries its own `0.1.0`. It is
the embedding contract's Rust binding (ADR-0056), so it is the one crate whose ABI
is semver-gated. Path-dependency consumers are unaffected. **Do not "fix" this back
to the workspace version** — the independent line is what makes the
`cargo-semver-checks` gate meaningful.

The gate: a breaking change to `tau-ports`'s public API must be **declared** by an
adequate version bump (at `0.x`, a break is a minor bump, `0.1.0 → 0.2.0`). Additive
changes need no bump. Breaking is allowed — but never implicitly; the bump is the
explicit, versioned acknowledgement.

## Pre-1.0 posture and the path to 1.0

Everything is `0.x` and unpublished. Breaking changes are permitted, but each must
be declared (a version bump for `tau-ports`; an `ir_format` / WIT-package bump for
the contracts, per ADR-0056). When the project baselines and publishes crates, the
IR and WIT contracts graduate to `1.0.0` and semver tightens to full
backwards-compatibility guarantees.

## Naming & wording notes

- The WIT package is **`tau:run`**, not `tau:host`. ADR-0056 wrote `tau:host@x.y.z`
  *illustratively* (to show the version-by-WIT-package mechanism); the shipped
  package is `tau:run` because it carries both the host imports and the `run` export.
- ADR-0055 says the WIT world is "generated from the ports." Read this as **provably
  non-drifting**: there is no Rust-trait→WIT generator (and the boundary is
  JSON-stringly-typed by design), so the guarantee is delivered by the drift test,
  not literal code generation.

## How to make a breaking change to each contract

- **IR schema:** bump `ir_format` per ADR-0056 (major for a removed/retyped field,
  minor for additive), regenerate `schemas/ir/tau-ir.v<X>.schema.json`, update the
  conformance kit. The drift test enforces regeneration.
- **WIT host world:** edit `wit/tau-run.wit` and update `wit_host_drift.rs` + the
  bindings deliberately; bump the WIT package version. The freeze test is the
  tripwire.
- **`tau-ports`:** make the change and bump `tau-ports`'s version. `cargo-semver-checks`
  fails the PR if the bump is inadequate for the API delta.
```

- [ ] **Step 2: Add it to SUMMARY.md**

In `docs/SUMMARY.md`, under the explanation section, after `- [Capabilities and consent](explanation/capabilities-and-consent.md)` (line 58), insert:

```
- [Contract compatibility & versioning](explanation/contract-compatibility.md)
```

(Any stable position within the explanation list is fine; placing it near capabilities/architecture keeps related concepts together. The requirement is that it is listed at all — mdbook silently skips unlisted pages.)

- [ ] **Step 3: Build the book**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines, exit 0. (If mdbook binaries are missing, STOP and report BLOCKED — do not cargo-install.)

- [ ] **Step 4: Commit**

```bash
git add docs/explanation/contract-compatibility.md docs/SUMMARY.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(epic-2.4): contract compat/versioning policy page (closes EPIC 2)"
```

---

## Self-Review

**Spec coverage:** `tau-ports` independent `0.1.0` (decision 1) → Task 1; `cargo-semver-checks` lane / break-⇒-bump gate (decision 2) → Task 2; policy doc consolidating the version model, the `tau-ports` resolution, the `tau:run` + "provably non-drifting" clarifications, the enforcement map, and the deprecation/migration/path-to-1.0 policy (decision 3) → Task 3. Out-of-scope items (no publishing, no 1.0 graduation, no other crate versioned, no ADR amendment) are respected — no task does them. No gaps.

**Placeholder scan:** no TBD/TODO. The one genuine uncertainty — the exact `cargo-semver-checks` CI invocation (nightly + `--baseline-rev`) — is given a concrete, runnable job spec plus an explicit "CI run is the verification" note and a `taiki-e/install-action` pin instruction; not left vague.

**Type consistency:** `tau-ports@0.1.0`, `tau:run`, the job name `ports-semver`, the test/guard names referenced (`schema_export`/`schema_conformance`/`wit_host_drift`), and the doc path `docs/explanation/contract-compatibility.md` are identical across all tasks and the Global Constraints.
