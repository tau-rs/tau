# β.7.5 PR-E2 — Guest drives the IR + real `tau build wasm` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tau build wasm <project>` emit a real `.wasm` component whose `run` export drives the baked workflow IR through `run_ir_streaming` inside the guest and returns the serialized typed `RunEvent` stream — so a trivial 1-agent cassette project runs end-to-end in wasmtime.

**Architecture:** The design's "PR-E" only half-landed: #369 gave the no_std *link* foundation; the guest `run` is still a stub returning `Ok("{}")` and `tau build wasm` still `bail!`s. This plan finishes the other half — the IR-bytes baking handshake (a guest `build.rs` reads `TAU_IR_BYTES`, `include_bytes!`s it), a hand-rolled no_std `block_on`/stream-collect executor (guest futures are backed by synchronous host imports, so a no-op-waker busy-poll is correct), three host-port adapters over the existing WIT imports (`complete`/`now-millis`/`next-u64`), an in-guest `ToolDispatcher` with no tools, and the real `tau build wasm` pipeline (load → lower for `any-wasi-strict` → canonical bytes → shell `cargo build … --target wasm32-wasip2` → emit + hash).

**Tech Stack:** Rust, `no_std` + `alloc`, `wasm32-wasip2`, `wit-bindgen`, `wasmtime` (host side, already wired in PR-D), `serde_json` (no_std/alloc), `futures-core` (no_std `Stream`).

## Global Constraints

