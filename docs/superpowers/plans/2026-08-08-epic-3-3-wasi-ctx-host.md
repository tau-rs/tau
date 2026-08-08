# EPIC 3.3 — Host `WasiCtx` from allow-bounded caps — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the wasmtime `WasiCtx` + `wasi:http` egress gate in `tau-wasm-host` from #533's `WasiConfiguration`, so a component grants exactly its declared hosts/preopens and nothing more.

**Architecture:** `tau-wasm-host` consumes the already-folded `WasiConfiguration` (`allowed_hosts: HostSet`, `methods`, `preopens: Vec<ResolvedPreopen>`) — no re-derivation. A pure `wasi.rs` module exposes the egress gate (`HttpHostGate`) and the preopen set (`preopen_dirs`), which are the exact objects the wasmtime linker installs. `lib.rs` wires them via `WasiView`/`WasiHttpView`/`WasiHttpHooks`. A new `run_component_with_wasi` entry takes the config; the existing `run_component` becomes a deny-all wrapper (zero churn to callers).

**Tech Stack:** Rust, wasmtime 47, wasmtime-wasi 47, wasmtime-wasi-http 47, hyper 1.

## Global Constraints

- **Cargo discipline (verbatim):** every cargo command is `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p <crate>` (build/check use `timeout 180`, `cargo build`/`cargo check`). Never bare cargo, always `-p`, always `timeout`. Doctests: `cargo test --doc -p <crate>`.
- **wasmtime family pinned to 47** — matches `Cargo.lock` (wasmtime 47.0.3). `wasmtime-wasi` and `wasmtime-wasi-http` must be `"47"`. No workspace-wide version change.
- **`thiserror` at the crate boundary, `anyhow` internally. `#![forbid(unsafe_code)]`** (workspace lint; do not add `unsafe`).
- **TDD, deny test first** — the "un-granted host/path unreachable" assertion is written and shown red before implementation.
- **Consume #533, never re-derive** — no local cap-folding; `WasiConfiguration` comes from `tau_ports::target::resolve_wasi_config`.
- **Branch:** `feat/epic-3-3-wasi-ctx-host`. PR to `main`. Merge queue ON — enroll with bare `gh pr merge <N> --squash --auto` (NO `--delete-branch`).
- **Out of scope (do not touch):** in-guest gate (3.4), WIT-world gen (3.2), `verify --bundle` (3.5), and wiring a production `tau run`/`tau build wasm` caller.

---

### Task 1: `WasiConfiguration::deny_all()` in tau-ports

The wasm host's no-caps entry needs a fail-closed config identical to folding an empty cap set. Add it as a named constructor on the type (cleaner than making `tau-wasm-host` name `tau_domain::Capability` just to fold `empty()`).

