# EPIC 2.3 — WIT Host World Freeze + ports↔WIT Drift Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Freeze the minimal 3-function WIT host surface and prove it stays in correspondence with the `tau-ports` traits it projects, via a `wit-parser`-based drift/freeze test.

**Architecture:** Approach B (authored canonical `.wit` + drift test, not codegen). A test in `tau-wasm-host` parses `wit/tau-run.wit` with `wit-parser` and asserts the `host` interface is exactly the 3 expected functions (frozen), the package is `tau:run@0.1.0`, the `runner` world imports `host`/exports `run`, and the function set matches a Rust host-port registry. The existing compile-time link in `host_ports.rs` remains the signature-drift guard. A docs page + CI lane give symmetry with 2.2.

**Tech Stack:** Rust, `wit-parser` 0.251 (test-only), mdbook.

## Global Constraints

- **CARGO RULES (repo CLAUDE.md):** every cargo command `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate> ...`. Never bare/`--workspace`. test 300s, check/build 180s.
- **Approach is B (drift test), NOT codegen.** Do NOT generate the `.wit`; do NOT rename the package; do NOT add a `build.rs` emitter. `wit/tau-run.wit` stays the authored canonical artifact.
- **Package/version stay `tau:run@0.1.0`** (ADR-0056's `tau:host` was illustrative). Do NOT rename or bump.
- **Freeze the HOST interface only.** Assert the `run` export *exists* but do NOT freeze its payload (explicitly out of scope).
- **The frozen surface (verbatim) — `interface host` in `wit/tau-run.wit`:**
  - `complete: func(request-json: string) -> result<string, string>`
  - `now-millis: func() -> u64`
  - `next-u64: func() -> u64`
- **The host-port registry (the Rust source the WIT is checked against):**
  `&[("complete", "LlmBackend"), ("now-millis", "Clock"), ("next-u64", "RandomSource")]`
- **`wit-parser` version:** add as a workspace dep pinned to the version `wit-bindgen` 0.58 already resolves (`0.251` per `Cargo.lock`) so no second copy is introduced. Verify it unifies after adding.
- **Commit identity:** `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit` (use `--no-verify` only if the lefthook corrupts git identity — documented CLAUDE.md behavior).
- **Branch:** `feat/epic-2-3-wit-host-world` (already checked out). Do not rename.

---

### Task 1: ports↔WIT drift/freeze test

**Files:**
- Modify: `Cargo.toml` (add `wit-parser` to `[workspace.dependencies]`)
- Modify: `crates/tau-wasm-host/Cargo.toml` (add `wit-parser` dev-dependency)
- Create: `crates/tau-wasm-host/tests/wit_host_drift.rs`

**Interfaces:**
- Consumes: `wit/tau-run.wit` (authored), `wit-parser` 0.251 API.
- Produces: a green drift/freeze test. The host-port registry const lives in this test file.

- [ ] **Step 1: Add `wit-parser` to the workspace**

In `Cargo.toml` `[workspace.dependencies]`, near the `wit-bindgen` line, add:

```toml
wit-parser = "0.251"
```

In `crates/tau-wasm-host/Cargo.toml` under `[dev-dependencies]`, add:

```toml
wit-parser = { workspace = true }
```

Verify it unifies (no second `wit-parser` version added):

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-wasm-host --tests
```

Expected: compiles. If cargo resolves a *new* `wit-parser` version distinct from the one `wit-bindgen` 0.58 uses, adjust the version string to match the existing resolved entry in `Cargo.lock` (the goal is one copy).

- [ ] **Step 2: Write the drift/freeze test (failing first)**

Create `crates/tau-wasm-host/tests/wit_host_drift.rs`:

```rust
//! Freezes the minimal WIT host surface (EPIC 2.3, ADR-0056) and proves it stays
//! in correspondence with the `tau-ports` traits it projects.
//!
//! Signature drift between these 3 functions and their ports already breaks
//! compilation via `tau-wasm-guest/src/host_ports.rs` (the `LlmBackend`/`Clock`/
//! `RandomSource` impls over the WIT-generated imports). THIS test freezes the
//! *set* and *shape* of the host surface so growing it is a deliberate,
//! test-breaking act. The `run` export payload is intentionally NOT frozen.

use std::collections::BTreeSet;
use std::path::PathBuf;
use wit_parser::Resolve;

/// The single Rust declaration of the host-crossing surface: each WIT host
/// function and the `tau-ports` trait it projects. The `.wit` is checked
/// against this.
const HOST_PORT_REGISTRY: &[(&str, &str)] = &[
    ("complete", "LlmBackend"),
    ("now-millis", "Clock"),
    ("next-u64", "RandomSource"),
];

fn wit_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/tau-wasm-host ; repo root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../wit/tau-run.wit")
}