- **CARGO discipline (CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Never bare `cargo`, never `--workspace`. Prefer `cargo nextest run` for tests; `cargo test --doc` for doctests. Check `pgrep -af cargo` before launching.
- **no_std purity:** `tau-wasm-guest` is `#![no_std]` + `extern crate alloc`, `crate-type = ["cdylib"]`, wasm32-only (`#[target.'cfg(target_arch = "wasm32")'.dependencies]`). It has a `#[panic_handler]` and `#[global_allocator]`, so it **cannot be unit-tested on the host** — all guest behavior is verified end-to-end through `tau-wasm-host` integration tests. State this in any task that "can't add a unit test."
- **IR format:** current is `v2.0.0` (`tau_ir::module::IrFormatVersion::CURRENT`). Do not change it.
- **Determinism:** guest output bytes must be identical for a fixed `(component, prompt, llm_responses)` triple — the property `WasmProfile` conformance (PR-G) relies on. No wall-clock, no real RNG in the guest; both come from host imports.
- **Target:** lower for `any-wasi-strict` (caps `{FilesystemRead, FilesystemWrite, NetworkHttp}`); a project needing `ProcessExec`/`AgentSpawn` is refused at build time (no `--allow` flag — Rust-like build-time enforcement).
- **Invariant:** every existing test stays green — all 13 conformance fixtures (dev+bundle), 5 bespoke plugins, sandbox e2e, Phase-2 bundle, Skills, tau-workflow v1, and the existing `tau-wasm-host` roundtrip test (it will be updated, not deleted).
- **Commits:** Conventional Commits, imperative, scoped `feat(β.7.5): …`. Commit after each task. Pushing is out of scope for the implementer unless asked.
- **wasm target must be installed:** `rustup target add wasm32-wasip2` (the existing roundtrip test already assumes this).

---

## File Structure

- `crates/tau-cli/src/cmd/build_wasm.rs` — **replace the stub** with the real pipeline. Add a `pub(crate)` helper `lower_to_wasm_ir` so tests and the command share one lowering path.
- `crates/tau-cli/tests/` — new integration test `cmd_build_wasm.rs` (artifact produced + hash; cap-fit refusal).
- `crates/tau-cli/tests/fixtures/wasm-build/` — two fixture projects (`trivial/tau.toml`, `needs-exec/tau.toml`).
- `crates/tau-wasm-guest/build.rs` — **new**; the `TAU_IR_BYTES` → `OUT_DIR/baked_ir.bin` handshake.
- `crates/tau-wasm-guest/src/baked.rs` — **new**; `pub static BAKED_IR: &[u8]` via `include_bytes!`.
- `crates/tau-wasm-guest/src/executor.rs` — **new**; no_std `block_on` + `collect_stream`.
- `crates/tau-wasm-guest/src/host_ports.rs` — **new**; `HostLlmBackend` / `HostClock` / `HostRandom` over the WIT imports.
- `crates/tau-wasm-guest/src/dispatcher.rs` — **new**; `GuestDispatcher` (no tools).
- `crates/tau-wasm-guest/src/guest.rs` — **modify** `run` to decode `BAKED_IR`, drive `run_ir_streaming`, serialize the stream.
- `crates/tau-wasm-guest/src/lib.rs` — **modify** to declare the new modules (wasm-only).
- `crates/tau-wasm-host/tests/roundtrip.rs` — **modify**: existing assertion changes (guest no longer returns `"{}"`); add the baked-IR-driving test. Add `TAU_IR_BYTES` support + dev-deps (`tau-ir`, `tau-ir-lower`, `tau-pkg`, `tempfile`).
- `docs/decisions/0046-wasm-aot-artifact-and-wit-world.md` — **modify**: note the run-driving observable is live (small status edit; full ADR finalization stays PR-G).

---

## Task 1: Real `tau build wasm` pipeline (artifact + hash + cap-fit refusal)

**Files:**
- Modify: `crates/tau-cli/src/cmd/build_wasm.rs` (replace the `bail!` stub)
- Create: `crates/tau-cli/tests/cmd_build_wasm.rs`
- Create: `crates/tau-cli/tests/fixtures/wasm-build/trivial/tau.toml`
- Create: `crates/tau-cli/tests/fixtures/wasm-build/needs-exec/tau.toml`

**Interfaces:**
- Consumes: `crate::cmd::project_load::load_project(&Path) -> Result<LoadedProject>` where `LoadedProject { project_root: PathBuf, project: ProjectConfig }`; `crate::cmd::build::native_tool_hash(&str) -> Option<[u8;32]>`; `crate::cmd::build::hex_lower(&[u8]) -> String`; `tau_ir_lower::{lower_project, Caches, LowerError}`; `tau_ir::{to_canonical_bytes, compute_hash}`; `tau_ports::target::TargetTriple` (`FromStr`) + `tau_ports::target::lookup`.
- Produces: `pub(crate) fn lower_to_wasm_ir(project: &std::path::Path) -> anyhow::Result<(tau_ir::IrModule, Vec<u8>)>` — reused by Task 4's e2e test; and `pub async fn run(&BuildWasmArgs, &mut Output) -> anyhow::Result<()>`.

**Notes for the implementer:**
- The command shells `cargo build -p tau-wasm-guest` against the **tau source workspace**, located at compile time via `env!("CARGO_MANIFEST_DIR")` (the `tau-cli` crate dir) → `parent().parent()` = workspace root. This is the in-repo β.7.5 path; a user-facing `tau run --wasm` that ships the guest is a γ concern. Document this in a code comment.
- Pass the IR bytes to the guest build via the `TAU_IR_BYTES` env var pointing at a `tempfile::NamedTempFile` that must stay alive until after the `cargo` call returns.
- Use a dedicated `CARGO_TARGET_DIR` (`<workspace>/target/tau-build-wasm`) + `CARGO_INCREMENTAL=0` (CLAUDE.md Rules 1 & 4). Parse `--message-format=json` for the artifact path (mirror `crates/tau-wasm-host/tests/roundtrip.rs:build_guest_component`).
- Capability-fit is enforced *inside* `lower_project`; surface `LowerError::CapabilityFitFailed` as a clean `anyhow` error (no bypass flag).

- [ ] **Step 1: Write the fixture projects**

Create `crates/tau-cli/tests/fixtures/wasm-build/trivial/tau.toml`:

```toml
[project]
name = "trivial-wasm"
version = "0.1.0"

[agents.main]
model = "claude"

[agents.main.prompt]
system = "You are a trivial test agent. Reply and stop."

[models.claude]
backend = "anthropic"
model_id = "claude-sonnet-4-6"
```

Create `crates/tau-cli/tests/fixtures/wasm-build/needs-exec/tau.toml` (a tool requiring `process-exec`, which `any-wasi-strict` forbids):

```toml
[project]
name = "needs-exec-wasm"
version = "0.1.0"

[agents.main]
model = "claude"
tools = ["runner"]

[agents.main.prompt]
system = "Uses a process-exec tool."

[models.claude]
backend = "anthropic"
model_id = "claude-sonnet-4-6"

[tools.runner]
kind = "native"
capabilities = ["process-exec"]
```

> If `kind`/`capabilities` field names differ from the project schema, mirror an existing fixture under `crates/tau-pkg/.../fixtures` or `crates/tau-ir-lower/tests`. The only requirement is that lowering this project for `any-wasi-strict` yields `LowerError::CapabilityFitFailed`. Verify the exact TOML shape against `crates/tau-pkg/src/project/project.rs` before finalizing.

- [ ] **Step 2: Write the failing integration test**

Create `crates/tau-cli/tests/cmd_build_wasm.rs`:

```rust
//! `tau build wasm` pipeline tests (β.7.5 PR-E2).
//!
//! These build the real `tau-wasm-guest` component for `wasm32-wasip2`, so
//! they require that target installed (`rustup target add wasm32-wasip2`).

use std::path::PathBuf;

use tau_cli::cmd::build_wasm::lower_to_wasm_ir;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

#[test]
fn trivial_project_lowers_to_wasm_ir() {
    let (module, bytes) = lower_to_wasm_ir(&fixture("trivial")).expect("trivial lowers");
    assert_eq!(module.ir_format.0, "v2.0.0");
    assert!(!bytes.is_empty(), "canonical IR bytes must be non-empty");
    // Re-decoding the bytes yields an equal module (round-trip sanity).
    let decoded = tau_ir::from_canonical_bytes(&bytes).expect("bytes decode");
    assert_eq!(decoded.ir_format.0, module.ir_format.0);
}

#[test]
fn project_needing_process_exec_is_refused() {
    let err = lower_to_wasm_ir(&fixture("needs-exec")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("capability") || msg.contains("CapabilityFit"),
        "expected a capability-fit refusal, got: {msg}"
    );
}
```

> The full `tau build wasm` artifact-production path (shelling cargo) is exercised in Task 4's e2e test — it is slow (a wasm build) and belongs in the integrated DoD test. Task 1's unit-level gate is the lowering + cap-fit logic, which is the part with branching to verify.

- [ ] **Step 3: Run the test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --test cmd_build_wasm 2>&1 | tail -20`
Expected: FAIL to compile — `lower_to_wasm_ir` does not exist yet.

- [ ] **Step 4: Implement the pipeline**

Replace `crates/tau-cli/src/cmd/build_wasm.rs` entirely:

```rust
//! `tau build wasm <project>` — IR-to-wasm AOT compiler (β.7.5).
//!
//! Pipeline: load + validate the project, lower its IR for `any-wasi-strict`
//! (capability-fit refuses `process-exec`/`agent-spawn` here), serialize the
//! canonical IR bytes, bake them into the `tau-wasm-guest` crate via the
//! `TAU_IR_BYTES` env handshake, shell `cargo build … --target wasm32-wasip2`,
//! and emit the produced `.wasm` plus its IR hash.
//!
//! The guest source lives in *this* workspace; the command shells cargo
//! against it (located via `CARGO_MANIFEST_DIR`). Shipping the guest for a
//! user-facing `tau run --wasm` is a γ concern.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::BuildWasmArgs;
use crate::cmd::build::{hex_lower, native_tool_hash};
use crate::cmd::project_load::load_project;
use crate::output::Output;

/// The triple every wasm build lowers for.
const WASM_TARGET: &str = "any-wasi-strict";

/// Load + validate the project and lower its IR for `any-wasi-strict`.
///
/// Returns the lowered module and its canonical bytes. Capability-fit is
/// enforced inside `lower_project`; a `process-exec`/`agent-spawn` project
/// surfaces `LowerError::CapabilityFitFailed` here (no bypass flag).
pub(crate) fn lower_to_wasm_ir(project: &Path) -> Result<(tau_ir::IrModule, Vec<u8>)> {
    let loaded = load_project(project).with_context(|| format!("loading {}", project.display()))?;

    let target: tau_ports::target::TargetTriple = WASM_TARGET
        .parse()
        .expect("any-wasi-strict is a registered triple");
    if tau_ports::target::lookup(&target).is_none() {
        bail!("internal: target `{WASM_TARGET}` not found in the registry");
    }

    let caches = tau_ir_lower::Caches {
        native_tool: &|name: &str| native_tool_hash(name),
        mcp_contract: &|_url| None,
        skill: &|_name| None,
    };

    let module = tau_ir_lower::lower_project(&loaded.project, &target, &caches)
        .map_err(|e| anyhow::anyhow!("lowering for {WASM_TARGET} failed: {e}"))?;
    let bytes = tau_ir::to_canonical_bytes(&module);
    Ok((module, bytes))
}

/// Locate the tau source workspace root at compile time.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tau-cli is two levels below the workspace root")
        .to_path_buf()
}

