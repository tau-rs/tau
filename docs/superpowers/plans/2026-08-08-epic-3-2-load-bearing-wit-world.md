# EPIC 3.2 Load-Bearing WIT World — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tau build wasm` compile the guest component against the capability-derived WIT world (produced by the merged `generate_world`), so an ungranted capability's WASI interface is provably absent from the world the component is bound to.

**Architecture:** Reuse the existing per-build injection pattern — `tau build wasm` writes the generated world to a tempfile and passes it via `TAU_WORLD_WIT`; the guest `build.rs` copies it into a gitignored `wit-gen/runner.wit` (falling back to a committed baseline when unset, for CI); `wit_bindgen::generate!` binds `tau:generated/runner` from `["wit-gen","wit"]` with vendored WASI 0.2.3 deps. A spike (Task 3) decides whether `no_std` + WASI bindgen compiles: pass → Tier 1 (Tasks 4–6); fail → Tier 2 fallback (Task 7, `wit-parser` validation instead of recompiling the guest against the world).

**Tech Stack:** Rust, `wit-bindgen` 0.58 (`macros`,`realloc`), `wit-parser`/`wit-component` (already in tree), `wasm32-wasip2`, vendored WASI 0.2.3 `.wit`.

**Spec:** `docs/superpowers/specs/2026-08-08-epic-3-2-load-bearing-wit-world-design.md`

## Global Constraints

- **CARGO RULES (CLAUDE.md):** never bare `cargo`. Every invocation:
  `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`.
  Timeouts: test 300s, build/check 180s, clippy 240s, fmt 30s. Prefer `cargo nextest run`; doctests via `cargo test --doc`. Check `pgrep -af cargo` before launching to avoid lock contention.
- **Guest wasm builds** shell `cargo build -p tau-wasm-guest --target wasm32-wasip2` and use the dedicated `CARGO_TARGET_DIR=target/tau-build-wasm` (already hardcoded in `build_wasm.rs`); the spike may use `target/agent-spike-wasm`. Requires `rustup target add wasm32-wasip2`.
- **WASI version pin:** `0.2.3`, the single source of truth being `tau_ports::target::wasi_map::WASI_VERSION`. Vendored deps and the closure table must all read `@0.2.3`.
- **`tau-wasm-guest` is wasm-only:** every module is `#[cfg(target_arch="wasm32")]`; on host the crate is empty. `cargo check --workspace` never expands the bindgen macro — only a real `wasm32-wasip2` build does.
- **Never dirty the git tree during a build:** `wit-gen/` is gitignored; no committed file is overwritten mid-build.
- **`wit/tau-host.wit` stays frozen** (guarded by `tau-wasm-host/tests/wit_host_drift.rs`) — do not edit it; do not add a `runner.wit` under `wit/`.
- **Do not touch** `tau-ports::target::wit_world` (the merged generator) or `world_from_module` — consume them as-is.
- Commits: conventional, imperative, scoped `epic-3-2`. Do not commit unless the executor’s workflow says to; use `--no-verify` only per CLAUDE.md’s docs/YAML exception (this plan touches Rust, so run the normal gate).

---

### Task 1: Vendor WASI 0.2.3 `.wit` deps + version-pin test

**Files:**
- Create: `crates/tau-wasm-guest/wit/deps/wasi-io/*.wit`, `.../wasi-clocks/*.wit`, `.../wasi-filesystem/*.wit`, `.../wasi-http/*.wit`
- Create: `crates/tau-wasm-guest/tests/wit_world.rs`
- Modify: `crates/tau-wasm-guest/Cargo.toml` (add `[dev-dependencies] tau-ports`)

**Interfaces:**
- Consumes: `tau_ports::target::wasi_map::WASI_VERSION` (`&str = "0.2.3"`).
- Produces: a vendored, self-consistent WASI 0.2.3 package set resolvable by both `wit_bindgen` (Task 4) and `wit-parser` (Task 7).

Vendor exactly the packages the generated world imports (`wit_world.rs` closure): `wasi:io@0.2.3` (`error`,`poll`,`streams`), `wasi:clocks@0.2.3` (`monotonic-clock`,`wall-clock`), `wasi:filesystem@0.2.3` (`types`,`preopens`), `wasi:http@0.2.3` (`types`,`outgoing-handler`). Source them from the pinned upstream `v0.2.3` tags (`github.com/WebAssembly/wasi-io`, `wasi-clocks`, `wasi-filesystem`, `wasi-http`) — the `wasi-http` v0.2.3 `wit/deps` bundle already contains the io/clocks/filesystem deps it needs, so copying `wasi-http@v0.2.3/wit/` + its `deps/` is the least-error path. Every package header must read `@0.2.3`.

