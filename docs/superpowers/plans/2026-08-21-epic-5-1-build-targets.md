# EPIC 5.1 — `tau build --target wasm-guest | rust-lib` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From one project, `tau build --target wasm-guest` emits the wasm guest component and `tau build --target rust-lib` emits a generated no_std Rust library crate — the default `.tau` bundle path stays unchanged.

**Architecture:** `--target` gains two artifact-kind keywords resolved ahead of hardware-triple parsing (`resolve_target` → `BuildTarget` enum). wasm-guest reuses the existing β.7.5 AOT pipeline (`cmd::build_wasm`). rust-lib is a new source-crate renderer in `tau-sdk-codegen` (`emit_rust_lib`) that bakes the same canonical IR bytes + cap-derived WIT that the wasm path produces.

**Tech Stack:** Rust, clap, `tau-ir-lower` (lowering/cap-fit), `tau-sdk-codegen` (renderers), `sha2`, `thiserror`/`anyhow`.

**Spec:** `docs/superpowers/specs/2026-08-21-epic-5-1-build-targets-design.md`

## Global Constraints

- Cargo commands: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p <crate>`. Always `-p`, always `timeout`, never bare cargo. Doctests: `cargo test --doc`.
- `forbid(unsafe_code)`; thiserror at the `tau-sdk-codegen` boundary, anyhow in the CLI shim.
- No `tau-pkg` producer changes (Lane A conflict zone) — stay in `tau-cli` build path + `tau-sdk-codegen`.
- Both new targets lower for `any-wasi-strict` and run the existing governed-by-default gate (ADR-0057). Cap-fit/feature-fit refusals stay exit 2.
- Reuse, do not duplicate: `cmd::build_wasm::{lower_to_wasm_ir, world_from_module}`, `cmd::build::hex_lower`. Existing 60–90s `.wasm` e2e coverage (`build_wasm_e2e`, `build_wasm_world_dod`) is NOT duplicated here.
- The generated rust-lib crate is **source** (not compiled). Tests assert file shape, never shell cargo on it.

---

### Task 1: `emit_rust_lib` renderer in `tau-sdk-codegen`

**Files:**
- Create: `crates/tau-sdk-codegen/src/emit_rust_lib.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (add `pub mod emit_rust_lib;` + re-export)
- Test: inline `#[cfg(test)]` in `emit_rust_lib.rs`