/// Shell `cargo build -p tau-wasm-guest` with the baked IR and return the
/// produced `.wasm` bytes.
fn build_guest_with_ir(ir_path: &Path) -> Result<Vec<u8>> {
    let root = workspace_root();
    let target_dir = root.join("target/tau-build-wasm");

    let output = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "tau-wasm-guest",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TAU_IR_BYTES", ir_path)
        .output()
        .context("failed to spawn cargo for the guest build")?;

    if !output.status.success() {
        bail!(
            "guest build failed (is wasm32-wasip2 installed?):\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout).context("cargo json output is utf-8")?;
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| {
            m["target"]["name"]
                .as_str()
                .is_some_and(|n| n == "tau-wasm-guest" || n == "tau_wasm_guest")
        })
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .context("no .wasm artifact in cargo output for tau-wasm-guest")?;

    std::fs::read(&wasm_path).with_context(|| format!("reading built component {wasm_path}"))
}

/// CLI entry point for `tau build wasm`.
pub async fn run(args: &BuildWasmArgs, output: &mut Output) -> Result<()> {
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd is readable"));

    let (module, bytes) = lower_to_wasm_ir(&project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));

    // Bake the IR bytes into a tempfile the guest build reads via TAU_IR_BYTES.
    let ir_file = tempfile::NamedTempFile::new().context("creating IR scratch file")?;
    std::fs::write(ir_file.path(), &bytes).context("writing IR scratch bytes")?;

    let wasm = build_guest_with_ir(ir_file.path())?;
    drop(ir_file); // bytes are consumed; safe to remove the scratch file now.

    let out_path = args.output.clone().unwrap_or_else(|| {
        project.join(format!("{}.wasm", project_stem(&project)))
    });
    std::fs::write(&out_path, &wasm).with_context(|| format!("writing {}", out_path.display()))?;

    output.line(&format!(
        "built {} ({} bytes, ir {})",
        out_path.display(),
        wasm.len(),
        ir_hash
    ));
    Ok(())
}

fn project_stem(project: &Path) -> String {
    project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}