- [ ] **Step 1: Vendor the files.** Copy the pinned `.wit` files into `crates/tau-wasm-guest/wit/deps/<pkg>/`. Keep upstream file names. Do not edit their contents.

- [ ] **Step 2: Add the dev-dep.** In `crates/tau-wasm-guest/Cargo.toml`:

```toml
[dev-dependencies]
tau-ports = { workspace = true }
```

- [ ] **Step 3: Write the failing version-pin test** in `crates/tau-wasm-guest/tests/wit_world.rs`:

```rust
use std::path::PathBuf;

fn guest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every vendored WASI package must be pinned to WASI_VERSION so the closure
/// table (wit_world.rs) and the vendored .wit cannot drift apart.
#[test]
fn vendored_wasi_versions_match_pin() {
    let pin = format!("@{}", tau_ports::target::wasi_map::WASI_VERSION); // "@0.2.3"
    let deps = guest_dir().join("wit/deps");
    let mut checked = 0usize;
    for entry in walk_wit(&deps) {
        let text = std::fs::read_to_string(&entry).unwrap();
        for line in text.lines() {
            let l = line.trim_start();
            if l.starts_with("package wasi:") {
                assert!(l.contains(&pin), "unpinned package in {}: {l}", entry.display());
                checked += 1;
            }
        }
    }
    assert!(checked >= 4, "expected >=4 vendored wasi packages, found {checked}");
}

fn walk_wit(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { out.extend(walk_wit(&p)); }
        else if p.extension().is_some_and(|x| x == "wit") { out.push(p); }
    }
    out
}
```

- [ ] **Step 3b: Run it to verify it fails** (before vendoring is complete / if a package is unpinned):
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-guest --test wit_world`
Expected initially: FAIL (missing dir or `checked < 4`).

- [ ] **Step 4: Vendor until green.** Re-run the command above. Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/tau-wasm-guest/wit/deps crates/tau-wasm-guest/tests/wit_world.rs crates/tau-wasm-guest/Cargo.toml
git commit -m "feat(epic-3-2): vendor WASI 0.2.3 wit deps + version-pin test"
```

---

### Task 2: Committed baseline world + generator-invariant test

**Files:**
- Create: `crates/tau-wasm-guest/wit-baseline/runner.wit`
- Modify: `crates/tau-wasm-guest/tests/wit_world.rs` (add one test)

**Interfaces:**
- Consumes: `tau_ports::target::wit_world::generate_world(&[]) -> Result<String, _>`.
- Produces: `wit-baseline/runner.wit`, the empty-cap world used as the CI/standalone fallback (Task 4). It is deliberately **off** the bindgen path so it never collides with `wit-gen/runner.wit`.

- [ ] **Step 1: Write the failing invariant test** (append to `tests/wit_world.rs`):

```rust
/// The committed baseline MUST be byte-identical to the empty-cap generator
/// output, so the fallback world CI compiles cannot drift from generate_world.
#[test]
fn baseline_equals_empty_generate_world() {
    let baseline = std::fs::read_to_string(guest_dir().join("wit-baseline/runner.wit"))
        .expect("baseline present");
    let generated = tau_ports::target::wit_world::generate_world(&[]).unwrap();
    assert_eq!(baseline, generated);
}
```

- [ ] **Step 2: Run to verify it fails**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-guest --test wit_world::baseline_equals_empty_generate_world`
Expected: FAIL (file missing).

- [ ] **Step 3: Create the baseline** by writing the exact `generate_world(&[])` output to `crates/tau-wasm-guest/wit-baseline/runner.wit`:

```wit
package tau:generated@0.1.0;

world runner {
    import host;

    export run: func(prompt: string) -> result<string, string>;
}
```
(If the assertion still fails, print `generate_world(&[])` and copy it verbatim — the generator is the source of truth, not this snippet.)

- [ ] **Step 4: Run to verify it passes.** Same command as Step 2. Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/tau-wasm-guest/wit-baseline/runner.wit crates/tau-wasm-guest/tests/wit_world.rs
git commit -m "feat(epic-3-2): committed empty-cap baseline world + generator invariant"
```