**Files:**
- Modify: `crates/tau-ports/src/target/wasi_map.rs` (add `impl WasiConfiguration` block + test near the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `WasiConfiguration { allowed_hosts: HostSet, methods: Option<BTreeSet<HttpMethod>>, preopens: Vec<ResolvedPreopen> }` (existing), `resolve_wasi_config` (existing).
- Produces: `WasiConfiguration::deny_all() -> WasiConfiguration`.

- [ ] **Step 1: Write the failing test.** Add to the existing test module in `wasi_map.rs` (the module that already has `use tau_domain::{HostName, HostSet, HttpMethod};` and `use tau_domain::Capability;` — reuse them; add `Capability` import if absent):

```rust
#[test]
fn deny_all_equals_empty_fold() {
    // The no-caps host config must be byte-identical to folding zero caps:
    // deny-all egress, no method grants, no preopens.
    let folded = resolve_wasi_config(core::iter::empty::<&Capability>());
    assert_eq!(WasiConfiguration::deny_all(), folded);
    assert_eq!(WasiConfiguration::deny_all().allowed_hosts, HostSet::Exact(Default::default()));
    assert!(WasiConfiguration::deny_all().methods.is_none());
    assert!(WasiConfiguration::deny_all().preopens.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports deny_all_equals_empty_fold`
Expected: FAIL — `no function or associated item named 'deny_all'`.

- [ ] **Step 3: Write minimal implementation.** Add an `impl WasiConfiguration` block adjacent to the struct definition in `wasi_map.rs`. Ensure `HostSet` and `std::collections::BTreeSet` are in scope at module level (the struct fields already reference them; if `HostSet` is only imported in tests, add `use tau_domain::HostSet;` to the module's top `use` block):

```rust
impl WasiConfiguration {
    /// The fail-closed WASI configuration: deny-all egress (`HostSet::Exact(∅)`),
    /// no method grants, no preopens. Identical to `resolve_wasi_config([])`.
    /// The wasm host's no-caps entry (`run_component`) uses this so a component
    /// with no declared capabilities receives no WASI authority.
    pub fn deny_all() -> Self {
        Self {
            allowed_hosts: HostSet::Exact(BTreeSet::new()),
            methods: None,
            preopens: Vec::new(),
        }
    }
}
```

(If `BTreeSet` is not already imported at module top — it is, since `methods: Option<BTreeSet<HttpMethod>>` — add `use std::collections::BTreeSet;`.)

- [ ] **Step 4: Run test to verify it passes.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports deny_all_equals_empty_fold`
Expected: PASS.

- [ ] **Step 5: Run the full tau-ports suite (no regressions) and commit.**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports
git add crates/tau-ports/src/target/wasi_map.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-3): add WasiConfiguration::deny_all fail-closed constructor"
```

---

### Task 2: Pure `wasi.rs` — egress gate + preopen set (Option A enforcement tests)

The pure, wasmtime-free core: the exact `HttpHostGate` and preopen list the linker will install. This is where the "un-granted host/path unreachable" assertions live — testing the *same* objects the live path consults, no re-derivation, no wasm build.

**Files:**
- Create: `crates/tau-wasm-host/src/wasi.rs`
- Modify: `crates/tau-wasm-host/src/lib.rs` (add `mod wasi;` + `pub use wasi::{HttpHostGate, preopen_dirs};` near the top, after the existing `use` lines)
- Modify: `crates/tau-wasm-host/Cargo.toml` (add `tau-domain` normal dep + dev-dep with `test-fixtures`)

**Interfaces:**
- Consumes: `tau_ports::target::{WasiConfiguration, PreopenAccess}`, `tau_domain::{HostSet, HttpMethod}`, and (tests) `tau_domain::fixtures::{cap_fs_write, cap_net_http}`, `tau_ports::target::resolve_wasi_config`.
- Produces:
  - `struct HttpHostGate` with `pub fn new(cfg: &WasiConfiguration) -> HttpHostGate` and `pub fn allows(&self, authority: &str, method: &HttpMethod) -> bool`.
  - `pub fn preopen_dirs(cfg: &WasiConfiguration) -> Vec<(&str, PreopenAccess)>`.

- [ ] **Step 1: Add the dependency lines to `crates/tau-wasm-host/Cargo.toml`.** Under `[dependencies]` add (tau-ports already present):

```toml
tau-domain    = { workspace = true }
```

Under `[dev-dependencies]` add:

```toml
tau-domain       = { workspace = true, features = ["test-fixtures"] }
```

- [ ] **Step 2: Write the failing test FIRST — create `crates/tau-wasm-host/src/wasi.rs` with only the test module and a stub.** Write the deny test at the top so it is the first thing that runs red:

```rust
//! Host-only translation of an allow-bounded `WasiConfiguration` (EPIC 3.3,
//! from tau-ports' `resolve_wasi_config`) into the egress gate and preopen set
//! the wasmtime embedder installs. Pure: no wasmtime types appear here, so the
//! enforcement decisions are unit-testable in isolation — yet these are the
//! exact objects `lib.rs` hands to the linker.

use std::collections::BTreeSet;

use tau_domain::{HostSet, HttpMethod};
use tau_ports::target::{PreopenAccess, WasiConfiguration};

/// The network egress gate. `lib.rs`'s `WasiHttpHooks::send_request` consults
/// it before wasmtime opens any socket for a `wasi:http` outgoing request.
#[derive(Debug, Clone)]
pub struct HttpHostGate {
    allowed: HostSet,
    methods: Option<BTreeSet<HttpMethod>>,
}

impl HttpHostGate {
    /// Build from the folded config.
    pub fn new(cfg: &WasiConfiguration) -> Self {
        Self {
            allowed: cfg.allowed_hosts.clone(),
            methods: cfg.methods.clone(),
        }
    }

    /// True iff a `wasi:http` request to `authority` (`host` or `host:port`)
    /// with `method` is authorized. `HostSet::Exact(∅)` (deny-all) rejects
    /// every host; `HostSet::Any` permits every host; `methods == None`
    /// permits every method, else only members of the set.
    pub fn allows(&self, authority: &str, method: &HttpMethod) -> bool {
        let host_ok =
            self.allowed.is_any() || self.allowed.exact_hosts().iter().any(|h| h == authority);
        let method_ok = match &self.methods {
            None => true,
            Some(set) => set.contains(method),
        };
        host_ok && method_ok
    }
}

/// The exact preopen set the embedder grants: `(host_dir, access)` per
/// `ResolvedPreopen`, identity-mapped (guest path == host_dir; #533's
/// `host_dir` is already absolute and glob-resolved). `lib.rs`'s
/// `build_wasi_ctx` consumes this same list.
pub fn preopen_dirs(cfg: &WasiConfiguration) -> Vec<(&str, PreopenAccess)> {
    cfg.preopens
        .iter()
        .map(|p| (p.host_dir.as_str(), p.access.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures::{cap_fs_write, cap_net_http};
    use tau_ports::target::resolve_wasi_config;

    fn gate(caps: &[tau_domain::Capability]) -> HttpHostGate {
        HttpHostGate::new(&resolve_wasi_config(caps))
    }

    // THE enforcement test — written first. An authority the caps did not
    // grant is not permitted; the granted one is.
    #[test]
    fn ungranted_host_denied() {
        let g = gate(&[cap_net_http(&["api.example.com"], &[])]);
        assert!(g.allows("api.example.com", &HttpMethod::Get), "granted host permitted");
        assert!(!g.allows("evil.example.com", &HttpMethod::Get), "un-granted host denied");
    }

    #[test]
    fn deny_all_denies_every_host() {
        let g = HttpHostGate::new(&WasiConfiguration::deny_all());
        assert!(!g.allows("api.example.com", &HttpMethod::Get));
        assert!(!g.allows("anything", &HttpMethod::Post));
    }

    #[test]
    fn any_host_permits_all() {
        let g = gate(&[cap_net_http(&["any"], &[])]);
        assert!(g.allows("whatever.example:443", &HttpMethod::Get));
    }

    #[test]
    fn method_outside_set_denied() {
        // net.http restricted to GET on api.example.com.
        let g = gate(&[cap_net_http(&["api.example.com"], &["GET"])]);
        assert!(g.allows("api.example.com", &HttpMethod::Get));
        assert!(!g.allows("api.example.com", &HttpMethod::Post), "un-granted method denied");
    }

    #[test]
    fn preopens_exactly_granted() {
        // fs.write "/work/**" resolves to a single RW preopen at /work.
        let cfg = resolve_wasi_config(&[cap_fs_write(&["/work/**"], None)]);
        assert_eq!(preopen_dirs(&cfg), vec![("/work", PreopenAccess::ReadWrite)]);
    }

    #[test]
    fn deny_all_config_grants_nothing() {
        let cfg = WasiConfiguration::deny_all();
        assert!(preopen_dirs(&cfg).is_empty(), "no preopens");
        assert!(!HttpHostGate::new(&cfg).allows("h", &HttpMethod::Get), "no egress");
    }
}
```

Then add to `crates/tau-wasm-host/src/lib.rs`, immediately after the existing `use` block (before the `bindgen!` macro):

```rust
mod wasi;
pub use wasi::{preopen_dirs, HttpHostGate};
```

- [ ] **Step 3: Run the tests to verify they compile and the enforcement tests pass.** (The module is pure — no wasmtime deps needed yet.)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host wasi::tests`
Expected: all six PASS. (If `deny_all` is missing, Task 1 was skipped — do Task 1 first.)

- [ ] **Step 4: Run the full crate suite to confirm no regressions from the new module/deps.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host`
Expected: PASS (existing determinism/cassette tests + the six new ones).

- [ ] **Step 5: Commit.**

```bash
git add crates/tau-wasm-host/src/wasi.rs crates/tau-wasm-host/src/lib.rs crates/tau-wasm-host/Cargo.toml Cargo.lock
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-3): pure egress gate + preopen set from WasiConfiguration"
```

---

### Task 3: Wire `WasiCtx` + `wasi:http` gate into the embedder

Grow `HostState` with a WASI context, resource table, http context, and the gate; implement the wasmtime views; add the egress hook; build the `WasiCtx` from preopens; add the capped entry point and turn `run_component` into a deny-all wrapper.

**Files:**
- Modify: `crates/tau-wasm-host/Cargo.toml` (add `wasmtime-wasi`, `wasmtime-wasi-http`, `hyper`)
- Modify: `crates/tau-wasm-host/src/lib.rs`

**Interfaces:**
- Consumes: `wasi::{HttpHostGate, preopen_dirs}` (Task 2), `tau_ports::target::{WasiConfiguration, PreopenAccess}`, `tau_domain::HttpMethod`, the wasmtime-wasi 47 API confirmed below.
- Produces:
  - `pub fn run_component_with_wasi(wasm_bytes: &[u8], prompt: &str, llm_responses: Vec<String>, wasi: &WasiConfiguration) -> Result<String, WasmHostError>`
  - `pub fn run_component(...)` unchanged signature, now delegating with `WasiConfiguration::deny_all()`.
  - `WasmHostError::WasiConfig(#[source] anyhow::Error)` variant.

**Verified wasmtime-wasi 47.0.3 API (do not guess — these are confirmed present):**
- `wasmtime_wasi::{DirPerms, FilePerms, ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView}` (crate root).
- `wasmtime_wasi::p2::add_to_linker_sync(&mut Linker<T>) where T: WasiView`.
- `WasiView::ctx(&mut self) -> WasiCtxView<'_>`, and `WasiCtxView { ctx: &mut WasiCtx, table: &mut ResourceTable }`.
- `wasmtime_wasi_http::WasiHttpCtx`; `wasmtime_wasi_http::p2::{add_only_http_to_linker_sync, default_send_request, HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView}`.
- `WasiHttpCtxView<'a> { ctx: &'a mut WasiHttpCtx, table: &'a mut ResourceTable, hooks: &'a mut dyn WasiHttpHooks }`.
- `WasiHttpHooks::send_request(&mut self, request: hyper::Request<HyperOutgoingBody>, config: OutgoingRequestConfig) -> HttpResult<HostFutureIncomingResponse>` (default impl gated behind the `default-send-request` feature, which is ON by default).
- `wasmtime_wasi_http::p2::body::HyperOutgoingBody`; `wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig}`; `wasmtime_wasi_http::p2::bindings::http::types::ErrorCode` (has `HttpRequestDenied`).

- [ ] **Step 1: Add deps to `crates/tau-wasm-host/Cargo.toml`.** Under `[dependencies]` (mirror the reference; hyper is only to name the request type):

```toml
wasmtime-wasi      = "47"
wasmtime-wasi-http = "47"
# Only to name the `hyper::Request<HyperOutgoingBody>` type in the egress hook.
hyper              = { version = "1", default-features = false }
```

- [ ] **Step 2: Write the failing wiring smoke test.** Add to the existing `#[cfg(test)] mod tests` in `lib.rs`:

```rust
#[test]
fn run_component_with_wasi_no_grants_matches_run_component() {
    // A deny-all config must reach the same validation→Load failure as the
    // no-caps wrapper on empty bytes — proving build_wasi_ctx produces a clean
    // no-preopen ctx and the dual WASI+http linker adds don't break setup.
    let err = run_component_with_wasi(
        &[],
        "p",
        vec![canned_response()],
        &tau_ports::target::WasiConfiguration::deny_all(),
    )
    .unwrap_err();
    assert!(matches!(err, WasmHostError::Load(_)), "got: {err:?}");
}
```

- [ ] **Step 3: Run it to confirm it fails (function absent).**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo build -p tau-wasm-host --tests`
Expected: FAIL — `cannot find function run_component_with_wasi`.

- [ ] **Step 4: Implement the wiring in `lib.rs`.**

(a) Extend imports (after the existing `use wasmtime::...` lines):

```rust
use tau_domain::HttpMethod;
use tau_ports::target::{PreopenAccess, WasiConfiguration};
use wasmtime_wasi::{
    DirPerms, FilePerms, ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
};
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode as WasiHttpErrorCode;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{
    add_only_http_to_linker_sync, default_send_request, HttpResult, WasiHttpCtxView,
    WasiHttpHooks, WasiHttpView,
};
use wasmtime_wasi_http::WasiHttpCtx;
```

(b) Add the error variant to `WasmHostError`:

```rust
    /// Building the WASI context failed — e.g. a preopen directory does not
    /// exist on the host. Surfaced instead of silently creating it.
    #[error("failed to configure WASI context: {0}")]
    WasiConfig(#[source] anyhow::Error),
```

(c) Grow `HostState` and its constructor:

```rust
struct HostState {
    responses: VecDeque<String>,
    clock_millis: u64,
    prng_state: u64,
    /// WASI 0.2 resource table (EPIC 3.3).
    table: ResourceTable,
    /// WASI 0.2 context: exactly the preopens derived from the component's
    /// allow-bounded caps; no stdio/env/args/network inherited (EPIC 3.3).
    wasi: WasiCtx,
    /// wasi:http resource bookkeeping; the egress *decision* lives in `gate`.
    http: WasiHttpCtx,
    /// The allow-bounded network egress gate (EPIC 3.3), consulted by the
    /// `WasiHttpHooks::send_request` override before any outgoing request.
    gate: HttpHostGate,
}

impl HostState {
    fn new(responses: Vec<String>, wasi: WasiCtx, gate: HttpHostGate) -> Self {
        Self {
            responses: responses.into(),
            clock_millis: 0,
            prng_state: PRNG_SEED,
            table: ResourceTable::new(),
            wasi,
            http: WasiHttpCtx::new(),
            gate,
        }
    }
}
```

(d) Implement the wasmtime views + the egress hook (place after `impl host::Host for HostState`):

```rust
impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for HostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            // `gate` is the WasiHttpHooks impl below; disjoint borrow from
            // `http`/`table`, so no aliasing conflict.
            hooks: &mut self.gate,
        }
    }
}

