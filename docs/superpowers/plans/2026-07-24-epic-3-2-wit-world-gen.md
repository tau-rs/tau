# EPIC 3.2 — WIT-world generation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** At `tau build wasm`, generate the guest component's WIT `world` from the used-and-`[allow]`-bounded capability set, importing exactly the WASI interfaces those caps require (+ transitive closure + `tau:host`).

**Architecture:** A pure `no_std` generator in `tau-ports::target::wit_world` folds 3.1's `map_capability` over a cap slice, unions the `Disposition::Wasi` imports, expands a hardcoded transitive-dep table, and renders deterministic WIT text (or errors on an `Unsupported` cap). `tau-cli::cmd::build_wasm` runs the reused `[allow]`/GOV000 governance gate, aggregates the lowered IR's used caps, calls the generator, and writes `<out>.wit` next to `<out>.wasm`.

**Tech Stack:** Rust (`no_std` + `alloc` in tau-ports), `thiserror`, `cargo nextest`. Consumes 3.1 (`tau_ports::target::wasi_map`, merged) read-only.

## Global Constraints

- Branch: `feat/epic-3-2-wit-world-gen` (already created off `origin/main`). PR to `main`. Never push to `main`. CI is the gate.
- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p <crate>`. Always `-p`, always `timeout`, prefer `nextest`. Doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo test -p <crate> --doc`.
- `thiserror` at boundaries, `anyhow` internally. `#![forbid(unsafe_code)]`, `deny(missing_docs)` (workspace lints — an undocumented `pub` item or any warning fails CI under `-D warnings`).
- `tau-ports::target::wit_world` is `no_std` + `alloc` (sibling of `wasi_map.rs`).
- Do NOT modify 3.1's `WitInterface` enum. Transitive interfaces live in 3.2's own table as package-id `&'static str`s.
- `WASI_VERSION = "0.2.3"` (from 3.1). All package-ids end `@0.2.3`.
- Construct capabilities in tests via `tau_domain::fixtures::*` (variants are `#[non_exhaustive]`; struct literals are E0639).
- Do NOT pull in 3.3 (host `WasiCtx`/WASI linking), guest-ABI wiring, 3.4 (gate-drop), or 3.5 (byte-compare).

---

### Task 1: Pure WIT-world generator (`tau-ports::target::wit_world`)

**Files:**
- Create: `crates/tau-ports/src/target/wit_world.rs`
- Modify: `crates/tau-ports/src/target/mod.rs:11` (add `pub mod wit_world;`) and `:19-22` (add a `pub use wit_world::{generate_world, WitWorldError};`)
- Test: in-file `#[cfg(test)] mod tests` (matches `wasi_map.rs` convention)

**Interfaces:**
- Consumes (from 3.1, read-only): `tau_ports::target::wasi_map::{map_capability, WasiMapping, Disposition, WitInterface, WASI_VERSION}`; `WitInterface::package_id()`; `WitInterface` derives `Copy + Ord + Hash`.
- Produces (Task 2 relies on these exact names/types):
  - `pub fn generate_world(caps: &[tau_domain::Capability]) -> Result<alloc::string::String, WitWorldError>`
  - `pub enum WitWorldError { UnsupportedOnWasm { cap: String, reason: &'static str } }`

- [ ] **Step 1: Write the module skeleton + failing determinism/empty test**

Create `crates/tau-ports/src/target/wit_world.rs` with the doc-comment, imports, and an empty-cap test that will fail to compile (function not yet defined):