---

### Task 3: SPIKE — does `no_std` + WASI bindgen compile? (selects the tier)

**Files:** none committed (throwaway probe). Record the outcome in the PR description and check the box that applies below.

**Goal:** Decide Tier 1 (compile the guest against the cap-world) vs Tier 2 (validate the world with `wit-parser`, leave the guest bound to the frozen world). The risk: `wit_bindgen` generating `wasi:http`/`wasi:filesystem` binding modules that may pull `std` into the `no_std` guest.

- [ ] **Step 1: Build a net+fs probe world.** Create a scratch dir `target/spike-wit/` containing: a copy of `crates/tau-wasm-guest/wit/deps/`, a copy of `crates/tau-wasm-guest/wit/tau-host.wit`, and `runner.wit` = the output of `generate_world(&[cap_net_http(&["h"],&["POST"]), cap_fs_read(&["/d"])])` (import both `wasi:http` and `wasi:filesystem` + closure).

- [ ] **Step 2: Temporarily point the guest at it.** On a throwaway branch, set `guest.rs` bindgen to `{ world: "tau:generated/runner", path: "<abs>/target/spike-wit" }` and build:
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-spike-wasm cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`

- [ ] **Step 3: Record the verdict.**
  - **PASS** (compiles): proceed with **Tasks 4, 5, 6** (Tier 1). Skip Task 7.
  - **FAIL** (missing `std`/`alloc` symbols, unresolved imports, or realloc/allocator errors specific to the wasi modules): proceed with **Task 7** (Tier 2). Skip Tasks 4–6. Note the exact error in the PR body.

- [ ] **Step 4: Discard the probe.** `git checkout -- crates/tau-wasm-guest/src/guest.rs`; `rm -rf target/spike-wit`. No commit.

---

## Tier 1 (spike PASSED) — Tasks 4–6

### Task 4: `TAU_WORLD_WIT` build.rs plumbing + bindgen switch

**Files:**
- Modify: `crates/tau-wasm-guest/build.rs`
- Modify: `crates/tau-wasm-guest/src/guest.rs:12-15`
- Create: `crates/tau-wasm-guest/.gitignore`

**Interfaces:**
- Consumes: env `TAU_WORLD_WIT` (path to cap-derived world text), set by Task 5.
- Produces: a guest whose `wit_bindgen` world is `wit-gen/runner.wit` (dynamic) resolved with `wit/` (deps + tau-host); standalone builds (no env) fall back to `wit-baseline/runner.wit`.

- [ ] **Step 1: Add `.gitignore`.** `crates/tau-wasm-guest/.gitignore`:
```
/wit-gen/
```

- [ ] **Step 2: Extend `build.rs`** to populate `wit-gen/runner.wit` (append after the IR-baking block, before `main` returns):

```rust
// Populate wit-gen/runner.wit: the cap-derived world from TAU_WORLD_WIT (set by
// `tau build wasm`), or the committed empty-cap baseline for standalone/CI builds.
let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
let wit_gen = manifest.join("wit-gen");
std::fs::create_dir_all(&wit_gen).expect("mkdir wit-gen");
println!("cargo:rerun-if-env-changed=TAU_WORLD_WIT");
let world = match std::env::var_os("TAU_WORLD_WIT") {
    Some(path) => {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        std::fs::read(&path).unwrap_or_else(|e| panic!("reading TAU_WORLD_WIT {}: {e}", path.display()))
    }
    None => {
        let base = manifest.join("wit-baseline/runner.wit");
        println!("cargo:rerun-if-changed={}", base.display());
        std::fs::read(&base).expect("reading wit-baseline/runner.wit")
    }
};
std::fs::write(wit_gen.join("runner.wit"), world).expect("writing wit-gen/runner.wit");
```

- [ ] **Step 3: Switch the bindgen** in `guest.rs:12-15`:

```rust
wit_bindgen::generate!({
    world: "tau:generated/runner",
    path: ["wit-gen", "wit"],
});
```
Leave the `wit_host` re-export module below it unchanged — `import host` still resolves via `tau:host`.

- [ ] **Step 4: Verify standalone build (baseline, no env) compiles.**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/tau-build-wasm cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
Expected: PASS (world = baseline, host-only, no WASI). Confirm `git status` shows `wit-gen/` untracked/ignored, tree otherwise clean.

- [ ] **Step 5: Commit.**
```bash
git add crates/tau-wasm-guest/build.rs crates/tau-wasm-guest/src/guest.rs crates/tau-wasm-guest/.gitignore
git commit -m "feat(epic-3-2): inject cap-derived world into guest build via TAU_WORLD_WIT"
```

---

### Task 5: `tau build wasm` sets `TAU_WORLD_WIT` + reproducibility assertion

**Files:**
- Modify: `crates/tau-cli/src/cmd/build_wasm.rs` (`build_guest_with_ir` + `run`)
- Modify: `crates/tau-cli/tests/cmd_build_wasm.rs`

**Interfaces:**
- Consumes: `world_from_module(&module) -> Result<String>` (already computed in `run` at the `world` binding).
- Produces: the guest build receives the world via env; `<out>.wit` continues to be emitted and is asserted byte-equal to the generator.

- [ ] **Step 1: Thread the world path into the guest build.** Change `build_guest_with_ir(ir_path: &Path)` to `build_guest_with_ir(ir_path: &Path, world_path: &Path)` and add to its `Command`:
```rust
        .env("TAU_WORLD_WIT", world_path)