/// The egress gate: every `wasi:http` outgoing request is routed here before
/// wasmtime opens a socket. A host/method the allow-bounded caps did not
/// authorize is rejected — the guest never gets a connection to it.
impl WasiHttpHooks for HttpHostGate {
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let authority = request
            .uri()
            .authority()
            .map(|a| a.as_str().to_string())
            .unwrap_or_default();
        // Unknown/custom method tokens are in no allow set → deny.
        let permitted = match HttpMethod::parse(request.method().as_str()) {
            Ok(method) => self.allows(&authority, &method),
            Err(_) => false,
        };
        if !permitted {
            return Err(WasiHttpErrorCode::HttpRequestDenied.into());
        }
        Ok(default_send_request(request, config))
    }
}
```

(e) Add `build_wasi_ctx` (place near `determinism_config`):

```rust
/// Build a `WasiCtx` granting exactly `cfg`'s preopens (RO for fs.read, RW for
/// fs.write) and nothing else: no stdio/env/args/network inherited. A preopen
/// whose host directory does not exist is a hard error — we never silently
/// create it. Network egress is denied at the ctx level; `wasi:http` is gated
/// separately by `HttpHostGate` in `send_request`.
fn build_wasi_ctx(cfg: &WasiConfiguration) -> Result<WasiCtx, WasmHostError> {
    let mut builder = WasiCtxBuilder::new();
    for (host_dir, access) in preopen_dirs(cfg) {
        let (dir_perms, file_perms) = match access {
            PreopenAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
            PreopenAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
        };
        // Identity map: the guest sees the same absolute path as the host dir.
        builder
            .preopened_dir(host_dir, host_dir, dir_perms, file_perms)
            .map_err(|e| WasmHostError::WasiConfig(e.into()))?;
    }
    Ok(builder.build())
}
```

(f) Add the capped entry and rewrite `run_component` as a wrapper. Replace the existing `pub fn run_component(...) { ... }` body:

```rust
/// Run a component whose WASI authority is bounded by `wasi` (EPIC 3.3):
/// fs preopens and a `wasi:http` egress allow-list built from the same
/// allow-bounded caps that produced the component's WIT world (3.2). An
/// un-granted host or path is unreachable at runtime.
pub fn run_component_with_wasi(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    wasi: &WasiConfiguration,
) -> Result<String, WasmHostError> {
    // Fail fast on a malformed cassette before touching wasmtime.
    for resp in &llm_responses {
        serde_json::from_str::<CompletionResponse>(resp).map_err(WasmHostError::InvalidResponse)?;
    }

    let wasi_ctx = build_wasi_ctx(wasi)?;
    let gate = HttpHostGate::new(wasi);

    let config = determinism_config().map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let engine = Engine::new(&config).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let component =
        Component::new(&engine, wasm_bytes).map_err(|e| WasmHostError::Load(e.into()))?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;
    add_only_http_to_linker_sync(&mut linker).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    Runner::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    let mut store = Store::new(&engine, HostState::new(llm_responses, wasi_ctx, gate));
    let runner = Runner::instantiate(&mut store, &component, &linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    match runner.call_run(&mut store, prompt) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(guest_err)) => Err(WasmHostError::Guest(guest_err)),
        Err(trap) => Err(WasmHostError::Trap(trap.into())),
    }
}