```

> Verify `Output` has a `line(&str)` method; if the CLI uses a different sink (e.g. `output.println` / a structured `Output` struct per the logging memory), match the existing pattern in `crates/tau-cli/src/cmd/build.rs`. Add `tempfile` to `crates/tau-cli`'s `[dependencies]` if not already present.

- [ ] **Step 5: Run the test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --test cmd_build_wasm 2>&1 | tail -20`
Expected: PASS (both tests). `trivial_project_lowers_to_wasm_ir` and `project_needing_process_exec_is_refused`.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cmd/build_wasm.rs crates/tau-cli/tests/cmd_build_wasm.rs crates/tau-cli/tests/fixtures/wasm-build crates/tau-cli/Cargo.toml
git commit -m "feat(β.7.5): tau build wasm lowers IR for any-wasi-strict + cap-fit refusal (PR-E2)"
```

---

## Task 2: Guest IR baking handshake (`build.rs` + `BAKED_IR`, decode-or-error)

**Files:**
- Create: `crates/tau-wasm-guest/build.rs`
- Create: `crates/tau-wasm-guest/src/baked.rs`
- Modify: `crates/tau-wasm-guest/src/lib.rs` (declare `baked` module, wasm-only)
- Modify: `crates/tau-wasm-guest/src/guest.rs` (`run` decodes `BAKED_IR`)
- Modify: `crates/tau-wasm-host/tests/roundtrip.rs` (update assertion; add baked-IR support) + `crates/tau-wasm-host/Cargo.toml` (dev-deps)

**Interfaces:**
- Produces: `pub static BAKED_IR: &[u8]` in `crate::baked`; guest `run` semantics — empty `BAKED_IR` → `Err("no baked IR")`; non-empty → decodes via `tau_ir::from_canonical_bytes` and (this task) returns `Ok(module.ir_format.0)`.
- Consumes: nothing new.

**Testability note:** the guest crate is wasm32-only with a `#[panic_handler]`; it has no host-runnable unit tests. Both assertions live in `tau-wasm-host`'s integration test, which builds the guest and runs it under wasmtime.

- [ ] **Step 1: Write the failing test (host-side, drives the guest)**

Modify `crates/tau-wasm-host/tests/roundtrip.rs`. First, make `build_guest_component` accept an optional IR-bytes path:

```rust
/// Build the guest, optionally baking `ir_bytes` via the `TAU_IR_BYTES`
/// handshake. `None` builds with an empty baked IR (the standalone smoke).
fn build_guest_component(ir_bytes: Option<&[u8]>) -> Vec<u8> {
    // ... existing setup (workspace_root, guest_target_dir) unchanged ...

    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(&workspace_root)
        .args([
            "build", "-p", "tau-wasm-guest",
            "--target", "wasm32-wasip2", "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &guest_target_dir);

    let _scratch; // keep the tempfile alive across the build
    if let Some(bytes) = ir_bytes {
        let f = tempfile::NamedTempFile::new().expect("ir scratch");
        std::fs::write(f.path(), bytes).expect("write ir scratch");
        cmd.env("TAU_IR_BYTES", f.path());
        _scratch = Some(f);
    } else {
        _scratch = None;
    }

    let output = cmd.output().expect("failed to spawn cargo to build the guest");
    // ... existing artifact-parsing + std::fs::read unchanged ...
}
```

Then add the new test and update the existing one:

```rust
/// Lower the trivial fixture's IR to canonical bytes (mirrors `tau build wasm`).
fn trivial_ir_bytes() -> Vec<u8> {
    let toml = r#"
[project]
name = "trivial-wasm"
version = "0.1.0"
[agents.main]
model = "claude"
[agents.main.prompt]
system = "Reply and stop."
[models.claude]
backend = "anthropic"
model_id = "claude-sonnet-4-6"
"#;
    let config = tau_pkg::project::ProjectConfig::parse_str(toml).expect("fixture parses");
    let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
    let caches = tau_ir_lower::Caches {
        native_tool: &|_| Some([0u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
    };
    let module = tau_ir_lower::lower_project(&config, &target, &caches).expect("lowers");
    tau_ir::to_canonical_bytes(&module)
}

#[test]
fn guest_with_no_baked_ir_errors() {
    let component = build_guest_component(None);
    let err = tau_wasm_host::run_component(&component, "hi", vec![]).unwrap_err();
    // Empty baked IR → guest returns its error arm.
    assert!(
        matches!(err, tau_wasm_host::WasmHostError::Guest(_)),
        "got: {err:?}"
    );
}

#[test]
fn guest_decodes_baked_ir_format() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let out = tau_wasm_host::run_component(&component, "hi", vec![]).expect("runs");
    assert!(out.contains("v2.0.0"), "guest should echo the IR format, got: {out}");
}
```

> Confirm the exact `ProjectConfig` re-export path (`tau_pkg::project::ProjectConfig` vs `tau_pkg::ProjectConfig`) against `crates/tau-pkg/src/lib.rs`. Delete/replace the prior `roundtrip` assertion that expected `Ok("{}")` — that behavior is gone.