```

- [ ] **Step 2: Write the world tempfile in `run`** (the `world` string is already computed before the build). Replace the build call site so the world is written to a `NamedTempFile` kept alive across the build, mirroring the IR tempfile:
```rust
    let world_file = tempfile::NamedTempFile::new().context("creating world scratch file")?;
    std::fs::write(world_file.path(), world.as_bytes()).context("writing world scratch bytes")?;
    let wasm = build_guest_with_ir(ir_file.path(), world_file.path())?;
    drop(ir_file);
    drop(world_file);
```
Keep the existing `<out>.wit` write (`wit_path`) exactly as-is.

- [ ] **Step 3: Write the failing reproducibility test** in `cmd_build_wasm.rs` (host-only, no wasm build — uses the existing `wasm_world_for_project` seam):
```rust
#[test]
fn emitted_world_is_deterministic_and_matches_generator() {
    let a = wasm_world_for_project(&fixture("net-http")).unwrap();
    let b = wasm_world_for_project(&fixture("net-http")).unwrap();
    assert_eq!(a, b, "world generation must be byte-deterministic");
    // The net-http fixture grants net → the world imports wasi:http, not wasi:filesystem.
    assert!(a.contains("import wasi:http/outgoing-handler@0.2.3;"));
    assert!(!a.contains("wasi:filesystem"));
}
```

- [ ] **Step 4: Run tests.**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_build_wasm`
Expected: PASS (this test) + existing tests still green.

- [ ] **Step 5: Commit.**
```bash
git add crates/tau-cli/src/cmd/build_wasm.rs crates/tau-cli/tests/cmd_build_wasm.rs
git commit -m "feat(epic-3-2): tau build wasm injects the cap-derived world; reproducibility test"
```

---

### Task 6: End-to-end DoD — ungranted cap absent from the component world

**Files:**
- Create: `crates/tau-cli/tests/build_wasm_world_dod.rs`
- Modify: `crates/tau-cli/Cargo.toml` (add `wit-component` dev-dep if not already present)
- Reuse fixtures: `crates/tau-cli/tests/fixtures/wasm-build/{net-http,over-reach}` (add an `fs`-only fixture if `over-reach` is not fs-granting — see Step 1).

**Interfaces:**
- Consumes: `tau_cli::cmd::build_wasm::run` path indirectly by shelling the same guest build; or call the crate build helper. Simplest: shell `cargo build -p tau-wasm-guest --target wasm32-wasip2` with `TAU_WORLD_WIT` set to each fixture's generated world (via `wasm_world_for_project`) and `TAU_IR_BYTES` to its lowered IR, mirroring `tau-wasm-host/tests/roundtrip.rs`.
- Produces: the DoD assertion — ungranted WASI interface absent from the built component's decoded world.

- [ ] **Step 1: Ensure an fs-only fixture exists.** If `fixtures/wasm-build/over-reach` grants `fs.read` and NOT `net.http`, use it. Otherwise create `fixtures/wasm-build/fs-only/tau.toml` granting only `fs.read` (copy an existing fixture, trim caps). Confirm via `wasm_world_for_project(&fixture("fs-only"))` containing `wasi:filesystem` and not `wasi:http`.