```rust
//! WIT-world generation for the wasm target (EPIC 3.2).
//!
//! [`generate_world`] folds a capability set through the 3.1
//! [`map_capability`](super::wasi_map::map_capability) table, unions the
//! `Disposition::Wasi` WIT imports, expands their hardcoded transitive
//! closure, and renders a deterministic WIT `world`. The world is the frozen
//! `tau:host` `runner` world's superset with the cap-derived WASI imports
//! added. An `Unsupported` capability (fs.exec, process.spawn) is a hard error.
//!
//! Output is a deterministic ABI *manifest*: without vendored WASI `.wit`
//! packages it is not standalone-resolvable WIT (that is 3.3+). Determinism is
//! the contract 3.5's `verify --bundle` byte-compare relies on.
//!
//! See `docs/superpowers/specs/2026-07-24-epic-3-2-wit-world-gen-design.md`.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;

use tau_domain::Capability;

use super::wasi_map::{map_capability, Disposition, WitInterface};

/// Error raised when a capability cannot be realized on the wasm target.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WitWorldError {
    /// A capability maps to `Disposition::Unsupported` on wasm (fs.exec,
    /// process.spawn) — it has no WASI ABI realization.
    #[error("capability `{cap}` cannot target wasm: {reason}")]
    UnsupportedOnWasm {
        /// Debug rendering of the offending capability.
        cap: String,
        /// The reason carried by `Disposition::Unsupported`.
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures::{cap_agent_spawn, cap_fs_read, cap_net_http, cap_process_spawn};

    #[test]
    fn empty_cap_set_yields_host_only_world() {
        let world = generate_world(&[]).expect("empty is ok");
        assert_eq!(
            world,
            "package tau:generated@0.1.0;\n\
             \n\
             world runner {\n\
             \x20   import host;\n\
             \n\
             \x20   export run: func(prompt: string) -> result<string, string>;\n\
             }\n"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-ports wit_world`
Expected: FAIL to compile — `generate_world` not found (and `wit_world` not yet declared in `mod.rs`).

- [ ] **Step 3: Wire the module + implement `generate_world` and the closure table**

In `crates/tau-ports/src/target/mod.rs` add after `pub mod wasi_map;`:

```rust
pub mod wit_world;
```

and add to the existing re-export block:

```rust
pub use wit_world::{generate_world, WitWorldError};
```

Append the implementation to `wit_world.rs` (above the `#[cfg(test)]` module):

```rust
/// Transitive WASI interfaces (as fully-qualified package-ids at
/// `WASI_VERSION`) that a direct [`WitInterface`] pulls in. These interfaces
/// are NOT in 3.1's `WitInterface` enum — they are the closure 3.2 owns.
///
/// Edges (WASI 0.2.3): `http/types` → io/{streams,poll,error} +
/// clocks/monotonic-clock; `filesystem/types` → io/{streams,poll,error} +
/// clocks/wall-clock; `io/streams` → io/{error,poll};
/// `clocks/monotonic-clock` → io/poll (all folded into the sets below).
fn transitive_closure(iface: WitInterface) -> &'static [&'static str] {
    match iface {
        WitInterface::WasiHttpTypes | WitInterface::WasiHttpOutgoingHandler => &[
            "wasi:io/streams@0.2.3",
            "wasi:io/poll@0.2.3",
            "wasi:io/error@0.2.3",
            "wasi:clocks/monotonic-clock@0.2.3",
        ],
        WitInterface::WasiFilesystemTypes | WitInterface::WasiFilesystemPreopens => &[
            "wasi:io/streams@0.2.3",
            "wasi:io/poll@0.2.3",
            "wasi:io/error@0.2.3",
            "wasi:clocks/wall-clock@0.2.3",
        ],
        // `WitInterface` is `#[non_exhaustive]`; a future interface contributes
        // no closure until this table is extended for it.
        _ => &[],
    }
}