/// Determinism-conformance entry (`WasmProfile`): no capabilities, so no WASI
/// grants — deny-all egress, zero preopens. Behaviourally identical to the
/// pre-3.3 host for a guest that imports no WASI interfaces.
pub fn run_component(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
) -> Result<String, WasmHostError> {
    run_component_with_wasi(wasm_bytes, prompt, llm_responses, &WasiConfiguration::deny_all())
}
```

(g) Update the existing in-`lib.rs` `HostState::new(...)` test call sites: the three unit tests (`clock_advances_by_fixed_step`, `prng_is_deterministic_and_seeded`, `complete_pops_responses_then_errors`) call `HostState::new(vec![...])` with one arg. Update them to `HostState::new(vec![...], WasiCtxBuilder::new().build(), HttpHostGate::new(&WasiConfiguration::deny_all()))`. Add a small test helper in that module:

```rust
fn empty_state(responses: Vec<String>) -> HostState {
    HostState::new(
        responses,
        WasiCtxBuilder::new().build(),
        HttpHostGate::new(&WasiConfiguration::deny_all()),
    )
}
```

and replace the three `HostState::new(...)` calls with `empty_state(...)`.

- [ ] **Step 5: Run the wiring smoke test — expect PASS.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host run_component_with_wasi_no_grants_matches_run_component`
Expected: PASS.