- [ ] **Step 2: Run the test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-wasm-host --test roundtrip 2>&1 | tail -25`
Expected: FAIL — `guest_decodes_baked_ir_format` (guest still returns `"{}"`, no `build.rs`), and compile error until dev-deps are added.

- [ ] **Step 3: Add the guest `build.rs` and `baked` module**

Create `crates/tau-wasm-guest/build.rs`:

```rust
//! Bakes the workflow IR into the guest. `tau build wasm` (and the host
//! roundtrip test) set `TAU_IR_BYTES` to a file of canonical IR bytes; this
//! copies it to `$OUT_DIR/baked_ir.bin`, which `src/baked.rs` `include_bytes!`s.
//! When unset (standalone smoke build) an empty file is written, and the guest
//! `run` returns its error arm.

use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let dest = out.join("baked_ir.bin");

    println!("cargo:rerun-if-env-changed=TAU_IR_BYTES");
    match std::env::var_os("TAU_IR_BYTES") {
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("reading TAU_IR_BYTES {}: {e}", path.display()));
            std::fs::write(&dest, bytes).expect("writing baked_ir.bin");
        }
        None => {
            std::fs::write(&dest, []).expect("writing empty baked_ir.bin");
        }
    }
}
```

Create `crates/tau-wasm-guest/src/baked.rs`:

```rust
//! The workflow IR baked at build time (see `build.rs`). Empty when no
//! `TAU_IR_BYTES` was supplied — the guest `run` then returns its error arm.

/// Canonical IR bytes baked into the component.
pub static BAKED_IR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/baked_ir.bin"));
```

- [ ] **Step 4: Wire the module and decode in `run`**

In `crates/tau-wasm-guest/src/lib.rs`, add (inside the wasm-only `cfg`, alongside the existing `mod guest;`):

```rust
#[cfg(target_arch = "wasm32")]
mod baked;
```

In `crates/tau-wasm-guest/src/guest.rs`, replace the `run` body:

```rust
impl Guest for Component {
    fn run(_prompt: String) -> Result<String, String> {
        let bytes = crate::baked::BAKED_IR;
        if bytes.is_empty() {
            return Err("tau-wasm-guest: no baked IR".to_string());
        }
        let module = tau_ir::from_canonical_bytes(bytes).map_err(|e| e.to_string())?;
        // Task 2 milestone: prove the baked bytes decode. Driving the IR
        // through run_ir_streaming lands in Task 3.
        Ok(module.ir_format.0)
    }
}
```

> Keep `extern crate alloc;` and the `use alloc::string::{String, ToString};` already at the top of `guest.rs`. `module.ir_format.0` is a `String`.

- [ ] **Step 5: Add dev-deps to the host crate**

In `crates/tau-wasm-host/Cargo.toml` `[dev-dependencies]`, add:

```toml
tau-ir = { workspace = true }
tau-ir-lower = { workspace = true }
tau-pkg = { workspace = true }
tempfile = { workspace = true }
```

> Check these alias names exist in the root `[workspace.dependencies]`; `tau-ir`/`tau-ir-lower` were added there in earlier β.7.5 PRs. `tempfile` is already a workspace dep (used elsewhere).

- [ ] **Step 6: Run the test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-wasm-host --test roundtrip 2>&1 | tail -25`
Expected: PASS — `guest_with_no_baked_ir_errors` + `guest_decodes_baked_ir_format`.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-wasm-guest/build.rs crates/tau-wasm-guest/src/baked.rs crates/tau-wasm-guest/src/lib.rs crates/tau-wasm-guest/src/guest.rs crates/tau-wasm-host/tests/roundtrip.rs crates/tau-wasm-host/Cargo.toml
git commit -m "feat(β.7.5): guest bakes + decodes IR via TAU_IR_BYTES handshake (PR-E2)"
```

---

## Task 3: Guest drives `run_ir_streaming` and serializes the typed stream

**Files:**
- Create: `crates/tau-wasm-guest/src/executor.rs`
- Create: `crates/tau-wasm-guest/src/host_ports.rs`
- Create: `crates/tau-wasm-guest/src/dispatcher.rs`
- Modify: `crates/tau-wasm-guest/src/lib.rs` (declare the three modules, wasm-only)
- Modify: `crates/tau-wasm-guest/src/guest.rs` (`run` drives the stream)
- Modify: `crates/tau-wasm-host/tests/roundtrip.rs` (add the driving test)

**Interfaces:**
- Consumes: `tau_runtime_core::interpreter::run_ir_streaming(Arc<IrModule>, &AgentId, Arc<D>, Vec<tau_ir::Message>) -> Result<impl Stream<Item = RunEvent> + 'static, RuntimeError>` where `D: ToolDispatcher + Send + Sync + 'static`; `tau_runtime_core::stream::RunEvent` (serde); `tau_runtime_core::builder::DynLlmBackend`; `tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult}`; `tau_ports::{Clock, RandomSource, LlmBackend, LlmError}`, `tau_ports::llm::{CompletionRequest, CompletionResponse, CompletionStream, batch_to_stream}`; the WIT import module `crate::host` (`complete`/`now_millis`/`next_u64`).
- Produces: guest `run` returns `Ok(json)` where `json` is `serde_json::to_string(&Vec<RunEvent>)`.

**Design decisions (locked here):**
- **Executor:** hand-rolled `block_on`/`collect_stream` with a no-op-waker busy-poll. Correct because every guest future is backed by a *synchronous* host import or in-process `RefCell` state — nothing registers a real wake, so `Poll::Pending` is effectively unreachable; we `spin_loop()` on it defensively.
- **Entry agent:** E2 scope = exactly one agent. Pick the sole agent; error if `agents.len() != 1`. (Trigger-driven entry is PR-F/G.)
- **initial_messages:** `Vec::new()` (mirrors `crates/tau-conformance/src/profile/dev.rs`). The WIT `run(prompt)` arg is accepted but unused in E2 — threading it into a user `Message` is PR-F (avoids the non-deterministic `MessageId::new()` rabbit hole now).
- **LLM:** `stream()` is the path the interpreter uses (it yields `TextDelta`s). `HostLlmBackend::stream` = `batch_to_stream(self.complete(req).await?)` — `batch_to_stream` is already a no_std `tau_ports::llm` fn.

- [ ] **Step 1: Write the failing driving test (host-side)**

Add to `crates/tau-wasm-host/tests/roundtrip.rs`:

```rust
/// A minimal valid CompletionResponse that ends the turn immediately
/// (no tool calls) — the cassette for a 1-agent reply-and-stop scenario.
fn end_turn_response() -> String {
    r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
}