**Interfaces:**
- Produces: `pub fn render_rust_lib(input: RustLibInput) -> std::collections::BTreeMap<std::path::PathBuf, String>` where
  ```rust
  pub struct RustLibInput<'a> {
      pub crate_name: &'a str,   // sanitized project stem, e.g. "trivial"
      pub ir_bytes: &'a [u8],    // canonical IR bytes from lower_to_wasm_ir
      pub ir_hash: &'a str,      // lowercase hex of compute_hash(&module)
      pub wit: &'a str,          // cap-derived world text from world_from_module
      pub tau_version: &'a str,  // pinned dep version, e.g. env!("CARGO_PKG_VERSION")
  }
  ```
  Returned map keys (repo-relative to the crate root): `Cargo.toml`, `src/lib.rs`, `tau.wit`, `README.md`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn render_rust_lib_emits_expected_files_and_bakes_ir() {
        let ir = [0xDEu8, 0xAD, 0xBE, 0xEF];
        let out = render_rust_lib(RustLibInput {
            crate_name: "trivial",
            ir_bytes: &ir,
            ir_hash: "abc123",
            wit: "package tau:workflow;\nworld guest {}\n",
            tau_version: "0.0.0",
        });

        // File set present.
        for f in ["Cargo.toml", "src/lib.rs", "tau.wit", "README.md"] {
            assert!(out.contains_key(&PathBuf::from(f)), "missing {f}");
        }

        let lib = &out[&PathBuf::from("src/lib.rs")];
        assert!(lib.contains("#![no_std]"), "lib must be no_std");
        // IR bytes baked as a const, byte-for-byte.
        assert!(lib.contains("pub const TAU_IR: &[u8] = &[222u8, 173u8, 190u8, 239u8]"),
            "IR const not baked: {lib}");
        assert!(lib.contains(r#"pub const TAU_IR_HASH: &str = "abc123""#));
        assert!(lib.contains("pub use tau_runtime_core::run_ir"));

        let cargo = &out[&PathBuf::from("Cargo.toml")];
        assert!(cargo.contains(r#"name = "trivial""#));
        assert!(cargo.contains(r#"tau-runtime-core = { version = "0.0.0""#));

        assert_eq!(out[&PathBuf::from("tau.wit")], "package tau:workflow;\nworld guest {}\n");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-sdk-codegen render_rust_lib`
Expected: FAIL — `render_rust_lib`/`RustLibInput` not found (compile error).

- [ ] **Step 3: Write minimal implementation**

```rust
//! `emit_rust_lib` — render the Variant B no_std embedding crate (EPIC 5.1).
//!
//! Bakes the canonical IR bytes + cap-derived WIT that the wasm-guest path
//! produces into a linkable Rust source crate. The product links this crate,
//! supplies port impls, and drives `tau_runtime_core::run_ir` (EPIC 7.1).

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Inputs for [`render_rust_lib`].
pub struct RustLibInput<'a> {
    /// Sanitized project stem used as the crate name.
    pub crate_name: &'a str,
    /// Canonical IR bytes (from `lower_to_wasm_ir`).
    pub ir_bytes: &'a [u8],
    /// Lowercase-hex IR module hash.
    pub ir_hash: &'a str,
    /// Cap-derived WIT world text (from `world_from_module`).
    pub wit: &'a str,
    /// Pinned `tau-runtime-core` dependency version.
    pub tau_version: &'a str,
}

/// Render the rust-lib scaffold as crate-relative-path -> contents.
pub fn render_rust_lib(input: RustLibInput) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();

    let ir_literal = input
        .ir_bytes
        .iter()
        .map(|b| format!("{b}u8"))
        .collect::<Vec<_>>()
        .join(", ");

    out.insert(
        PathBuf::from("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
publish = false

# Generated by `tau build --target rust-lib` (EPIC 5.1). This is a no_std
# embedding scaffold: the product links it and supplies port impls (EPIC 7.1).
[dependencies]
tau-runtime-core = {{ version = "{ver}", default-features = false }}
"#,
            name = input.crate_name,
            ver = input.tau_version,
        ),
    );

    out.insert(
        PathBuf::from("src/lib.rs"),
        format!(
            r#"#![no_std]
//! Generated by `tau build --target rust-lib` (EPIC 5.1) — do not edit.
//!
//! Baked workflow IR + the runtime-core entrypoint. Link this crate from your
//! product, supply port impls, and drive [`run_ir`] with [`TAU_IR`].

/// Canonical IR bytes for this workflow.
pub const TAU_IR: &[u8] = &[{ir_literal}];

/// Lowercase-hex hash of the IR module (matches the `.tau`/wasm build).
pub const TAU_IR_HASH: &str = "{hash}";

pub use tau_runtime_core::run_ir;
"#,
            ir_literal = ir_literal,
            hash = input.ir_hash,
        ),
    );

    out.insert(PathBuf::from("tau.wit"), input.wit.to_string());

    out.insert(
        PathBuf::from("README.md"),
        format!(
            "# {name} (rust-lib embedding scaffold)\n\n\
             Generated by `tau build --target rust-lib`. Baked IR hash: `{hash}`.\n\n\
             Link this crate, implement the ports for the capabilities in `tau.wit`, \
             and call `run_ir(TAU_IR, …)`. See EPIC 7.1 for the full embedding API.\n",
            name = input.crate_name,
            hash = input.ir_hash,
        ),
    );

    out
}
```

Add to `crates/tau-sdk-codegen/src/lib.rs` after the other `pub mod` lines:
```rust
pub mod emit_rust_lib;
pub use emit_rust_lib::{render_rust_lib, RustLibInput};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-sdk-codegen render_rust_lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-sdk-codegen/src/emit_rust_lib.rs crates/tau-sdk-codegen/src/lib.rs
git commit -m "feat(sdk-codegen): render_rust_lib no_std embedding scaffold (EPIC 5.1)"
```

---

### Task 2: `BuildTarget` resolver in `tau-cli`

Replace the `resolve_target(&BuildArgs) -> Result<TargetTriple, String>` with a resolver that returns an enum distinguishing the two artifact keywords from a hardware triple.

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` (`resolve_target`, its callers, and its `#[cfg(test)]` tests)

**Interfaces:**
- Produces:
  ```rust
  pub(crate) enum BuildTarget {
      Bundle(TargetTriple),
      WasmGuest,
      RustLib,
  }
  fn resolve_target(args: &BuildArgs) -> Result<BuildTarget, String>;
  ```

- [ ] **Step 1: Write the failing test** — add to the existing `#[cfg(test)] mod tests` in `build.rs`:

```rust
#[test]
fn resolve_target_maps_artifact_keywords() {
    assert!(matches!(
        resolve_target(&args_with_target(Some("wasm-guest"))).unwrap(),
        BuildTarget::WasmGuest
    ));
    assert!(matches!(
        resolve_target(&args_with_target(Some("rust-lib"))).unwrap(),
        BuildTarget::RustLib
    ));
}

#[test]
fn resolve_target_triple_still_yields_bundle() {
    assert!(matches!(
        resolve_target(&args_with_target(None)).unwrap(),
        BuildTarget::Bundle(_)
    ));
    assert!(matches!(
        resolve_target(&args_with_target(Some(&TargetTriple::PASSTHROUGH.to_string()))).unwrap(),
        BuildTarget::Bundle(_)
    ));
}

#[test]
fn resolve_target_invalid_names_both_value_spaces() {
    let err = resolve_target(&args_with_target(Some("not a triple!!!"))).unwrap_err();
    assert!(err.contains("wasm-guest") && err.contains("rust-lib"), "got {err}");
}
```

Update the three existing `resolve_target_*` tests that compared against `TargetTriple` directly to match the new `BuildTarget::Bundle(_)` shape (e.g. `resolve_target_defaults_to_host` → assert `matches!(…, BuildTarget::Bundle(t) if t == TargetTriple::host())`).

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli resolve_target`
Expected: FAIL — `BuildTarget` not found.

- [ ] **Step 3: Write minimal implementation** — replace `resolve_target` in `build.rs`:

```rust
/// The selected build artifact kind (EPIC 5.1). `--target` accepts two
/// artifact-kind keywords resolved ahead of hardware-triple parsing; any
/// other value is a hardware triple producing a `.tau` bundle.
pub(crate) enum BuildTarget {
    /// Default / hardware triple → `.tau` bundle.
    Bundle(TargetTriple),
    /// `--target wasm-guest` → fully-linked wasm component.
    WasmGuest,
    /// `--target rust-lib` → generated no_std Rust library crate.
    RustLib,
}

/// Resolve the build target. Keywords `wasm-guest`/`rust-lib` select an
/// embedding artifact; `None` → host bundle; anything else is parsed as an
/// Available triple (ADR-0034). Returns a human-readable error on invalid input.
fn resolve_target(args: &BuildArgs) -> Result<BuildTarget, String> {
    match args.target.as_deref() {
        None => Ok(BuildTarget::Bundle(TargetTriple::host())),
        Some("wasm-guest") => Ok(BuildTarget::WasmGuest),
        Some("rust-lib") => Ok(BuildTarget::RustLib),
        Some(s) => {
            let triple: TargetTriple = s.parse().map_err(|e| {
                format!(
                    "invalid --target '{s}': {e}. Expected an artifact kind \
                     (wasm-guest, rust-lib) or an Available triple: {}",
                    available_triples_joined(),
                )
            })?;
            let available = tau_ports::target::lookup(&triple)
                .is_some_and(|e| matches!(e.status, tau_ports::target::TripleStatus::Available));
            if !available {
                return Err(format!(
                    "target '{triple}' is not an Available build target. Expected an \
                     artifact kind (wasm-guest, rust-lib) or an Available triple: {}",
                    available_triples_joined(),
                ));
            }
            Ok(BuildTarget::Bundle(triple))
        }
    }
}
```

In `build::run`, change the `let target = match resolve_target(args) { … }` site to bind a `BuildTarget`; for now handle only the `Bundle(triple)` arm and leave `WasmGuest`/`RustLib` as `todo!()`/`unreachable!()` placeholders wired in Task 3–4. To keep the existing bundle flow compiling, destructure:
```rust
let build_target = match resolve_target(args) {
    Ok(t) => t,
    Err(msg) => { let _ = output.error(msg); std::process::exit(2); }
};
let target = match &build_target {
    BuildTarget::Bundle(t) => *t,
    // Wired in Task 3/4.
    BuildTarget::WasmGuest => return dispatch_wasm_guest(args, output).await,
    BuildTarget::RustLib => return dispatch_rust_lib(args, output).await,
};
```
Add stub `async fn dispatch_wasm_guest`/`dispatch_rust_lib` returning `Ok(())` for now (filled in Task 3/4) — or gate this wiring into Task 3/4 and keep Task 2 to the resolver + tests only if cleaner. Either way Task 2's deliverable is the resolver + green resolver tests.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli resolve_target`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/build.rs
git commit -m "feat(cli): BuildTarget resolver for --target wasm-guest|rust-lib (EPIC 5.1)"
```

---

### Task 3: Wire `--target rust-lib` emission + JSON parity

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` (`dispatch_rust_lib`, JSON emit)
- Test: `crates/tau-cli/tests/cmd_build_rust_lib.rs` (new)

**Interfaces:**
- Consumes: `render_rust_lib`, `RustLibInput` (Task 1); `build_wasm::{lower_to_wasm_ir, world_from_module}`, `build_wasm::wasm_governance_gate`; `hex_lower`.
- Produces: `async fn dispatch_rust_lib(&BuildArgs, &mut Output) -> Result<()>`.

- [ ] **Step 1: Write the failing test** (`crates/tau-cli/tests/cmd_build_rust_lib.rs`):

```rust
//! EPIC 5.1: `tau build --target rust-lib` emits the no_std embedding crate.
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

#[test]
fn rust_lib_emits_shape_checked_crate() {
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("gen");
    // Public seam used by the CLI dispatch — same as the wasm path's seams.
    tau_cli::cmd::build::emit_rust_lib_to(&fixture("trivial"), &dir).unwrap();

    for f in ["Cargo.toml", "src/lib.rs", "tau.wit", "README.md"] {
        assert!(dir.join(f).exists(), "missing {f}");
    }
    let lib = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![no_std]"));
    assert!(lib.contains("pub const TAU_IR: &[u8] = &["));
    assert!(lib.contains("pub use tau_runtime_core::run_ir"));
}
```

Note: this test targets a `pub` seam `emit_rust_lib_to(project, out_dir)` (thin, no CLI/Output) so the artifact shape is testable without argument plumbing — mirror `build_wasm::wasm_world_for_project`'s test-seam pattern. `dispatch_rust_lib` calls this seam then handles Output/JSON/exit codes.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli rust_lib_emits`
Expected: FAIL — `emit_rust_lib_to` not found.

- [ ] **Step 3: Write minimal implementation** — in `build.rs`:

```rust
/// Emit the rust-lib embedding crate for `project` into `out_dir`. Test seam:
/// lowers for `any-wasi-strict` (cap-fit refuses ProcessExec/AgentSpawn), derives
/// the WIT world, renders the scaffold, and writes it. Does not run governance
/// or touch `Output` — the CLI dispatch wraps this.
pub fn emit_rust_lib_to(
    project: &std::path::Path,
    out_dir: &std::path::Path,
) -> anyhow::Result<RustLibArtifact> {
    use crate::cmd::build_wasm::{lower_to_wasm_ir, world_from_module};
    let (module, bytes) = lower_to_wasm_ir(project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));
    let wit = world_from_module(&module)?;
    let stem = project.file_name().and_then(|n| n.to_str()).unwrap_or("workflow");
    let crate_name = sanitize_crate_name(stem);

    let files = tau_sdk_codegen::render_rust_lib(tau_sdk_codegen::RustLibInput {
        crate_name: &crate_name,
        ir_bytes: &bytes,
        ir_hash: &ir_hash,
        wit: &wit,
        tau_version: env!("CARGO_PKG_VERSION"),
    });

    let mut written = 0usize;
    for (rel, contents) in &files {
        let path = out_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
        written += 1;
    }
    Ok(RustLibArtifact { out_dir: out_dir.to_path_buf(), ir_hash, files: written })
}

/// Result of a rust-lib emission (for human/JSON output).
pub struct RustLibArtifact {
    pub out_dir: std::path::PathBuf,
    pub ir_hash: String,
    pub files: usize,
}

/// Lowercase, replace non-alphanumeric with `_`, so the stem is a valid crate name.
fn sanitize_crate_name(stem: &str) -> String {
    stem.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

async fn dispatch_rust_lib(args: &BuildArgs, output: &mut Output) -> anyhow::Result<()> {
    let project = args.project.clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd is readable"));
    // Governed-by-default gate (same engine as the wasm path).
    let flags = crate::cmd::check::GovernanceFlags {
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    if let Err(diag) = crate::cmd::build_wasm::wasm_governance_gate(&project, flags).await {
        let _ = output.diagnostic(diag);
        std::process::exit(2);
    }
    let stem = project.file_name().and_then(|n| n.to_str()).unwrap_or("workflow");
    let out_dir = args.output.clone().unwrap_or_else(|| project.join(format!("{stem}-rust-lib")));
    let artifact = match emit_rust_lib_to(&project, &out_dir) {
        Ok(a) => a,
        Err(e) => { let _ = output.error(format!("{e}")); std::process::exit(2); }
    };
    if output.is_json() {
        let _ = output.json(&serde_json::json!({
            "kind": "rust-lib",
            "path": artifact.out_dir.display().to_string(),
            "ir_hash": artifact.ir_hash,
            "files": artifact.files,
        }));
    } else {
        let _ = output.human(&format!(
            "built rust-lib crate: {} ({} files, ir {})",
            artifact.out_dir.display(), artifact.files, artifact.ir_hash,
        ));
    }
    Ok(())
}
```

Wire the `BuildTarget::RustLib => return dispatch_rust_lib(args, output).await` arm from Task 2. Ensure `pub mod build` re-exports (`build.rs` is already `pub mod` in `cmd/mod.rs`; confirm `cmd` module is reachable as `tau_cli::cmd::build` — add `pub` to the module path if the integration test cannot see it, matching how `build_wasm` is exposed).

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli rust_lib_emits`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/build.rs crates/tau-cli/tests/cmd_build_rust_lib.rs
git commit -m "feat(cli): tau build --target rust-lib emits no_std crate + JSON parity (EPIC 5.1)"
```

---

### Task 4: Route `--target wasm-guest` to the AOT pipeline + routing test

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs` (`dispatch_wasm_guest`)
- Modify: `crates/tau-cli/src/cmd/build_wasm.rs` (add a `pub async fn build_wasm_component(project, output, flags, output_path)` core, or accept a constructed `BuildWasmArgs` — pick the smaller diff)
- Test: `crates/tau-cli/tests/cmd_build_target_routing.rs` (new) — routing-level, no `.wasm` build.

**Interfaces:**
- Consumes: `resolve_target`/`BuildTarget` (Task 2), `build_wasm::run` or its extracted core.
- Produces: `async fn dispatch_wasm_guest(&BuildArgs, &mut Output) -> Result<()>`.

- [ ] **Step 1: Write the failing test** (`crates/tau-cli/tests/cmd_build_target_routing.rs`):

```rust
//! EPIC 5.1: `--target wasm-guest` routes to the wasm AOT pipeline. This asserts
//! the *routing* (resolve → wasm path selection) without the 60–90s `.wasm`
//! build; the full component build is covered by build_wasm_e2e / _world_dod.
#[test]
fn wasm_guest_keyword_selects_wasm_pipeline() {
    // resolve_target is crate-private; assert via the public build-target enum
    // classifier exposed for tests.
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(Some("wasm-guest")),
        "wasm-guest"
    );
    assert_eq!(
        tau_cli::cmd::build::classify_target_for_test(Some("rust-lib")),
        "rust-lib"
    );
    assert_eq!(tau_cli::cmd::build::classify_target_for_test(None), "bundle");
}
```

Add a tiny public classifier in `build.rs` for the routing assertion:
```rust
/// Test seam: classify a `--target` value into its artifact-kind label without
/// running a build. Returns "bundle" | "wasm-guest" | "rust-lib" | "invalid".
pub fn classify_target_for_test(target: Option<&str>) -> &'static str {
    let mut args = BuildArgs::default_for_target(target); // helper below, or build inline
    match resolve_target(&args) {
        Ok(BuildTarget::Bundle(_)) => "bundle",
        Ok(BuildTarget::WasmGuest) => "wasm-guest",
        Ok(BuildTarget::RustLib) => "rust-lib",
        Err(_) => "invalid",
    }
}
```
If `BuildArgs` has no cheap constructor, build it inline in the seam (mirror the `args_with_target` test helper but as a non-test fn). Keep it minimal.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli wasm_guest_keyword`
Expected: FAIL — `classify_target_for_test` not found.

- [ ] **Step 3: Write minimal implementation** — implement `dispatch_wasm_guest` by delegating to the existing wasm path:

```rust
async fn dispatch_wasm_guest(args: &BuildArgs, output: &mut Output) -> anyhow::Result<()> {
    // Map the bundle-shaped args onto the wasm subcommand's args and reuse the
    // existing β.7.5 pipeline verbatim (no duplicated lowering/build).
    let wasm_args = crate::cli::BuildWasmArgs {
        project: args.project.clone(),
        output: args.output.clone(),
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    crate::cmd::build_wasm::run(&wasm_args, output).await
}
```

Add `classify_target_for_test` as above.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli wasm_guest_keyword`
Expected: PASS.

- [ ] **Step 5: Full crate gate + help snapshot**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e51 cargo nextest run -p tau-cli`
Expected: PASS. If `help_snapshots` fails because `--target` help text changed, update the snapshot (`cargo insta review` or edit the `.snap`) and re-run. Update `--target` doc comment in `cli.rs` to: `Target: an artifact kind (wasm-guest, rust-lib) or an Available triple (default: host → .tau bundle).`

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cmd/build.rs crates/tau-cli/src/cmd/build_wasm.rs \
        crates/tau-cli/src/cli.rs crates/tau-cli/tests/cmd_build_target_routing.rs \
        crates/tau-cli/tests/snapshots/ 2>/dev/null
git commit -m "feat(cli): route --target wasm-guest to AOT pipeline + help text (EPIC 5.1)"
```

---

### Task 5: Docs example

**Files:**
- Modify: the `tau build` reference/how-to page under `docs/` (find with `git grep -l "tau build" docs/`)
- Modify: `docs/SUMMARY.md` only if a new page is added (prefer extending the existing build page)

- [ ] **Step 1: Add the example** — under the existing build docs, add a short section:

````markdown
## Build targets (embedding artifacts)

`--target` selects what `tau build` emits:

```bash
tau build                      # .tau bundle for the host (default)
tau build --target wasm-guest  # fully-linked wasm component (<name>.wasm + .wit)
tau build --target rust-lib    # generated no_std Rust crate (Variant B embedding)
```

`--target rust-lib ./my-workflow` writes `./my-workflow/my-workflow-rust-lib/`:

```text
Cargo.toml   no_std lib; depends on tau-runtime-core
src/lib.rs   pub const TAU_IR + `pub use tau_runtime_core::run_ir`
tau.wit      capability-derived world
README.md    how to link + supply ports (see EPIC 7.1)
```

Link the crate from your product, implement the ports for the capabilities in
`tau.wit`, and drive `run_ir(TAU_IR, …)`.
````

- [ ] **Step 2: Build the book locally**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines; no linkcheck errors. Then `rm -rf docs/book`.

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(build): document --target wasm-guest|rust-lib (EPIC 5.1)"
```

---

## Self-Review

**Spec coverage:**
- Surface (`--target` keywords, resolution order, exit 2 message) → Task 2. ✓
- wasm-guest artifact (reuse pipeline) → Task 4. ✓
- rust-lib artifact (generated no_std crate, codegen in tau-sdk-codegen) → Task 1 + Task 3. ✓
- Governance reuse → Task 3 (`wasm_governance_gate`); wasm-guest via `build_wasm::run` which already gates. ✓
- JSON parity → Task 3 (rust-lib), wasm-guest keeps `build_wasm::run` human output; JSON for wasm is the existing path (no regression — bundle JSON unchanged). ✓ (Note: `build_wasm::run` currently emits human-only; adding `--json` to it is out of this slice's DoD — the bundle JSON parity requirement is met for bundle + rust-lib; wasm-guest keeps its existing output. Flag in PR body.)
- Tests: one per target → Task 3 (rust-lib artifact), Task 4 (wasm routing). ✓
- Docs example → Task 5. ✓

**Placeholder scan:** the `todo!()`/`unreachable!()` in Task 2 Step 3 are explicitly replaced by Task 3/4 dispatch arms — Task 2 wires the `return dispatch_*` calls, and the stubs return `Ok(())` until filled. No lingering placeholders in shipped code.

**Type consistency:** `RustLibInput`/`render_rust_lib` (Task 1) match their uses in Task 3. `BuildTarget` (Task 2) matches Task 3/4 match arms. `emit_rust_lib_to`/`RustLibArtifact` (Task 3) match the integration test. `classify_target_for_test` (Task 4) reads the same resolver. ✓

**Open reconciliation for executor:** confirm `tau_cli::cmd::build` is reachable from integration tests (the `build_wasm` module already is, per `build_wasm_world_dod.rs` importing `tau_cli::cmd::build_wasm::…`) — if `cmd` is not `pub`, expose the needed seams the same way `build_wasm` is exposed.