fn load() -> Resolve {
    let mut resolve = Resolve::new();
    // tau-run.wit is self-contained (no package deps), so push_file suffices.
    resolve
        .push_file(wit_path())
        .expect("parse wit/tau-run.wit");
    resolve
}

#[test]
fn package_is_tau_run_0_1_0() {
    let resolve = load();
    let pkg = resolve.packages.iter().next().expect("one package").1;
    assert_eq!(pkg.name.namespace, "tau");
    assert_eq!(pkg.name.name, "run");
    assert_eq!(
        pkg.name.version.as_ref().map(|v| v.to_string()),
        Some("0.1.0".to_string()),
        "embedding-contract version (ADR-0056) must stay tau:run@0.1.0"
    );
}

#[test]
fn host_interface_is_frozen_to_the_three_functions() {
    let resolve = load();
    let host = resolve
        .interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some("host"))
        .map(|(_, i)| i)
        .expect("`host` interface present");

    let got: BTreeSet<&str> = host.functions.keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = HOST_PORT_REGISTRY.iter().map(|(f, _)| *f).collect();
    assert_eq!(
        got, want,
        "host surface drifted; update wit/tau-run.wit AND host_ports.rs AND this \
         test + the registry deliberately (ADR-0056 freeze)"
    );
}

#[test]
fn host_function_param_shapes_are_frozen() {
    let resolve = load();
    let host = resolve
        .interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some("host"))
        .map(|(_, i)| i)
        .expect("`host` interface present");

    // complete(request-json: string) -> result<string, string>
    let complete = &host.functions["complete"];
    let cparams: Vec<&str> = complete.params.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(cparams, vec!["request-json"], "complete params frozen");

    // now-millis() -> u64  and  next-u64() -> u64  take no params
    assert!(host.functions["now-millis"].params.is_empty(), "now-millis takes no params");
    assert!(host.functions["next-u64"].params.is_empty(), "next-u64 takes no params");
}

#[test]
fn runner_world_imports_host_and_exports_run() {
    let resolve = load();
    let world = resolve
        .worlds
        .iter()
        .find(|(_, w)| w.name == "runner")
        .map(|(_, w)| w)
        .expect("`runner` world present");

    let import_names: BTreeSet<String> = world
        .imports
        .keys()
        .map(|k| format!("{k:?}"))
        .collect();
    assert!(
        import_names.iter().any(|k| k.contains("host")),
        "runner must import the host interface; got {import_names:?}"
    );
    let export_names: BTreeSet<String> = world
        .exports
        .keys()
        .map(|k| format!("{k:?}"))
        .collect();
    assert!(
        export_names.iter().any(|k| k.contains("run")),
        "runner must export run; got {export_names:?}"
    );
}
```

NOTE on the `wit-parser` 0.251 API: the field/method names above (`Resolve::push_file`, `resolve.packages`/`interfaces`/`worlds` as `Arena`/`Id`-keyed iterables, `Interface.functions` as an `IndexMap<String, Function>`, `Function.params: Vec<(String, Type)>`, `PackageName { namespace, name, version }`, `World.imports`/`exports` keyed by `WorldKey`) are correct for the 0.25x line but verify against the installed version while implementing and adjust accessors if a field was renamed. The four assertions (package id+version, frozen function-name set vs the registry, param shapes, world import/export presence) are the contract — keep them even if the access path changes.

- [ ] **Step 3: Run the test — verify it passes**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-wasm-host --test wit_host_drift
```

Expected: 4 tests PASS. (TDD note: to confirm the freeze actually bites, temporarily add a dummy `ping: func();` to the `host` interface in `wit/tau-run.wit`, re-run, watch `host_interface_is_frozen_to_the_three_functions` FAIL, then revert.)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tau-wasm-host/Cargo.toml crates/tau-wasm-host/tests/wit_host_drift.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-2.3): freeze WIT host world + ports↔WIT drift test"
```

---

### Task 2: docs reference page + CI lane

**Files:**
- Create: `docs/reference/wit-host-world.md`
- Modify: `docs/SUMMARY.md` (add under the reference section, after the `IR JSON Schema` line)
- Modify: `.github/workflows/ci.yml` (add a `wit-host-drift` job)

**Interfaces:**
- Consumes: Task 1 (the test file name `wit_host_drift`).
- Produces: the documented embedding-contract surface + CI enforcement.

- [ ] **Step 1: Write the reference page**

Create `docs/reference/wit-host-world.md`:

```markdown
# WIT host world (embedding contract)