#[test]
fn guest_drives_ir_and_returns_typed_stream() {
    let component = build_guest_component(Some(&trivial_ir_bytes()));
    let out = tau_wasm_host::run_component(&component, "hi", vec![end_turn_response()])
        .expect("guest runs the baked IR");

    // The guest returns a JSON array of RunEvents.
    let events: Vec<tau_runtime_core::stream::RunEvent> =
        serde_json::from_str(&out).expect("guest output is a RunEvent array");

    assert!(
        matches!(events.first(), Some(tau_runtime_core::stream::RunEvent::RunStarted)),
        "stream must start with RunStarted; got {:?}",
        events.first()
    );
    assert!(
        matches!(events.last(), Some(tau_runtime_core::stream::RunEvent::RunCompleted { .. })),
        "stream must end with RunCompleted; got {:?}",
        events.last()
    );
}
```

Add `tau-runtime-core = { workspace = true }` to `crates/tau-wasm-host/Cargo.toml` `[dev-dependencies]` (host-side it builds with default/std features — fine for the test).

- [ ] **Step 2: Run the test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-wasm-host --test roundtrip guest_drives_ir 2>&1 | tail -25`
Expected: FAIL — guest still returns the `ir_format` string, not a RunEvent array.

- [ ] **Step 3: Implement the executor**

Create `crates/tau-wasm-guest/src/executor.rs`:

```rust
//! Single-threaded no_std executor. Guest futures are backed by synchronous
//! host imports + in-process `RefCell` state, so they never register a real
//! wake; a no-op waker with a busy-poll loop drives them to completion.

use alloc::vec::Vec;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use futures_core::Stream;

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| RawWaker::new(core::ptr::null(), &VTABLE), // clone
    |_| {},                                        // wake
    |_| {},                                        // wake_by_ref
    |_| {},                                        // drop
);

fn noop_waker() -> Waker {
    // SAFETY: every vtable fn ignores the data pointer and is a pure no-op.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

/// Drive a future to completion on the single guest thread.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}

/// Drain a stream to completion, collecting every item.
pub fn collect_stream<S: Stream>(stream: S) -> Vec<S::Item> {
    let mut stream = pin!(stream);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut out = Vec::new();
    loop {
        match stream.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(item)) => out.push(item),
            Poll::Ready(None) => return out,
            Poll::Pending => core::hint::spin_loop(),
        }
    }
}
```

- [ ] **Step 4: Implement the host-port adapters**

Create `crates/tau-wasm-guest/src/host_ports.rs`:

```rust
//! Adapters mapping the three `tau:run/host` WIT imports onto the core ports
//! the interpreter consumes: LLM inference, clock, and randomness. All three
//! cross the wasm boundary because credentials (β.5) and determinism live
//! host-side.

use alloc::string::{String, ToString};

use tau_ports::llm::{batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream};
use tau_ports::{Clock, LlmBackend, LlmError, RandomSource};

use crate::host;

/// `LlmBackend` backed by the host `complete` import (cassette in conformance).
pub struct HostLlmBackend;

impl LlmBackend for HostLlmBackend {
    fn name(&self) -> &str {
        "wasm-host"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let req_json =
            serde_json::to_string(&req).map_err(|e| LlmError::Internal { message: e.to_string() })?;
        let resp_json = host::complete(&req_json).map_err(|e| LlmError::Internal { message: e })?;
        serde_json::from_str(&resp_json).map_err(|e| LlmError::Internal { message: e.to_string() })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        // The interpreter streams; replay the whole completion as one batch.
        let resp = self.complete(req).await?;
        Ok(batch_to_stream(resp))
    }
}

/// `Clock` backed by the host `now-millis` import.
pub struct HostClock;

impl Clock for HostClock {
    fn now(&self) -> i64 {
        host::now_millis() as i64
    }
}

/// `RandomSource` backed by the host `next-u64` import.
pub struct HostRandom;

impl RandomSource for HostRandom {
    fn fill(&self, dest: &mut [u8]) {
        let mut i = 0;
        while i < dest.len() {
            let bytes = host::next_u64().to_le_bytes();
            let take = core::cmp::min(8, dest.len() - i);
            dest[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }
}
```