/// Generate the guest component's WIT `world` from a capability set.
///
/// Folds each capability through [`map_capability`], keeps the
/// `Disposition::Wasi` imports, unions them, expands the transitive closure,
/// and renders a deterministic `world runner` importing `tau:host` + the
/// resulting WASI interfaces and exporting `run`. `InGuest`/`HostMediated`
/// capabilities contribute no import; an `Unsupported` capability is a hard
/// error ([`WitWorldError::UnsupportedOnWasm`]).
///
/// # Example
///
/// ```
/// use tau_ports::target::wit_world::generate_world;
/// use tau_domain::fixtures::cap_fs_read;
///
/// let wit = generate_world(&[cap_fs_read(&["/d"])]).unwrap();
/// assert!(wit.contains("import wasi:filesystem/types@0.2.3;"));
/// assert!(wit.contains("export run: func(prompt: string)"));
/// ```
pub fn generate_world(caps: &[Capability]) -> Result<String, WitWorldError> {
    // 1. Union the direct WASI interfaces the granted caps require.
    let mut ifaces: BTreeSet<WitInterface> = BTreeSet::new();
    for cap in caps {
        let mapping = map_capability(cap);
        match mapping.disposition {
            Disposition::Wasi => ifaces.extend(mapping.imports),
            Disposition::Unsupported { reason } => {
                return Err(WitWorldError::UnsupportedOnWasm {
                    cap: format!("{cap:?}"),
                    reason,
                });
            }
            // No WASI surface — contributes nothing to the world.
            Disposition::InGuest | Disposition::HostMediated => {}
            // `Disposition` is `#[non_exhaustive]`: a future disposition is
            // conservatively treated as contributing no import.
            _ => {}
        }
    }

    // 2. Expand to fully-qualified package-ids (direct + transitive closure),
    //    deduped and sorted via BTreeSet → deterministic output.
    let mut imports: BTreeSet<&'static str> = BTreeSet::new();
    for iface in &ifaces {
        imports.insert(iface.package_id());
        for id in transitive_closure(*iface) {
            imports.insert(id);
        }
    }

    // 3. Render. `import host;` mirrors the frozen `wit/tau-host.wit` style.
    let mut out = String::new();
    out.push_str("package tau:generated@0.1.0;\n\nworld runner {\n    import host;\n");
    for id in &imports {
        out.push_str("    import ");
        out.push_str(id);
        out.push_str(";\n");
    }
    out.push_str("\n    export run: func(prompt: string) -> result<string, string>;\n}\n");
    Ok(out)
}
```

- [ ] **Step 4: Run the empty-world test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-ports wit_world`
Expected: PASS (`empty_cap_set_yields_host_only_world`).