tau's **embedding contract** (ADR-0056) is the WIT host world in
[`wit/tau-run.wit`](https://github.com/tau-rs/tau/blob/main/wit/tau-run.wit) —
`package tau:run@0.1.0`. Language-neutral embedders consume it via wit-bindgen / jco.

The host world has a **frozen minimal 3-function surface** — the ports the guest
cannot satisfy in-wasm, projected across the boundary:

| WIT host function | signature | tau-ports trait |
|---|---|---|
| `complete` | `func(request-json: string) -> result<string, string>` | `llm::LlmBackend` (JSON-serialized request/response) |
| `now-millis` | `func() -> u64` | `time::Clock` |
| `next-u64` | `func() -> u64` | `random::RandomSource` |

The surface is frozen and drift-tested (`tau-wasm-host/tests/wit_host_drift.rs`):
adding, removing, renaming, or re-shaping a host function fails the test
deliberately. Signature drift between these functions and their ports also breaks
compilation via `tau-wasm-guest/src/host_ports.rs`.

The `runner` world also **exports** `run`; that payload is not yet frozen and the
package stays `0.x` until it settles (then it graduates to `1.0.0` under ADR-0056's
embedding-contract semver). The package is named `tau:run` (it carries both the
host imports and the `run` export); ADR-0056's `tau:host` was illustrative of the
versioning mechanism.
```

- [ ] **Step 2: Add it to SUMMARY.md**

In `docs/SUMMARY.md`, immediately after the line `- [IR JSON Schema](reference/ir-json-schema.md)`, insert:

```
- [WIT host world](reference/wit-host-world.md)
```

- [ ] **Step 3: Build the book**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines, exit 0. (If mdbook binaries are missing, STOP and report BLOCKED — do not cargo-install.)

- [ ] **Step 4: Add the CI lane**

Read `.github/workflows/ci.yml` and find the `schema-conformance` job added by EPIC 2.2 (it runs `cargo test -p tau-ir --features schema ...` on linux-stable). Add a sibling job mirroring its setup exactly (same checkout SHA, `./.github/actions/setup-rust`, `toolchain: stable`, `shared-key: linux-stable`, sccache/mold, `timeout-minutes`):

```yaml
  wit-host-drift:
    name: WIT host world (drift + freeze)
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10 # v6
      - uses: ./.github/actions/setup-rust
        with:
          toolchain: stable
          shared-key: linux-stable
          with-sccache: "true"
          with-mold: "true"
      - run: cargo test -p tau-wasm-host --test wit_host_drift
```

Copy the EXACT `with:` keys/values from the `schema-conformance` job (the snippet above mirrors it; reconcile any difference in favour of what that job actually uses). `ci-summary` aggregates the overall CI conclusion dynamically, so no change to `ci-summary.yml` is needed. Validate YAML:

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"
```

- [ ] **Step 5: Commit**

```bash
git add docs/reference/wit-host-world.md docs/SUMMARY.md .github/workflows/ci.yml
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "docs(epic-2.3): WIT host world reference page + CI drift lane"
```

---

## Self-Review

**Spec coverage:** drift/freeze test (spec component 2) → Task 1; host-port registry (component 1) → Task 1 const; signature-drift compile guard (component 3) → documented in the test's module doc (Task 1) + the docs page (Task 2); package/version KEEP → Task 1 `package_is_tau_run_0_1_0` + Global Constraints; freeze host-only / `run` export presence-only → Task 1 `runner_world...` test + Global Constraints; published-contract symmetry (docs page + CI) → Task 2. Honest residual gap → documented in the docs page + test doc, no code (correctly — it's unenforceable by design). No gaps.

**Placeholder scan:** no TBD/TODO. The one discovery point — exact `wit-parser` 0.251 accessor names — is bounded by a NOTE naming the four contractual assertions to preserve regardless of access path, plus a `cargo check` gate (Task 1 Step 1) and the passing-test gate (Step 3).

**Type consistency:** the registry literal, the three function names (`complete`/`now-millis`/`next-u64`), the package id (`tau:run@0.1.0`), the test file name (`wit_host_drift`), and the trait names (`LlmBackend`/`Clock`/`RandomSource`) are identical across Tasks 1–2 and the Global Constraints.