- [ ] **Step 2: Write the failing e2e test** (`#[ignore]`, builds the guest — pattern from `roundtrip.rs`):
```rust
//! EPIC 3.2 DoD: an ungranted capability's WASI interface is absent from the
//! world the guest component is compiled against. Builds the wasm guest, so it
//! is #[ignore]d like the other guest-build tests (run with --run-ignored).

use std::path::PathBuf;
use std::process::Command;
use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm-build").join(name)
}

/// Build the guest for a fixture and return the component's imported interface
/// names, decoded from the wasm via wit-component.
fn imported_interfaces(fixture_name: &str) -> Vec<String> {
    let (_module, bytes) = lower_to_wasm_ir(&fixture(fixture_name)).unwrap();
    let world = wasm_world_for_project(&fixture(fixture_name)).unwrap();
    let ir = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir.path(), &bytes).unwrap();
    let wit = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wit.path(), world.as_bytes()).unwrap();

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf();
    let out = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["build","-p","tau-wasm-guest","--target","wasm32-wasip2","--release","--message-format=json"])
        .env("CARGO_INCREMENTAL","0")
        .env("CARGO_TARGET_DIR", root.join("target/tau-build-wasm"))
        .env("TAU_IR_BYTES", ir.path())
        .env("TAU_WORLD_WIT", wit.path())
        .output().unwrap();
    assert!(out.status.success(), "guest build failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let wasm_path = stdout.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| m["target"]["name"].as_str().is_some_and(|n| n=="tau-wasm-guest"||n=="tau_wasm_guest"))
        .flat_map(|m| m["filenames"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_string)).collect::<Vec<_>>())
        .find(|f| f.ends_with(".wasm")).unwrap();
    let wasm = std::fs::read(&wasm_path).unwrap();
    // wit_component::decode of a component yields (Resolve, WorldId) directly.
    let (resolve, world_id) = match wit_component::decode(&wasm).expect("decode component") {
        wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
        _ => panic!("expected a component, got a wit package"),
    };
    resolve.worlds[world_id].imports.keys()
        .filter_map(|k| match k { wit_parser::WorldKey::Interface(id) => resolve.id_of(*id), _ => None })
        .collect()
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn ungranted_net_is_absent_from_component_world() {
    let ifaces = imported_interfaces("fs-only");
    assert!(ifaces.iter().any(|i| i.starts_with("wasi:filesystem/")), "fs granted → present: {ifaces:?}");
    assert!(!ifaces.iter().any(|i| i.starts_with("wasi:http/")), "net ungranted → absent: {ifaces:?}");
}
```
(If `wit_component::decode`’s exact return API differs in the pinned version, adapt: the goal is “list the component’s imported interface package-ids”. `wit-component` re-exports `wit_parser`; `DecodedWasm::resolve()` + `world.imports` is the stable shape.)

- [ ] **Step 3: Add dev-dep if needed.** If `wit-component` isn’t resolvable from `tau-cli` tests, add to `crates/tau-cli/Cargo.toml`:
```toml
[dev-dependencies]
wit-component = { workspace = true }   # or a pinned version matching wit-bindgen 0.58's tree
```
(Prefer a `workspace = true` entry; add the version to `[workspace.dependencies]` if absent.)