- [ ] **Step 5: Add the remaining behavior tests**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn net_only_imports_http_plus_transitive() {
        let world = generate_world(&[cap_net_http(&["api.anthropic.com"], &["POST"])]).unwrap();
        for want in [
            "import host;",
            "import wasi:http/types@0.2.3;",
            "import wasi:http/outgoing-handler@0.2.3;",
            "import wasi:io/streams@0.2.3;",
            "import wasi:io/poll@0.2.3;",
            "import wasi:io/error@0.2.3;",
            "import wasi:clocks/monotonic-clock@0.2.3;",
        ] {
            assert!(world.contains(want), "missing `{want}` in:\n{world}");
        }
        // fs / wall-clock interfaces must NOT appear for a net-only cap set.
        assert!(!world.contains("wasi:filesystem"), "net-only leaked fs:\n{world}");
        assert!(!world.contains("wall-clock"), "net-only leaked wall-clock:\n{world}");
    }

    #[test]
    fn fs_only_imports_filesystem_plus_transitive() {
        let world = generate_world(&[cap_fs_read(&["/data/**"])]).unwrap();
        for want in [
            "import wasi:filesystem/types@0.2.3;",
            "import wasi:filesystem/preopens@0.2.3;",
            "import wasi:io/streams@0.2.3;",
            "import wasi:clocks/wall-clock@0.2.3;",
        ] {
            assert!(world.contains(want), "missing `{want}` in:\n{world}");
        }
        assert!(!world.contains("wasi:http"), "fs-only leaked http:\n{world}");
        assert!(!world.contains("monotonic-clock"), "fs-only leaked monotonic:\n{world}");
    }

    #[test]
    fn mixed_unions_and_dedupes() {
        let caps = [cap_net_http(&["h"], &[]), cap_fs_read(&["/d"])];
        let world = generate_world(&caps).unwrap();
        assert!(world.contains("wasi:http/types@0.2.3;"));
        assert!(world.contains("wasi:filesystem/types@0.2.3;"));
        // io/streams is shared by both families — appears exactly once.
        assert_eq!(world.matches("import wasi:io/streams@0.2.3;").count(), 1);
    }

    #[test]
    fn in_guest_caps_contribute_no_import() {
        let world = generate_world(&[cap_agent_spawn(&["worker"])]).unwrap();
        assert!(!world.contains("wasi:"), "in-guest cap leaked a wasi import:\n{world}");
        assert!(world.contains("import host;"));
    }

    #[test]
    fn unsupported_cap_is_a_hard_error() {
        let err = generate_world(&[cap_process_spawn(&["ls"])]).unwrap_err();
        match err {
            WitWorldError::UnsupportedOnWasm { reason, .. } => assert!(!reason.is_empty()),
        }
    }

    #[test]
    fn output_is_deterministic_regardless_of_cap_order() {
        let a = [cap_net_http(&["h"], &[]), cap_fs_read(&["/d"])];
        let b = [cap_fs_read(&["/d"]), cap_net_http(&["h"], &[])];
        assert_eq!(generate_world(&a).unwrap(), generate_world(&b).unwrap());
    }

    #[test]
    fn every_transitive_id_is_version_pinned() {
        for iface in [
            WitInterface::WasiHttpTypes,
            WitInterface::WasiFilesystemTypes,
        ] {
            for id in transitive_closure(iface) {
                assert!(id.starts_with("wasi:"), "not qualified: {id}");
                assert!(id.ends_with("@0.2.3"), "version drift: {id}");
            }
        }
    }
```

- [ ] **Step 6: Run the full module test suite + doctest**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-ports wit_world`
Then: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo test -p tau-ports --doc wit_world`
Expected: all PASS.

- [ ] **Step 7: Clippy the crate (workspace `-D warnings`)**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo clippy -p tau-ports --all-targets`
Expected: clean (no `missing_docs`, no dead-code, no warnings).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ports/src/target/wit_world.rs crates/tau-ports/src/target/mod.rs
git commit -m "feat(tau-ports): WIT-world generator from capability set (EPIC 3.2 Task 1)"
```

---

### Task 2: Wire generation + `[allow]` gate into `tau build wasm`

**Files:**
- Modify: `crates/tau-cli/src/cli.rs:232-242` (add governance flags to `BuildWasmArgs`)
- Modify: `crates/tau-cli/src/cmd/build_wasm.rs` (governance gate, cap aggregation, world generation, `<out>.wit` write)
- Create: `crates/tau-cli/tests/fixtures/wasm-build/net-http/tau.toml` (net.http fixture; plus any minimal files a project needs — mirror the existing `trivial` fixture)
- Test: `crates/tau-cli/tests/cmd_build_wasm.rs` (append; does NOT shell cargo — same discipline as the existing file)

**Interfaces:**
- Consumes: `tau_ports::target::wit_world::{generate_world, WitWorldError}` (Task 1); `tau_domain::canon_caps`; `crate::cmd::check::{evaluate_governance, render_no_constitution, render_violations, CheckCtx, GovernanceFlags, GovernanceOutcome}`; `tau_ir::IrModule` with `module.workflow.capability_table.0: BTreeMap<ToolId, CapabilityRequirements { declared: Vec<Capability> }>`.
- Produces:
  - `pub fn wasm_world_for_project(project: &std::path::Path) -> anyhow::Result<String>` — lower + aggregate caps + `generate_world`. (Test seam; no governance, no cargo.)
  - `pub async fn wasm_governance_gate(project_path: &std::path::Path, flags: GovernanceFlags) -> Result<(), String>` — `Ok(())` to proceed, `Err(diagnostic)` to refuse. (Test seam; no `process::exit`.)

- [ ] **Step 1: Add governance flags to `BuildWasmArgs`**

In `crates/tau-cli/src/cli.rs`, extend `BuildWasmArgs` (after the `output` field) to mirror `BuildArgs`:

```rust
    /// Authorize a wasm build of a project that declares NO `[allow]` ceiling.
    /// Governed-by-default: without this, a missing `[allow]` is a hard error
    /// (GOV000). Distinct from `--no-governance` (skip an existing ceiling).
    #[arg(long, conflicts_with = "no_governance")]
    pub allow_ungoverned: bool,
    /// Build a project that HAS an `[allow]` ceiling without enforcing it.
    #[arg(long)]
    pub no_governance: bool,
