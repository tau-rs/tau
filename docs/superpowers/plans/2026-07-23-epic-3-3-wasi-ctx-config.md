# EPIC 3.3 — Host `WasiCtx` from allow-bounded capabilities — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Configure the wasmtime host `WasiCtx` from a component's allow-bounded capabilities so its filesystem/network authority at runtime matches its declared caps and nothing more.

**Architecture:** A new host-only module in `tau-wasm-host` folds the cap set (derived exactly like E3.2) through E3.1's `map_capability(cap).config` into a `WasiGrants` value (network `HostAccess` + filesystem `PreopenGrant`s). A new `run_component_with_caps` entry builds a `WasiCtx` from those grants (preopens + default-deny network) and a `WasiHttpView` that rejects non-permitted authorities, then links WASI alongside the existing `tau:host/host` stubs. The existing `run_component` becomes a no-caps wrapper so the determinism-conformance path is byte-identical.

**Tech Stack:** Rust, wasmtime 45 (`wasmtime-wasi` 45, `wasmtime-wasi-http` 45), `wit-bindgen` 0.58, `wasm32-wasip2`, `cargo nextest`.

## Global Constraints

- **CARGO (every invocation):** `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host` — always `-p tau-wasm-host`, always `timeout`, never bare `cargo`. Build/check use `timeout 180`; clippy `timeout 240`.
- **Doctests** (the crate examples): `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo test -p tau-wasm-host --doc` (nextest cannot run doctests).
- **wasmtime version is pinned at 45** (RUSTSEC-2026-0222 accepted in `deny.toml`, dev-graph-only). `wasmtime-wasi` and `wasmtime-wasi-http` MUST be `"45"` to match — never a different major.
- **Error discipline:** `thiserror` at the crate boundary (`WasmHostError`), `anyhow` internally. `#![forbid(unsafe_code)]` stays (the crate is host-only std; no `unsafe`).
- **Determinism invariant:** `run_component` (the no-caps path) must stay behaviorally identical — the `WasmProfile` conformance depends on it. Do not alter `determinism_config`, the three `tau:host/host` stubs, or the existing unit tests.
- **Cap source (reuse E3.2 verbatim, do NOT re-derive):**
  `module.workflow.capability_table.0.values().flat_map(|r| r.declared.iter().cloned())` then `tau_domain::canon_caps(&used)`.
- **Branch:** `feat/epic-3-3-wasi-ctx-config`. PR to `main`. Merge queue on; enroll with bare `gh pr merge <N> --squash --auto` (NO `--delete-branch`).
- **Commits:** conventional, imperative, scoped `epic-3-3`; use `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify` (docs/Rust mix; CI is the gate).

---

## File Structure

- **Create `crates/tau-wasm-host/src/wasi.rs`** — the pure, host-only translation: `HostAccess`, `PreopenGrant`, `PreopenAccess` re-use, `WasiGrants`, `wasi_grants_from_caps`, `HostAccess::permits`, glob-prefix helper. Fully unit-tested here. No wasmtime types.
- **Modify `crates/tau-wasm-host/src/lib.rs`** — `mod wasi;` + re-exports; extend `HostState` with WASI fields; impl `IoView`/`WasiView`/`WasiHttpView`; add `run_component_with_caps`; make `run_component` a wrapper.
- **Modify `crates/tau-wasm-host/Cargo.toml`** — add `wasmtime-wasi`, `wasmtime-wasi-http`, `tau-domain` (for `Capability`), and dev-deps as needed.
- **Create `crates/tau-wasm-host/tests/fixtures/fs-probe/Cargo.toml` + `src/lib.rs`** — a standalone (non-workspace) `wasm32-wasip2` component implementing the `runner` world; `run(path)` does `std::fs::read(path)`.
- **Create `crates/tau-wasm-host/tests/wasi_fs_enforcement.rs`** — the runtime negative-enforcement integration test (`#[ignore]`, shells `cargo build`).

---

## Task 1: `HostAccess` network policy (pure)

**Files:**
- Create: `crates/tau-wasm-host/src/wasi.rs`
- Modify: `crates/tau-wasm-host/src/lib.rs` (add `mod wasi;` and `pub use`)
- Test: inline `#[cfg(test)]` in `src/wasi.rs`

**Interfaces:**
- Produces: `pub enum HostAccess { DenyAll, Any, Only(std::collections::BTreeSet<String>) }` with `pub fn permits(&self, authority: &str) -> bool`.

- [ ] **Step 1: Write the failing test**

Create `crates/tau-wasm-host/src/wasi.rs` with only:

```rust
//! Host-only translation of allow-bounded capabilities into a wasmtime
//! `WasiCtx` configuration (EPIC 3.3). Pure: no wasmtime types leak in here.

use std::collections::BTreeSet;

/// Network egress policy folded across all of a component's capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAccess {
    /// No network capability present — deny all egress.
    DenyAll,
    /// Some `net.http` cap authorized `hosts = "any"` — unrestricted egress.
    Any,
    /// Union of exact authorized host authorities (`host` or `host:port`).
    Only(BTreeSet<String>),
}

impl HostAccess {
    /// True iff `authority` (a `host` or `host:port` string) may be reached.
    pub fn permits(&self, authority: &str) -> bool {
        match self {
            HostAccess::DenyAll => false,
            HostAccess::Any => true,
            HostAccess::Only(hosts) => hosts.contains(authority),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_matches_policy() {
        assert!(!HostAccess::DenyAll.permits("a.com"));
        assert!(HostAccess::Any.permits("a.com"));
        let only = HostAccess::Only(["a.com".into(), "b.com:8443".into()].into());
        assert!(only.permits("a.com"));
        assert!(only.permits("b.com:8443"));
        assert!(!only.permits("b.com")); // port is part of the authority
        assert!(!only.permits("c.com"));
    }
}
```

Add to `crates/tau-wasm-host/src/lib.rs` just after the module doc-comment / `use` block (around line 24):

```rust
mod wasi;
pub use wasi::HostAccess;
```

- [ ] **Step 2: Run test to verify it fails, then passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host wasi::tests::permits_matches_policy`
Expected: PASS (this task is pure and self-contained; the "failing" state is only if you typo the logic).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-wasm-host/src/wasi.rs crates/tau-wasm-host/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(epic-3-3): add HostAccess network egress policy"
```

---

## Task 2: `WasiGrants` derivation from caps (pure)

**Files:**
- Modify: `crates/tau-wasm-host/src/wasi.rs`
- Modify: `crates/tau-wasm-host/Cargo.toml` (add `tau-domain` dep)
- Modify: `crates/tau-wasm-host/src/lib.rs` (extend `pub use`)
- Test: inline `#[cfg(test)]` in `src/wasi.rs`

**Interfaces:**
- Consumes: `HostAccess` (Task 1); `tau_ports::target::wasi_map::{map_capability, WasiConfig, Preopen, PreopenAccess}`; `tau_domain::{Capability, package::host::HostSet}`.
- Produces:
  - `pub struct PreopenGrant { pub host_path: std::path::PathBuf, pub guest_path: String, pub access: tau_ports::target::wasi_map::PreopenAccess }`
  - `pub struct WasiGrants { pub hosts: HostAccess, pub preopens: Vec<PreopenGrant> }`
  - `pub fn wasi_grants_from_caps(caps: &[tau_domain::Capability], sandbox_root: &std::path::Path) -> Result<WasiGrants, crate::WasmHostError>`

- [ ] **Step 1: Add the `tau-domain` dependency**

In `crates/tau-wasm-host/Cargo.toml`, under `[dependencies]` (alongside `tau-ports`) — `Capability` appears in the public `wasi_grants_from_caps` / `run_component_with_caps` signatures, so it is a normal dep:

```toml
tau-domain = { workspace = true }
```