- [ ] **Step 6: Run the FULL crate suite — existing determinism/roundtrip/e2e tests must stay green** (the real guest imports no WASI, so the permissive linker is inert for it).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host`
Expected: PASS — all of: existing cassette/clock/prng tests, the roundtrip/fan_monitor integration tests, the six `wasi::tests`, and the wiring smoke test.

- [ ] **Step 7: Clippy + fmt (CI mirrors `-D warnings`).**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo clippy -p tau-wasm-host --all-targets -- -D warnings
timeout 30 env CARGO_TARGET_DIR=target/agent-e33 cargo fmt -p tau-wasm-host -- --check
```
Expected: clean. (If `default_send_request` is reported unused when the feature is off, confirm `wasmtime-wasi-http`'s `default-send-request` feature is enabled — it is on by default; do not disable default features.)

- [ ] **Step 8: Commit.**

```bash
git add crates/tau-wasm-host/src/lib.rs crates/tau-wasm-host/Cargo.toml Cargo.lock
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-3): configure host WasiCtx + wasi:http egress gate from caps"
```

---

## Final verification & PR

- [ ] **Full green across touched crates:**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-ports
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo nextest run -p tau-wasm-host
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e33 cargo test --doc -p tau-wasm-host
```

- [ ] **Push and open PR to `main`, enroll in merge queue (bare, no `--delete-branch`):**

```bash
git push -u origin feat/epic-3-3-wasi-ctx-host
gh pr create --base main --title "feat(EPIC 3.3): configure host WasiCtx from allow-bounded caps" \
  --body "$(cat <<'EOF'
Configures the wasmtime `WasiCtx` + `wasi:http` egress gate in `tau-wasm-host`
from #533's `WasiConfiguration`, so a component grants exactly its declared
hosts/preopens and nothing more (EPIC 3.3).

- `run_component_with_wasi(&WasiConfiguration)` new entry; `run_component`
  becomes a deny-all wrapper (zero churn to existing callers).
- fs → `preopened_dir` (RO/RW); network → host-side `WasiHttpHooks::send_request`
  allow-list (host + method); hardware → no grant.
- Consumes #533's canonical fold — no re-derivation.
- Enforcement tests (Option A): the un-granted host/path/method is rejected by
  the same gate/preopen objects the live linker installs; deny test written first.

Out of scope: in-guest gate (3.4), WIT-gen (3.2), verify --bundle (3.5),
production caller wiring.
EOF
)"
# then, using the printed PR number N:
gh pr merge <N> --squash --auto
```

---

## Self-Review

**Spec coverage:**
- "network hosts → allowed-hosts" → Task 2 `HttpHostGate` + Task 3 `send_request` gate. ✓
- "fs paths → preopens" → Task 2 `preopen_dirs` + Task 3 `build_wasi_ctx`. ✓
- "hardware → no WASI grant" → inherent: `resolve_wasi_config` drops non-`Wasi` caps; nothing to wire. ✓ (no task needed)
- "grants exactly those and nothing more" → `build_wasi_ctx` inherits no stdio/env/args/network; `preopens_exactly_granted` + `deny_all_config_grants_nothing` assert the "nothing more". ✓
- "un-granted host/path NOT reachable at runtime; deny test first" → `ungranted_host_denied` (first test, Task 2) against the live gate object; `method_outside_set_denied`. ✓
- Consume #533, no re-derivation → `HttpHostGate::new`/`preopen_dirs` read `WasiConfiguration` fields directly. ✓
- New entry + no-caps wrapper, zero caller churn → Task 3 (f). ✓
- deny-all default → Task 1 `WasiConfiguration::deny_all()`. ✓

**Placeholder scan:** none — every code step is complete and compilable; no "TODO"/"handle errors"/"similar to".

**Type consistency:** `HttpHostGate::new`/`allows`/`preopen_dirs` signatures identical across Tasks 2 and 3; `HostState::new(responses, WasiCtx, HttpHostGate)` consistent between constructor (3c), views (3d), entry (3f), and test helper (3g); `WasmHostError::WasiConfig`/`Load` used consistently; `WasiConfiguration::deny_all()` from Task 1 used in Tasks 2 and 3.

**Risk note for the executor:** the wasmtime-wasi 47 API items above are docs.rs-verified by name and signature, but exact trait bounds (e.g. whether `add_to_linker_sync` requires a `WasiView` supertrait like `IoView`) are resolved by the compiler in Step 4/6 — if a supertrait impl is demanded, add the trivial impl mirroring the `table` accessor. No logic changes result.