- [ ] **Step 4: Run the ignored test.**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test build_wasm_world_dod --run-ignored all`
Expected: PASS — `wasi:filesystem` present, `wasi:http` absent. (If tree-shaking drops the unused `wasi:filesystem` import too, weaken the granted-present assertion to a comment and keep the ungranted-absent assertion, which is the DoD; note this in the PR.)

- [ ] **Step 5: Commit.**
```bash
git add crates/tau-cli/tests/build_wasm_world_dod.rs crates/tau-cli/Cargo.toml crates/tau-cli/tests/fixtures/wasm-build
git commit -m "test(epic-3-2): DoD e2e — ungranted cap absent from component world"
```

---

## Tier 2 (spike FAILED) — Task 7 (instead of Tasks 4–6)

### Task 7: `wit-parser` validation of the generated world against vendored deps

**Files:**
- Modify: `crates/tau-wasm-guest/tests/wit_world.rs` (add resolution tests)
- Modify: `crates/tau-wasm-guest/Cargo.toml` (`[dev-dependencies] wit-parser`)
- Modify: `crates/tau-cli/src/cmd/build_wasm.rs` (`world_from_module` or a new `validate_world` step) + `crates/tau-cli/tests/cmd_build_wasm.rs`

**Interfaces:**
- Consumes: vendored deps (Task 1), `generate_world` output, `wit_parser::Resolve`.
- Produces: a build-time guarantee that the emitted `<out>.wit` resolves cleanly against the vendored WASI deps (well-formed, cap-exact, version-pinned) — without recompiling the guest against it. The guest stays bound to the frozen host-only world.

- [ ] **Step 1: Add the dev-dep.** `crates/tau-wasm-guest/Cargo.toml`:
```toml
[dev-dependencies]
wit-parser = { workspace = true }
tau-ports = { workspace = true }
```

- [ ] **Step 2: Write the failing resolution test** in `tests/wit_world.rs`:
```rust
/// The generated world must resolve against the vendored WASI deps + tau-host,
/// and an ungranted interface must be absent from the resolved world.
#[test]
fn generated_world_resolves_and_excludes_ungranted() {
    use tau_domain::fixtures::cap_fs_read;
    let world_text = tau_ports::target::wit_world::generate_world(&[cap_fs_read(&["/d"])]).unwrap();

    let mut resolve = wit_parser::Resolve::new();
    // Load vendored deps + frozen tau-host into the resolve graph.
    resolve.push_dir(&guest_dir().join("wit")).expect("load wit/ (deps + tau-host)");
    // Parse and merge the generated world.
    let pkg = wit_parser::UnresolvedPackageGroup::parse("generated.wit".as_ref(), &world_text).unwrap();
    let pkg_id = resolve.push_group(pkg).expect("generated world resolves against vendored deps");

    let world_id = resolve.select_world(pkg_id, Some("runner")).unwrap();
    let names: Vec<String> = resolve.worlds[world_id].imports.keys()
        .filter_map(|k| match k { wit_parser::WorldKey::Interface(id) => resolve.id_of(*id), _ => None })
        .collect();
    assert!(names.iter().any(|n| n.starts_with("wasi:filesystem/")), "fs granted → present: {names:?}");
    assert!(!names.iter().any(|n| n.starts_with("wasi:http/")), "net ungranted → absent: {names:?}");
}
```
(Adapt the exact `wit_parser` calls to the pinned API — `Resolve::push_dir`, `push_group`/`push`, `select_world` are the stable shape; the intent is “resolve the generated world against the vendored deps and enumerate its imports”.)

- [ ] **Step 3: Run to verify it fails, then passes.**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-guest --test wit_world`
Expected: FAIL until the generated world + vendored deps resolve cleanly; then PASS.

- [ ] **Step 4: Wire validation into the build.** In `build_wasm.rs`, after `world_from_module` produces the world in `run`, add a `validate_world(&world, vendored_deps_dir)?` call that runs the same `Resolve` resolution and returns `Err` (exit 2) if the world does not resolve — so a malformed world fails the build rather than shipping a bad `<out>.wit`. Keep emitting `<out>.wit`. Add a host-only test in `cmd_build_wasm.rs` asserting `validate_world(&wasm_world_for_project(&fixture("net-http")).unwrap(), ..)` is `Ok`.

- [ ] **Step 5: Commit.**
```bash
git add crates/tau-wasm-guest/tests/wit_world.rs crates/tau-wasm-guest/Cargo.toml crates/tau-cli/src/cmd/build_wasm.rs crates/tau-cli/tests/cmd_build_wasm.rs
git commit -m "feat(epic-3-2): validate generated world against vendored WASI deps (Tier 2)"
```

---

## Final verification (either tier)

- [ ] **Workspace still green:**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-guest` and `-p tau-cli`.
- [ ] **Standalone wasm build green** (CI parity):
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/tau-build-wasm cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
- [ ] **Frozen host WIT untouched:**
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host --test wit_host_drift`
- [ ] **Clippy + fmt:**
`timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-wasm-guest -p tau-cli --all-targets` ; `cargo fmt --check`.
- [ ] **Tree clean:** `git status` shows only intended files; `wit-gen/` ignored.
- [ ] Open PR against `main`, base branch per the branch-protection workflow; reference the spec and note the tier chosen (from Task 3) in the body.
```