```

- [ ] **Step 2: Write the failing world-content test**

Append to `crates/tau-cli/tests/cmd_build_wasm.rs`:

```rust
use tau_cli::cmd::build_wasm::wasm_world_for_project;

#[test]
fn trivial_project_generates_host_only_world() {
    let world = wasm_world_for_project(&fixture("trivial")).expect("trivial world");
    assert!(world.contains("import host;"));
    assert!(!world.contains("wasi:"), "trivial should grant no wasi surface:\n{world}");
}

#[test]
fn net_http_project_generates_http_world() {
    let world = wasm_world_for_project(&fixture("net-http")).expect("net-http world");
    assert!(world.contains("import wasi:http/outgoing-handler@0.2.3;"), "{world}");
    assert!(world.contains("import wasi:io/streams@0.2.3;"), "{world}");
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-cli cmd_build_wasm`
Expected: FAIL to compile — `wasm_world_for_project` not found.

- [ ] **Step 4: Implement `wasm_world_for_project` (cap aggregation + generation)**

In `crates/tau-cli/src/cmd/build_wasm.rs`, add imports and the function. Near the top:

```rust
use tau_ports::target::wit_world::generate_world;
```

Add after `lower_to_wasm_ir`:

```rust
/// Aggregate the lowered IR's used capabilities and generate the guest's WIT
/// world. Separated from `run` so it is testable without shelling the wasm
/// build. Governance is enforced separately by [`wasm_governance_gate`].
///
/// The used caps come from every tool's `declared` set in the IR capability
/// table; they are canonicalized so cap order and duplicates never affect the
/// world. After the governance gate proceeds these caps are provably within
/// `[allow]` (the gate enforces tool ⊆ agent-effective ⊆ root ceiling), so the
/// generated world is the `[allow]`-bounded set — no redundant `meet`.
pub fn wasm_world_for_project(project: &Path) -> Result<String> {
    let (module, _bytes) = lower_to_wasm_ir(project)?;
    let used: Vec<tau_domain::Capability> = module
        .workflow
        .capability_table
        .0
        .values()
        .flat_map(|req| req.declared.iter().cloned())
        .collect();
    let caps = tau_domain::canon_caps(&used);
    generate_world(&caps).map_err(|e| anyhow::anyhow!("wasm WIT-world generation failed: {e}"))
}
```

- [ ] **Step 5: Run the world-content tests to verify they pass**

Create the `net-http` fixture first — mirror `crates/tau-cli/tests/fixtures/wasm-build/trivial/` but give one tool a `net.http` capability to `api.anthropic.com`. (Inspect the `trivial` fixture for the exact minimal `tau.toml` shape; copy it and add the capability.)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-cli cmd_build_wasm`
Expected: PASS (`trivial_project_generates_host_only_world`, `net_http_project_generates_http_world`).

- [ ] **Step 6: Write the failing governance-gate test**

Append to `crates/tau-cli/tests/cmd_build_wasm.rs`:

```rust
use tau_cli::cmd::build_wasm::wasm_governance_gate;
use tau_cli::cmd::check::GovernanceFlags;

#[tokio::test]
async fn ungoverned_project_is_refused_on_wasm_path() {
    // `trivial` declares no `[allow]` ceiling → GOV000 unless opted out.
    let err = wasm_governance_gate(&fixture("trivial"), GovernanceFlags::default())
        .await
        .expect_err("ungoverned must be refused");
    assert!(err.contains("GOV000"), "expected GOV000, got: {err}");
}

#[tokio::test]
async fn allow_ungoverned_flag_lets_it_proceed() {
    let flags = GovernanceFlags { allow_ungoverned: true, no_governance: false };
    wasm_governance_gate(&fixture("trivial"), flags)
        .await
        .expect("--allow-ungoverned proceeds");
}
```

(If `fixture("trivial")` already declares an `[allow]` block, add a new `ungoverned` fixture without one and point these tests at it.)

- [ ] **Step 7: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-cli cmd_build_wasm`
Expected: FAIL to compile — `wasm_governance_gate` not found.

- [ ] **Step 8: Implement `wasm_governance_gate`**

In `crates/tau-cli/src/cmd/build_wasm.rs`, add the async gate (mirrors `build.rs::evaluate_build_governance`, but returns the diagnostic instead of `process::exit` so it is unit-testable):

```rust
use crate::cmd::check::{
    evaluate_governance, render_no_constitution, render_violations, CheckCtx, GovernanceFlags,
    GovernanceOutcome,
};

/// Governed-by-default gate for the wasm build path (ADR-0057 / D2), reusing
/// the `tau check governance` engine. Returns `Ok(())` to proceed or
/// `Err(diagnostic)` — the caller prints the diagnostic and exits 2. `tau build
/// wasm` produces no bundle, so the `GovernanceVerdict` is not stamped.
pub async fn wasm_governance_gate(
    project_path: &Path,
    flags: GovernanceFlags,
) -> std::result::Result<(), String> {
    let target: tau_ports::target::TargetTriple = WASM_TARGET
        .parse()
        .expect("any-wasi-strict is a registered triple");
    let ctx = CheckCtx::load(project_path.to_path_buf(), false, Some(target))
        .await
        .map_err(|e| format!("cannot evaluate governance: {e}"))?;
    let Some(project) = &ctx.project else {
        // Unparseable project — let the lowering path surface the precise error.
        return Ok(());
    };
    match evaluate_governance(project, &ctx, flags) {
        GovernanceOutcome::Proceed(_) => Ok(()),
        GovernanceOutcome::NoConstitution => Err(render_no_constitution()),
        GovernanceOutcome::Violations(findings) => Err(render_violations(&findings)),
    }
}
```

Confirm `crate::cmd::check` re-exports these six symbols (it does for `build.rs`). If any is only `pub(crate)` under a different path, adjust the `use` to the path `build.rs` uses.

- [ ] **Step 9: Run the gate tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-cli cmd_build_wasm`
Expected: PASS (both governance tests + the two world-content tests).

- [ ] **Step 10: Call both seams from `run()`**

In `crates/tau-cli/src/cmd/build_wasm.rs::run`, before `lower_to_wasm_ir`, add the gate; after the `.wasm` is written, generate + write the `.wit`. Edit `run` so it reads:

```rust
pub async fn run(args: &BuildWasmArgs, output: &mut Output) -> Result<()> {
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd is readable"));

    // Governed-by-default gate (ADR-0057 / D2) — refuse an ungoverned or
    // over-reaching project before doing any build work.
    let flags = GovernanceFlags {
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    if let Err(diag) = wasm_governance_gate(&project, flags).await {
        let _ = output.diagnostic(diag);
        std::process::exit(2);
    }

    let (module, bytes) = lower_to_wasm_ir(&project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));

    // Bake the IR bytes into a tempfile the guest build reads via TAU_IR_BYTES.
    let ir_file = tempfile::NamedTempFile::new().context("creating IR scratch file")?;
    std::fs::write(ir_file.path(), &bytes).context("writing IR scratch bytes")?;

    let wasm = build_guest_with_ir(ir_file.path())?;
    drop(ir_file);

    let out_path = args
        .output
        .clone()
        .unwrap_or_else(|| project.join(format!("{}.wasm", project_stem(&project))));
    std::fs::write(&out_path, &wasm).with_context(|| format!("writing {}", out_path.display()))?;

    // Generate + write the cap-derived WIT world next to the component.
    let world = wasm_world_for_project(&project)?;
    let wit_path = out_path.with_extension("wit");
    std::fs::write(&wit_path, &world)
        .with_context(|| format!("writing {}", wit_path.display()))?;

    let _ = output.human(&format!(
        "built {} ({} bytes, ir {}) + {}",
        out_path.display(),
        wasm.len(),
        ir_hash,
        wit_path.display()
    ));
    Ok(())
}
```

`output.diagnostic` is the same method `build.rs` uses; `GovernanceFlags` derives `Default`. Note `wasm_world_for_project` re-lowers — acceptable (lowering is cheap vs. the cargo wasm build); a future refactor can thread the already-lowered `module` through, but do NOT do that here (keeps the test seam simple).

- [ ] **Step 11: Full crate test + clippy**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo nextest run -p tau-cli cmd_build_wasm`
Then: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e32 cargo clippy -p tau-cli --all-targets`
Expected: tests PASS; clippy clean.

- [ ] **Step 12: Commit**

```bash
git add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/build_wasm.rs \
        crates/tau-cli/tests/cmd_build_wasm.rs \
        crates/tau-cli/tests/fixtures/wasm-build/net-http
git commit -m "feat(tau-cli): generate WIT world + enforce [allow] on tau build wasm (EPIC 3.2 Task 2)"
```

---

## Self-Review

**Spec coverage:**
- Fold `map_capability`, keep `Wasi`, union imports → Task 1 Step 3. ✅
- Transitive closure via hardcoded table → Task 1 Step 3 (`transitive_closure`) + drift test Step 5. ✅
- Emit world: `import host` + WASI imports + `export run`, deterministic/sorted → Task 1 Step 3 + determinism test. ✅
- Unsupported → build error → Task 1 (`WitWorldError`) + Task 2 propagation via `wasm_world_for_project`. ✅
- `[allow]`/GOV000 on wasm path (Approach B) → Task 2 `wasm_governance_gate` + `run` wiring. ✅
- World from IR used-caps (no `meet`) → Task 2 `wasm_world_for_project` + doc rationale. ✅
- Write `<out>.wit` artifact → Task 2 Step 10. ✅
- Test matrix (net/fs/mixed/empty/unsupported/determinism/drift; ungoverned/over-reach*/happy) → Tasks 1 & 2. *Over-reach (`used ⊄ ceiling`) is enforced by the reused `evaluate_governance` engine (already unit-tested at its own layer); the wasm path exercises the same `Violations` arm as `tau build`. If a wasm-specific over-reach fixture is cheap to add, add one asserting `wasm_governance_gate` returns `Err` containing the ceiling-violation text.
- Out-of-scope (3.3/3.4/3.5, guest-ABI wiring) → untouched. ✅

**Placeholder scan:** No TBD/TODO; every code step has full code. The `net-http` fixture (Task 2 Step 5) references the existing `trivial` fixture for its exact shape rather than inlining a guessed `tau.toml` — the implementer inspects one real file; this is a deliberate "match the existing pattern" instruction, not a placeholder.

**Type consistency:** `generate_world(&[Capability]) -> Result<String, WitWorldError>` and `WitWorldError::UnsupportedOnWasm { cap, reason }` are identical across Tasks 1 and 2. `wasm_world_for_project`/`wasm_governance_gate` signatures match between their definitions (Task 2 Steps 4/8) and call sites (tests Steps 2/6, `run` Step 10). `module.workflow.capability_table.0` matches the verified `CapabilityTable(pub BTreeMap<…>)` shape.