> `host::complete` returns `Result<String, String>` per the WIT (`result<string, string>`). Confirm the generated name is `host::complete` (the `wit_bindgen::generate!` import path) — it may be `crate::tau::run::host` depending on how PR-C named it; match what `guest.rs`'s `wit_bindgen::generate!` produced. Adjust the `use crate::host;` line accordingly.

- [ ] **Step 5: Implement the dispatcher**

Create `crates/tau-wasm-guest/src/dispatcher.rs`:

```rust
//! In-guest `ToolDispatcher` for the E2 cassette-only scenario: no tools,
//! a single host-backed LLM backend, and host-backed clock/random for
//! determinism.

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use serde_json::Value;

use tau_ir::ToolId;
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_ports::{Clock, RandomSource};

pub struct GuestDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn RandomSource>,
}

impl GuestDispatcher {
    pub fn new(
        backend: Arc<dyn DynLlmBackend>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
    ) -> Self {
        Self {
            backend,
            clock,
            random,
        }
    }
}

impl ToolDispatcher for GuestDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let name = tool_id.0.clone();
        Box::pin(async move {
            Err(RuntimeError::Internal {
                message: format!("tool `{name}` invoked but the wasm guest wires no tools (E2)"),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn RandomSource>> {
        Some(self.random.clone())
    }
}
```

> `Pin`/`Box`/`Arc`/`format` come from `alloc`; ensure the `use alloc::...` lines compile under no_std. `serde_json::Value` is available (already a guest dep).

- [ ] **Step 6: Wire modules + rewrite `run`**

In `crates/tau-wasm-guest/src/lib.rs`, add (wasm-only) alongside the others:

```rust
#[cfg(target_arch = "wasm32")]
mod executor;
#[cfg(target_arch = "wasm32")]
mod host_ports;
#[cfg(target_arch = "wasm32")]
mod dispatcher;
```

Replace the `run` body in `crates/tau-wasm-guest/src/guest.rs`:

```rust
impl Guest for Component {
    fn run(_prompt: String) -> Result<String, String> {
        use alloc::sync::Arc;
        use alloc::vec::Vec;

        let bytes = crate::baked::BAKED_IR;
        if bytes.is_empty() {
            return Err("tau-wasm-guest: no baked IR".to_string());
        }
        let module = tau_ir::from_canonical_bytes(bytes).map_err(|e| e.to_string())?;

        // E2 scope: exactly one agent; it is the entry.
        if module.workflow.agents.len() != 1 {
            return Err(alloc::format!(
                "tau-wasm-guest: E2 supports exactly one agent, found {}",
                module.workflow.agents.len()
            ));
        }
        let entry = module
            .workflow
            .agents
            .keys()
            .next()
            .expect("len checked == 1")
            .clone();

        let backend: Arc<dyn tau_runtime_core::builder::DynLlmBackend> =
            Arc::new(crate::host_ports::HostLlmBackend);
        let clock: Arc<dyn tau_ports::Clock> = Arc::new(crate::host_ports::HostClock);
        let random: Arc<dyn tau_ports::RandomSource> = Arc::new(crate::host_ports::HostRandom);
        let dispatcher = Arc::new(crate::dispatcher::GuestDispatcher::new(backend, clock, random));

        let module = Arc::new(module);
        let stream = crate::executor::block_on(
            tau_runtime_core::interpreter::run_ir_streaming(
                module,
                &entry,
                dispatcher,
                Vec::new(),
            ),
        )
        .map_err(|e| e.to_string())?;

        let events = crate::executor::collect_stream(stream);
        serde_json::to_string(&events).map_err(|e| e.to_string())
    }
}
```

> Keep the existing top-of-file `extern crate alloc;` and `wit_bindgen::generate!`. If the generated import module is `crate::tau::run::host` rather than `crate::host`, fix the `use crate::host;` in `host_ports.rs` to match — this is the single most likely compile snag.

- [ ] **Step 7: Run the test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-wasm-host --test roundtrip 2>&1 | tail -30`
Expected: PASS — all roundtrip tests including `guest_drives_ir_and_returns_typed_stream`.

- [ ] **Step 8: Verify the guest still builds standalone for wasm (CI smoke)**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-wasm-guest --target wasm32-wasip2 --release 2>&1 | tail -15`
Expected: builds clean with an empty baked IR (no `TAU_IR_BYTES`).

- [ ] **Step 9: Commit**

```bash
git add crates/tau-wasm-guest/src/executor.rs crates/tau-wasm-guest/src/host_ports.rs crates/tau-wasm-guest/src/dispatcher.rs crates/tau-wasm-guest/src/lib.rs crates/tau-wasm-guest/src/guest.rs crates/tau-wasm-host/tests/roundtrip.rs crates/tau-wasm-host/Cargo.toml
git commit -m "feat(β.7.5): guest drives run_ir_streaming over baked IR, returns typed RunEvent stream (PR-E2)"
```

---