And under `[dev-dependencies]` (the cap constructors are gated behind `test-fixtures`, matching `tau-ports`'s dev-dep):

```toml
tau-domain = { workspace = true, features = ["test-fixtures"] }
```

(`tau-ports` is already a dep; `map_capability`, `WasiConfig`, `Preopen`, `PreopenAccess`, `Disposition` come from `tau_ports::target::wasi_map`. `HostSet` comes from `tau_domain::package::host`.)

- [ ] **Step 2: Write the failing tests**

Append to `crates/tau-wasm-host/src/wasi.rs`:

```rust
use std::path::{Path, PathBuf};

use tau_domain::Capability;
use tau_ports::target::wasi_map::{map_capability, Preopen, PreopenAccess, WasiConfig};

/// One filesystem preopen the host will grant the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreopenGrant {
    /// Real host directory to open (sandbox_root joined with the guest dir).
    pub host_path: PathBuf,
    /// Path as the guest sees it (the glob's static prefix directory).
    pub guest_path: String,
    /// Read-only or read-write.
    pub access: PreopenAccess,
}

/// The full WASI grant set derived from a component's allow-bounded caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiGrants {
    pub hosts: HostAccess,
    pub preopens: Vec<PreopenGrant>,
}

/// The longest leading directory prefix of a glob pattern containing no glob
/// metacharacter (`*`, `?`, `[`). Segments up to the first glob segment are
/// kept verbatim; a pattern with no glob metacharacter is returned whole — its
/// named path IS the preopen (tighter than preopening a parent directory, so
/// the guest never gains authority over siblings the capability didn't name).
/// `/data/**` -> `/data`; `/data/*.txt` -> `/data`; `/out` -> `/out`;
/// `/data/logs` -> `/data/logs`; `/a/b/c.txt` -> `/a/b/c.txt`.
///
/// RULING (Option T, tight): a plain trailing segment is NEVER stripped. A cap
/// that names a bare file would then fail-closed (no descriptor) rather than
/// over-granting its parent dir; 3.4's in-guest gate does fine-grained matching.
fn glob_prefix_dir(pattern: &str) -> String {
    let mut dir = String::from("/");
    for seg in pattern.trim_start_matches('/').split('/') {
        if seg.is_empty() || seg.contains(['*', '?', '[']) {
            break;
        }
        if dir.len() > 1 {
            dir.push('/');
        }
        dir.push_str(seg);
    }
    dir
}

/// Fold the caps' [`WasiConfig`]s into a [`WasiGrants`]. Reuses E3.1's
/// [`map_capability`]; hardware / in-guest / host-mediated caps contribute
/// nothing (they carry `WasiConfig::None`).
pub fn wasi_grants_from_caps(
    caps: &[Capability],
    sandbox_root: &Path,
) -> Result<WasiGrants, crate::WasmHostError> {
    use tau_ports::target::wasi_map::Disposition;

    let mut any = false;
    let mut exact: BTreeSet<String> = BTreeSet::new();
    let mut has_net = false;
    // guest_path -> access, RW wins over RO for the same dir.
    let mut preopen_map: std::collections::BTreeMap<String, PreopenAccess> =
        std::collections::BTreeMap::new();

    for cap in caps {
        let mapping = map_capability(cap);
        if let Disposition::Unsupported { reason } = &mapping.disposition {
            return Err(crate::WasmHostError::UnsupportedCap {
                reason: reason.clone(),
            });
        }
        match mapping.config {
            WasiConfig::None => {}
            WasiConfig::AllowedHosts { hosts, .. } => {
                has_net = true;
                if hosts.is_any() {
                    any = true;
                } else {
                    exact.extend(hosts.exact_hosts());
                }
            }
            WasiConfig::Preopens(preopens) => {
                for Preopen { paths, access } in preopens {
                    for pat in paths {
                        let guest_path = glob_prefix_dir(&pat);
                        let entry = preopen_map
                            .entry(guest_path)
                            .or_insert(PreopenAccess::ReadOnly);
                        if access == PreopenAccess::ReadWrite {
                            *entry = PreopenAccess::ReadWrite;
                        }
                    }
                }
            }
        }
    }

    let hosts = if any {
        HostAccess::Any
    } else if has_net {
        HostAccess::Only(exact)
    } else {
        HostAccess::DenyAll
    };

    let preopens = preopen_map
        .into_iter()
        .map(|(guest_path, access)| PreopenGrant {
            host_path: sandbox_root.join(guest_path.trim_start_matches('/')),
            guest_path,
            access,
        })
        .collect();

    Ok(WasiGrants { hosts, preopens })
}

#[cfg(test)]
mod grant_tests {
    use super::*;
    use tau_domain::fixtures::{cap_fs_read, cap_fs_write, cap_net_http};

    #[test]
    fn glob_prefix_rule() {
        assert_eq!(glob_prefix_dir("/data/**"), "/data");
        assert_eq!(glob_prefix_dir("/out"), "/out");
        assert_eq!(glob_prefix_dir("/data/*.txt"), "/data");
        assert_eq!(glob_prefix_dir("/data/logs"), "/data/logs");
        assert_eq!(glob_prefix_dir("/a/b/c.txt"), "/a/b/c.txt");
    }

    #[test]
    fn no_net_cap_is_deny_all() {
        let g = wasi_grants_from_caps(&[cap_fs_read(&["/data/**"])], Path::new("/tmp/root")).unwrap();
        assert_eq!(g.hosts, HostAccess::DenyAll);
    }

    #[test]
    fn exact_hosts_become_only() {
        let g = wasi_grants_from_caps(
            &[cap_net_http(&["a.com", "b.com"], &[])],
            Path::new("/tmp/root"),
        )
        .unwrap();
        assert_eq!(
            g.hosts,
            HostAccess::Only(["a.com".into(), "b.com".into()].into())
        );
    }

    #[test]
    fn fs_read_maps_to_readonly_preopen_under_root() {
        let g = wasi_grants_from_caps(&[cap_fs_read(&["/data/**"])], Path::new("/tmp/root")).unwrap();
        assert_eq!(g.preopens.len(), 1);
        let p = &g.preopens[0];
        assert_eq!(p.guest_path, "/data");
        assert_eq!(p.host_path, PathBuf::from("/tmp/root/data"));
        assert_eq!(p.access, PreopenAccess::ReadOnly);
    }

    #[test]
    fn fs_write_wins_over_read_for_same_dir() {
        // Both caps name the `/data` glob dir; the two preopens merge and RW wins.
        let g = wasi_grants_from_caps(
            &[cap_fs_read(&["/data/**"]), cap_fs_write(&["/data/**"], None)],
            Path::new("/tmp/root"),
        )
        .unwrap();
        assert_eq!(g.preopens.len(), 1);
        assert_eq!(g.preopens[0].guest_path, "/data");
        assert_eq!(g.preopens[0].access, PreopenAccess::ReadWrite);
    }
}
```

> NOTE: fixture signatures are confirmed — `cap_fs_read(&[&str])`, `cap_fs_write(&[&str], Option<u64>)`, `cap_net_http(&[&str] hosts, &[&str] methods)` where `hosts == ["any"]` yields `HostSet::Any`. They live in `tau_domain::fixtures` (module `pub mod fixtures;` in `tau-domain/src/lib.rs:29`), gated behind the `test-fixtures` feature added to the dev-dep in Step 1.

- [ ] **Step 3: Add the `UnsupportedCap` error variant**

In `crates/tau-wasm-host/src/lib.rs`, add to `enum WasmHostError`:

```rust
    /// A capability maps to `Disposition::Unsupported` on wasm (should have
    /// been rejected at `tau build wasm`; belt-and-suspenders at host time).
    #[error("capability unsupported on wasm: {reason}")]
    UnsupportedCap { reason: String },
```

Extend the `pub use` in `lib.rs`:

```rust
pub use wasi::{wasi_grants_from_caps, HostAccess, PreopenGrant, WasiGrants};
```

- [ ] **Step 4: Run the tests**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host wasi::`
Expected: all `wasi::` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-wasm-host/src/wasi.rs crates/tau-wasm-host/src/lib.rs crates/tau-wasm-host/Cargo.toml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(epic-3-3): derive WasiGrants from allow-bounded caps"
```

---

## Task 3: Wire core WASI into the host + `run_component_with_caps`

**Files:**
- Modify: `crates/tau-wasm-host/Cargo.toml` (add `wasmtime-wasi`)
- Modify: `crates/tau-wasm-host/src/lib.rs`
- Test: existing unit tests must stay green; add a wiring smoke unit test.

**Interfaces:**
- Consumes: `WasiGrants`, `wasi_grants_from_caps` (Task 2).
- Produces: `pub fn run_component_with_caps(wasm_bytes: &[u8], prompt: &str, llm_responses: Vec<String>, caps: &[tau_domain::Capability], sandbox_root: &std::path::Path) -> Result<String, WasmHostError>`. `run_component(...)` delegates with `&[]` caps and a throwaway sandbox root.

- [ ] **Step 1: Add the dependency**

In `crates/tau-wasm-host/Cargo.toml` `[dependencies]`:

```toml
wasmtime-wasi = "45"
```

- [ ] **Step 2: Extend `HostState` and implement the WASI view traits**

In `crates/tau-wasm-host/src/lib.rs`:

Add imports near the top:

```rust
use std::path::Path;
use wasmtime_wasi::p2::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi::{DirPerms, FilePerms, IoView, ResourceTable};
```

> API NOTE (wasmtime-wasi 45): the p2 preview2 surface lives under `wasmtime_wasi::p2`. `IoView` (`fn table(&mut self) -> &mut ResourceTable`) and `WasiView` (`fn ctx(&mut self) -> &mut WasiCtx`, or a `WasiCtxView` in some 45.x points) come from these paths. Before writing, confirm with:
> `ls ~/.cargo/registry/src/*/wasmtime-wasi-45*/src/` and `grep -rn "pub trait WasiView\|pub trait IoView\|pub fn add_to_linker_sync" ~/.cargo/registry/src/*/wasmtime-wasi-45*/src/`.
> If `WasiView::ctx` returns `WasiCtxView<'_>`, implement it as `WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }`. The rest of the plan is agnostic to that detail.

Extend `HostState` with WASI fields (keep the existing three):

```rust
struct HostState {
    responses: VecDeque<String>,
    clock_millis: u64,
    prng_state: u64,
    // WASI 0.2 host state (EPIC 3.3).
    table: ResourceTable,
    wasi: WasiCtx,
}
```

Update `HostState::new` to take the built `WasiCtx`:

```rust
fn new(responses: Vec<String>, wasi: WasiCtx) -> Self {
    Self {
        responses: responses.into(),
        clock_millis: 0,
        prng_state: PRNG_SEED,
        table: ResourceTable::new(),
        wasi,
    }
}
```

Implement the view traits (place after the `impl host::Host for HostState` block):

```rust
impl IoView for HostState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}
impl WasiView for HostState {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}
```

- [ ] **Step 3: Build the `WasiCtx` from grants and link WASI**

Add a helper in `lib.rs`:

```rust
/// Build a `WasiCtx` that grants exactly `grants.preopens` and denies all
/// network egress by default (network is gated separately by the wasi-http
/// filter in a later task; raw `wasi:sockets` stays default-deny).
fn wasi_ctx_from_grants(grants: &WasiGrants) -> Result<WasiCtx, WasmHostError> {
    let mut builder = WasiCtxBuilder::new();
    for p in &grants.preopens {
        // Ensure the host dir exists so preopen succeeds; the caller's
        // sandbox_root scopes it.
        std::fs::create_dir_all(&p.host_path)
            .map_err(|e| WasmHostError::WasiConfig(e.into()))?;
        let (dir_perms, file_perms) = match p.access {
            PreopenAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
            PreopenAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
        };
        builder
            .preopened_dir(&p.host_path, &p.guest_path, dir_perms, file_perms)
            .map_err(|e| WasmHostError::WasiConfig(e.into()))?;
    }
    Ok(builder.build())
}
```

> API NOTE: `WasiCtxBuilder::preopened_dir(host_path, guest_path, DirPerms, FilePerms)` returns `Result<&mut Self>` in v45. If the signature differs (e.g. takes `AsRef<Path>` / `&str`), adapt; `p.host_path` is `PathBuf`, `p.guest_path` is `String`. Add `use tau_ports::target::wasi_map::PreopenAccess;`.

Add the `WasiConfig` error variant to `WasmHostError`:

```rust
    /// Building the WASI context (e.g. a preopen dir failed to open).
    #[error("failed to configure WASI context: {0}")]
    WasiConfig(#[source] anyhow::Error),
```

Rewrite `run_component` to delegate, and add `run_component_with_caps`:

```rust
/// EPIC 3.3: run a component whose WASI authority is bounded by `caps`.
/// Filesystem globs resolve to preopens under `sandbox_root`; network egress
/// is denied unless a `net.http` cap authorizes the target host.
pub fn run_component_with_caps(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    caps: &[tau_domain::Capability],
    sandbox_root: &Path,
) -> Result<String, WasmHostError> {
    for resp in &llm_responses {
        serde_json::from_str::<CompletionResponse>(resp).map_err(WasmHostError::InvalidResponse)?;
    }

    let grants = wasi_grants_from_caps(caps, sandbox_root)?;
    let wasi = wasi_ctx_from_grants(&grants)?;

    let config = determinism_config().map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let engine = Engine::new(&config).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let component =
        Component::new(&engine, wasm_bytes).map_err(|e| WasmHostError::Load(e.into()))?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;
    Runner::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    let mut store = Store::new(&engine, HostState::new(llm_responses, wasi));
    let runner = Runner::instantiate(&mut store, &component, &linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    match runner.call_run(&mut store, prompt) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(guest_err)) => Err(WasmHostError::Guest(guest_err)),
        Err(trap) => Err(WasmHostError::Trap(trap.into())),
    }
}

/// Determinism-conformance entry: no capabilities, so no WASI grants.
pub fn run_component(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
) -> Result<String, WasmHostError> {
    run_component_with_caps(wasm_bytes, prompt, llm_responses, &[], Path::new("."))
}
```

> API NOTE: `wasmtime_wasi::p2::add_to_linker_sync` is the WASI 0.2 linker-add in v45. Confirm the exact path with the grep above; older points expose it as `wasmtime_wasi::add_to_linker_sync`. Both existing `run_component` tests (`malformed_cassette_rejected_before_wasm`, `well_formed_cassette_passes_validation_then_fails_at_load`) still pass because validation precedes WASI setup — the empty-bytes load still fails at `Component::new`.

- [ ] **Step 4: Run the existing + build to verify green**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host`
Expected: all existing unit tests PASS (clock/prng/complete/cassette), plus `wasi::` tests. If `well_formed_cassette_passes_validation_then_fails_at_load` now fails at a different variant, adjust the assertion to `matches!(err, WasmHostError::Load(_))` still holds (empty bytes fail at `Component::new`, before WASI matters — grants for empty caps build a no-preopen ctx cleanly).

- [ ] **Step 5: Clippy + commit**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo clippy -p tau-wasm-host --all-targets`
Expected: no warnings (workspace treats warnings as deny in CI).

```bash
git add crates/tau-wasm-host/Cargo.toml crates/tau-wasm-host/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(epic-3-3): build host WasiCtx preopens from caps"
```

---

## Task 4: fs-probe fixture + runtime negative-enforcement test (acceptance)

**Files:**
- Create: `crates/tau-wasm-host/tests/fixtures/fs-probe/Cargo.toml`
- Create: `crates/tau-wasm-host/tests/fixtures/fs-probe/src/lib.rs`
- Create: `crates/tau-wasm-host/tests/wasi_fs_enforcement.rs`

**Interfaces:**
- Consumes: `run_component_with_caps` (Task 3); `tau_domain::fixtures::cap_fs_read`.

- [ ] **Step 1: Author the fs-probe fixture (implements the `runner` world, does a real fs read)**

Create `crates/tau-wasm-host/tests/fixtures/fs-probe/Cargo.toml`:

```toml
[package]
name = "tau-wasm-fs-probe"
version = "0.0.0"
edition = "2021"
publish = false
# Intentionally NOT a workspace member: a std wasm32-wasip2 component built on
# demand by the enforcement test. Empty [workspace] makes cargo treat this as
# its own root so it doesn't attach to the outer workspace.

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = { version = "0.58", default-features = false, features = ["macros", "realloc"] }

[workspace]
```

Create `crates/tau-wasm-host/tests/fixtures/fs-probe/src/lib.rs`:

```rust
//! A minimal WASI 0.2 component that exercises host WasiCtx enforcement:
//! its `run(path)` attempts `std::fs::read(path)` and reports the outcome.
//! Built only for `wasm32-wasip2` by `tests/wasi_fs_enforcement.rs`.

wit_bindgen::generate!({
    world: "runner",
    path: "../../../../../wit",
});

struct Component;

impl Guest for Component {
    fn run(path: String) -> Result<String, String> {
        match std::fs::read(&path) {
            Ok(bytes) => Ok(format!("read {} bytes", bytes.len())),
            Err(e) => Err(format!("denied: {e}")),
        }
    }
}

export!(Component);
```

> NOTE: the `path:` is relative to this fixture's manifest dir; `crates/tau-wasm-host/tests/fixtures/fs-probe/` → workspace `wit/` is five `..`. Verify by counting: `fs-probe → fixtures → tests → tau-wasm-host → crates → <root>`, then `/wit`. If wit-bindgen 0.58 needs `generate_all` or a `with:` for the `tau:host/host` import, add `generate_all` — the guest at `crates/tau-wasm-guest/src/guest.rs:12` is the reference invocation. The `Guest` trait + `export!` names come from the `runner` world.

- [ ] **Step 2: Write the enforcement test (this is the acceptance test — write it before trusting the wiring)**

Create `crates/tau-wasm-host/tests/wasi_fs_enforcement.rs`:

```rust
//! EPIC 3.3 acceptance: a component's filesystem authority at runtime matches
//! its declared caps. A granted path (inside a preopen) reads; an un-granted
//! path (no preopen) is NOT reachable from the guest.
//!
//! `#[ignore]` by default: shells `cargo build --target wasm32-wasip2` for the
//! fs-probe fixture (needs `rustup target add wasm32-wasip2`). Run with:
//!   cargo nextest run -p tau-wasm-host --run-ignored all

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_domain::fixtures::cap_fs_read;
use tau_wasm_host::{run_component_with_caps, WasmHostError};

/// Build the fs-probe fixture for wasm32-wasip2 and return the component bytes.
fn build_fs_probe() -> Vec<u8> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fs-probe/Cargo.toml");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the workspace root")
        .to_path_buf();
    let target_dir = workspace_root.join("target/wasm-fs-probe-fixture");

    let output = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("failed to spawn cargo for fs-probe");

    assert!(
        output.status.success(),
        "fs-probe build failed (is wasm32-wasip2 installed?):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("cargo json is utf-8");
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("no .wasm artifact for fs-probe");

    std::fs::read(&wasm_path).expect("read fs-probe component")
}

#[test]
#[ignore = "builds a wasm32-wasip2 fixture; run in the wasm lane"]
fn granted_path_is_readable_ungranted_path_is_not() {
    let component = build_fs_probe();
    let sandbox = tempfile::tempdir().expect("sandbox tempdir");

    // Grant read on `/data/**`. wasi_grants maps this to a preopen at
    // host `<sandbox>/data` seen by the guest as `/data`.
    let caps = [cap_fs_read(&["/data/**"])];

    // Seed a file inside the granted dir.
    std::fs::create_dir_all(sandbox.path().join("data")).unwrap();
    std::fs::write(sandbox.path().join("data/ok.txt"), b"hello").unwrap();

    // GRANTED: reading /data/ok.txt succeeds.
    let ok = run_component_with_caps(&component, "/data/ok.txt", vec![], &caps, sandbox.path());
    assert!(
        matches!(&ok, Ok(msg) if msg.contains("read 5 bytes")),
        "granted path should read, got: {ok:?}"
    );

    // UN-GRANTED: /etc/secret has no preopen — the guest cannot reach it.
    let denied =
        run_component_with_caps(&component, "/etc/secret", vec![], &caps, sandbox.path());
    match denied {
        Ok(payload) => panic!("un-granted path was reachable: {payload}"),
        // The guest's `run` returned its Err arm ("denied: ...") — WASI gave
        // the guest no descriptor for a non-preopened path.
        Err(WasmHostError::Guest(msg)) => assert!(msg.contains("denied"), "got: {msg}"),
        // A trap is also acceptable evidence of non-reachability.
        Err(WasmHostError::Trap(_)) => {}
        Err(other) => panic!("unexpected host error: {other:?}"),
    }
}
```

- [ ] **Step 3: Run the enforcement test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host --run-ignored all granted_path_is_readable_ungranted_path_is_not`
Expected: PASS. If the granted read fails with "denied", inspect the preopen guest/host path mapping (Task 2 `glob_prefix_dir` + Task 3 `preopened_dir` arg order — host path first, guest path second). If the un-granted read unexpectedly succeeds, the WASI ctx is inheriting ambient fs — ensure no `inherit_stdio`/preopen of `/` and that only `grants.preopens` are added.

- [ ] **Step 4: Confirm the determinism path is untouched**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host --run-ignored all`
Expected: `roundtrip` + `fan_monitor_simple` + `wit_host_drift` still PASS (the guest builds unchanged; `run_component` delegates with empty caps → no preopens, no behavior change).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-wasm-host/tests/fixtures/fs-probe crates/tau-wasm-host/tests/wasi_fs_enforcement.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(epic-3-3): runtime enforcement — un-granted path unreachable"
```

---

## Task 5: Wire wasi-http allowed-hosts filter (network side)

**Files:**
- Modify: `crates/tau-wasm-host/Cargo.toml` (add `wasmtime-wasi-http`)
- Modify: `crates/tau-wasm-host/src/lib.rs`
- Test: inline unit test that the derived `HostAccess` gates authorities (the live wasi:http guest is Option-A-deferred; the filter decision is `HostAccess::permits`, already covered in Task 1).

**Interfaces:**
- Consumes: `HostAccess` (Task 1), `WasiGrants.hosts` (Task 2).
- Produces: `HostState` additionally stores `WasiHttpCtx` + `HostAccess`; impl `WasiHttpView` whose `send_request` rejects authorities failing `permits`; wasi-http linked in `run_component_with_caps`.

- [ ] **Step 1: Add the dependency**

In `crates/tau-wasm-host/Cargo.toml` `[dependencies]`:

```toml
wasmtime-wasi-http = "45"
```

- [ ] **Step 2: Store http ctx + policy on `HostState`**

In `lib.rs`, add fields:

```rust
    // wasi:http egress state + policy (EPIC 3.3).
    http: wasmtime_wasi_http::WasiHttpCtx,
    host_access: HostAccess,
```

Update `HostState::new` to accept and store them (pass `grants.hosts.clone()` and `WasiHttpCtx::new()` from `run_component_with_caps`).

- [ ] **Step 3: Implement `WasiHttpView` with the authority filter**

```rust
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

impl WasiHttpView for HostState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn send_request(
        &mut self,
        request: wasmtime_wasi_http::types::OutgoingRequest,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        let authority = request.authority.clone();
        if !self.host_access.permits(&authority) {
            return Err(wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpRequestDenied.into());
        }
        wasmtime_wasi_http::types::default_send_request(self, request)
    }
}
```

> API NOTE (this is the highest-drift point in the plan — verify before writing): in wasmtime-wasi-http 45 the `send_request` override signature and the outgoing-request type name (`OutgoingRequest` vs a `hyper::Request` + `OutgoingRequestConfig` pair) vary. Confirm with:
> `grep -rn "fn send_request\|pub struct OutgoingRequest\|fn default_send_request\|HttpRequestDenied\|pub trait WasiHttpView" ~/.cargo/registry/src/*/wasmtime-wasi-http-45*/src/`.
> Match the real signature; the load-bearing logic is only: extract the target authority, `if !self.host_access.permits(authority) { return Err(HttpRequestDenied) }`, else delegate to the default. If the default helper needs `&mut self` + the request, pass them through as the grep shows. `WasiHttpView` requires `IoView` (already impl'd in Task 3).

- [ ] **Step 4: Link wasi-http in `run_component_with_caps`**

After the core-WASI linker-add:

```rust
    wasmtime_wasi_http::add_only_http_to_linker_sync(&mut linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;
```

> API NOTE: `add_only_http_to_linker_sync` adds just the wasi:http interfaces (core WASI already added). Confirm the fn name with the grep (`grep -rn "pub fn add_only_http_to_linker_sync\|pub fn add_to_linker_sync" ~/.cargo/registry/src/*/wasmtime-wasi-http-45*/src/`). Since the fs-probe (std, no http) does not import wasi:http, linking it is inert for the Task 4 test — it only becomes live when a guest imports wasi:http.

- [ ] **Step 5: Unit-test the folded policy end of the wire**

Append to `src/wasi.rs` `grant_tests`:

```rust
    #[test]
    fn any_host_cap_yields_any_policy() {
        // `cap_net_http(&["any"], &[])` yields `HostSet::Any` → permit any host.
        let g = wasi_grants_from_caps(
            &[cap_net_http(&["any"], &[])],
            Path::new("/tmp/root"),
        )
        .unwrap();
        assert_eq!(g.hosts, HostAccess::Any);
        assert!(g.hosts.permits("anything.example:443"));
    }
```

(`cap_net_http` is already imported in `grant_tests` from Task 2.)

- [ ] **Step 6: Build, clippy, test, commit**

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo build -p tau-wasm-host --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo clippy -p tau-wasm-host --all-targets
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host
```
Expected: all green; wasi-http compiles and links; `HostAccess::Any` test passes.

```bash
git add crates/tau-wasm-host/Cargo.toml crates/tau-wasm-host/src/lib.rs crates/tau-wasm-host/src/wasi.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(epic-3-3): gate wasi:http egress by allow-bounded hosts"
```

---

## Task 6: cargo-deny + docs + PR

**Files:**
- Possibly modify: `deny.toml` (only if the new deps introduce advisories/licenses)
- Modify: `docs/superpowers/plans/2026-07-23-epic-3-3-wasi-ctx-config.md` (tick boxes as executed)

- [ ] **Step 1: Run cargo-deny to catch new advisories/licenses**

Run: `timeout 180 env CARGO_TARGET_DIR=target/agent-e33 cargo deny check 2>&1 | tail -40` (if `cargo-deny` is installed; otherwise note that CI runs it).
Expected: no NEW denied advisory/license from `wasmtime-wasi`/`wasmtime-wasi-http` (they share the wasmtime 45 tree already RUSTSEC-accepted). If a new transitive advisory appears, add a scoped exception with a comment mirroring the existing wasmtime block in `deny.toml`.

- [ ] **Step 2: Full crate gate**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo test -p tau-wasm-host --doc
```
Expected: all green (ignored wasm tests excluded from the default lane; run them once with `--run-ignored all` as in Task 4).

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin feat/epic-3-3-wasi-ctx-config
gh pr create --base main --title "feat(epic-3-3): configure host WasiCtx from allow-bounded caps" --body "$(cat <<'EOF'
EPIC 3.3 — host side of the EPIC 3 chain (after E3.1 #511 mapping + E3.2 #517 WIT-world gen).

Builds the wasmtime `WasiCtx` from the SAME allow-bounded caps E3.2 reads
(`capability_table` → `canon_caps` → E3.1 `map_capability(cap).config`):
- fs paths → preopens (glob's static prefix dir, mapped under a sandbox root)
- network hosts → `HostAccess` gating `wasi:http` egress (default-deny)
- hardware / in-guest / host-mediated caps → no WASI grant

New `run_component_with_caps`; `run_component` stays a no-caps wrapper so the
`WasmProfile` determinism path is byte-identical.

Acceptance test (`tests/wasi_fs_enforcement.rs`, `#[ignore]`, wasm lane): a
component granted `/data/**` reads `/data/ok.txt` but an un-granted `/etc/secret`
is NOT reachable at runtime.

Scope: host WasiCtx wiring only. In-guest gate (3.4) and WIT generation (3.2)
untouched. A live `wasi:http` negative test awaits a guest that issues http
(Option A per the design spec).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Enroll auto-merge**

```bash
gh pr merge <N> --squash --auto
```
(No `--delete-branch` — conflicts with the merge queue.)

---

## Self-Review (author checklist — completed)

**Spec coverage:** cap-source reuse (Task 2 §Global Constraints), E3.1 mapping reuse (Task 2), fs preopens (Tasks 2–4), network allowed-hosts (Tasks 2,5), hardware skipped (Task 2 fold), glob→dir rule (Task 2), `run_component_with_caps` + wrapper (Task 3), determinism untouched (Tasks 3–4), fs-probe fixture + negative runtime test (Task 4), unit tests for grants/permits (Tasks 1–2,5), wasmtime 45 pin + deny (Global + Task 6). ✅ all mapped.

**Placeholder scan:** no TBD/TODO; every code step has full content. The `> API NOTE` blocks are deliberate verification prompts against the real 45.x source (the crates aren't in the registry cache yet), each with the exact `grep` to run and the load-bearing invariant — not placeholders.

**Type consistency:** `HostAccess`/`permits` (T1) → used T2,T5. `WasiGrants{hosts,preopens}`, `PreopenGrant{host_path,guest_path,access}`, `wasi_grants_from_caps` (T2) → used T3. `run_component_with_caps` signature identical T3↔T4. `WasmHostError::{UnsupportedCap,WasiConfig}` defined before use. `PreopenAccess` sourced from `tau_ports::target::wasi_map` throughout.