## Task 4: End-to-end DoD — `tau build wasm` → wasmtime, typed stream

**Files:**
- Create: `crates/tau-cli/tests/build_wasm_e2e.rs`
- Modify: `docs/decisions/0046-wasm-aot-artifact-and-wit-world.md` (status note)

**Interfaces:**
- Consumes: `tau_cli::cmd::build_wasm::lower_to_wasm_ir`; `tau_wasm_host::run_component`; the same `trivial` fixture from Task 1.
- Produces: the PR-E2 DoD assertion — the full CLI lowering path feeds a guest that runs in wasmtime and returns a typed stream.

> This test ties Task 1 (CLI lowering) and Task 3 (guest driving) together through the real artifact path. It is slow (a wasm build); keep it in its own test file so it can be `--test`-targeted.

- [ ] **Step 1: Write the failing e2e test**

Create `crates/tau-cli/tests/build_wasm_e2e.rs`:

```rust
//! β.7.5 PR-E2 DoD: `tau build wasm` of a trivial 1-agent cassette project
//! produces a component that runs in wasmtime and returns a typed RunEvent
//! stream. Requires `wasm32-wasip2` installed.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest with the trivial fixture's IR baked in, via the same
/// lowering the CLI uses, and return the component bytes.
fn build_trivial_component() -> Vec<u8> {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("trivial")).expect("lowers");

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let target_dir = workspace_root.join("target/tau-build-wasm-e2e");

    let ir_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir_file.path(), &bytes).unwrap();

    let output = Command::new(env!("CARGO"))
        .current_dir(&workspace_root)
        .args([
            "build", "-p", "tau-wasm-guest",
            "--target", "wasm32-wasip2", "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TAU_IR_BYTES", ir_file.path())
        .output()
        .expect("cargo spawn");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .flat_map(|m| {
            m["filenames"].as_array().into_iter().flatten()
                .filter_map(|f| f.as_str().map(str::to_string)).collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("a .wasm artifact");
    std::fs::read(wasm_path).unwrap()
}

#[test]
fn build_wasm_then_run_returns_typed_stream() {
    let component = build_trivial_component();
    let response = r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string();
    let out = tau_wasm_host::run_component(&component, "hi", vec![response]).expect("runs");

    let events: Vec<tau_runtime_core::stream::RunEvent> =
        serde_json::from_str(&out).expect("typed stream");
    assert!(matches!(events.last(), Some(tau_runtime_core::stream::RunEvent::RunCompleted { .. })));
}
```

Add `tau-wasm-host`, `tau-runtime-core`, `tempfile` to `crates/tau-cli/Cargo.toml` `[dev-dependencies]` if absent.

- [ ] **Step 2: Run the test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --test build_wasm_e2e 2>&1 | tail -20`
Expected: FAIL initially only if Tasks 1+3 are incomplete; once both land it should compile. (If running tasks in order, this passes immediately after Task 3 — that is acceptable; the value is the regression guard.)

- [ ] **Step 3: Make it pass (no new product code expected)**

If Tasks 1 and 3 are complete, this passes as written. If it fails, debug against the failure (likely a dev-dep or fixture-path issue) — do not weaken the assertion.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --test build_wasm_e2e 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Update ADR-0046 status note**

In `docs/decisions/0046-wasm-aot-artifact-and-wit-world.md`, find the note saying the typed-RunEvent-stream observable is deferred and replace it with a one-line status update:

```markdown
> **Status (PR-E2, 2026-06-20):** the guest now drives `run_ir_streaming`
> over the baked IR and returns the serialized typed `RunEvent` stream; the
> `dev == wasm` conformance arm (`WasmMode`) is flipped live in PR-G.
```

> Do not restructure the ADR; PR-G finalizes 0046/0050. Keep `docs/SUMMARY.md` unchanged (the ADR is already listed).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/tests/build_wasm_e2e.rs crates/tau-cli/Cargo.toml docs/decisions/0046-wasm-aot-artifact-and-wit-world.md
git commit -m "test(β.7.5): e2e tau build wasm -> wasmtime typed stream + ADR-0046 status (PR-E2)"
```

---

## Final verification (run before opening the PR)

- [ ] Guest standalone wasm build: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
- [ ] Host + guest e2e: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host`
- [ ] CLI tests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
- [ ] Conformance unaffected: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance -p tau-ir-conformance`
- [ ] Core unchanged: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
- [ ] clippy on touched crates: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli -p tau-wasm-host` (guest clippy runs under the wasm target in CI)
- [ ] fmt: `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check`

## Out of scope (these are PR-F / PR-G, not PR-E2)

- Native tools (`read_temp`/`set_fan`) in-guest → `tau-native-tools` crate (PR-F).
- Threading the WIT `prompt` into a user `Message` (PR-F).
- Multi-agent / trigger-driven entry selection (PR-F/G).
- In-guest MCP facilitator + cassette, ADR-0050 (PR-F).
- `WasmMode` as a third `ExecutionMode`, the `conformance (wasm)` CI lane, flipping `fan_monitor_dev_matches_wasm` live, byte-equal parity fixture (PR-G).
- β.4 context manager / fixture `13_context_pipeline` (blocked on β.4; PR-G).
