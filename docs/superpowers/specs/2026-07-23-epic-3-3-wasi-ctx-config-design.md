# EPIC 3.3 — Host `WasiCtx` from allow-bounded capabilities

Status: approved (Option A)
Crate surface: `tau-wasm-host` (reads `tau-ports::target` mapping + the IR cap set)

## Problem

E3.1 (#511) gave a pure capability → WASI/WIT mapping (`tau_ports::target::map_capability`).
E3.2 (#517) uses it to emit the *WIT world* (the ABI surface) at `tau build wasm`. Neither
touches the running host: `tau-wasm-host::run_component` links only the three `tau:host/host`
deterministic stubs and constructs **no** `WasiCtx` — it has no `wasmtime-wasi` dependency at
all. So a component's declared network/filesystem caps are not enforced by the host at runtime.

E3.3 closes that gap on the **host** side only: build the wasmtime `WasiCtx` from the *same*
allow-bounded caps E3.2 reads, so the guest's sockets/filesystem authority matches its caps and
nothing more.

Out of scope (do not touch): the in-guest gate (3.4, collides with 4.5); WIT generation (3.2);
`tau-pkg` lowering / `tau-ir`.

## Cap source — reuse E3.2 verbatim

Derive caps exactly as `tau-cli::cmd::build_wasm::world_from_module` does — do **not** re-derive
from `EffectiveCapability` or the manifest. By the time this runs the governance gate has proven
`tool ⊆ agent-effective ⊆ root ceiling`, so the IR `declared` set *is* the `[allow]`-bounded set:

```rust
let used: Vec<Capability> = module.workflow.capability_table.0
    .values().flat_map(|req| req.declared.iter().cloned()).collect();
let caps = tau_domain::canon_caps(&used); // deterministic antichain
```

For each cap, read `tau_ports::target::map_capability(cap).config` (the `WasiConfig`), **not**
`.imports` (that was 3.2's concern).

## Data flow

```
IrModule.capability_table ─▶ Vec<Capability> ─canon_caps─▶ antichain
   │  for each cap: map_capability(cap).config          (REUSE E3.1)
   ▼
WasiConfig { None | AllowedHosts{HostSet,methods} | Preopens(Vec<Preopen>) }
   │  NEW in tau-wasm-host (host-only; needs wasmtime types)
   ▼
WasiGrants { hosts: HostAccess, preopens: Vec<PreopenGrant> }
   │  apply
   ▼
WasiCtxBuilder (+ WasiHttpView) ─▶ Store<HostState{ WasiCtx, ResourceTable, WasiHttpCtx, +stubs }>
```

## New pure translation (host-only, unit-testable)

The point where "grants exactly those and nothing more" is asserted directly.

```rust
/// Network egress policy folded across all caps.
pub enum HostAccess {
    DenyAll,             // no network cap present
    Any,                 // some Http{ hosts: HostSet::Any }
    Only(BTreeSet<String>), // union of exact host authorities (host[:port] strings)
}
impl HostAccess {
    pub fn permits(&self, authority: &str) -> bool; // DenyAll=>false, Any=>true, Only=>member
}

pub struct PreopenGrant {
    pub host_path: PathBuf,   // sandbox_root joined with the glob's static prefix dir
    pub guest_path: String,   // the glob's static prefix dir, as the guest sees it
    pub access: PreopenAccess // ReadOnly | ReadWrite, from fs.read / fs.write
}

pub struct WasiGrants { pub hosts: HostAccess, pub preopens: Vec<PreopenGrant> }

/// Walk caps → WasiConfig → grants, resolving fs globs against `sandbox_root`.
pub fn wasi_grants_from_caps(
    caps: &[Capability],
    sandbox_root: &Path,
) -> Result<WasiGrants, WasmHostError>;
```

### Preopen glob → dir rule (E3.1 deferred this to 3.3)

Caps carry glob patterns (`/data/**`, `/out`, `/data/*.txt`). WASI preopens are concrete dirs
with no per-file filtering. The host grants **coarse dir-level authority = the glob's static
prefix directory**, mapped under `sandbox_root`:

| cap glob      | guest_path | host_path                |
|---------------|------------|--------------------------|
| `/data/**`    | `/data`    | `sandbox_root/data`      |
| `/out`        | `/out`     | `sandbox_root/out`       |
| `/data/*.txt` | `/data`    | `sandbox_root/data`      |

Fine-grained per-glob path matching is **3.4's job** (in-guest gate). This split is the clean
boundary: host = structural sandbox (a path outside every preopen has no descriptor at all),
in-guest gate = fine authorization within a preopen. `fs.write.max_bytes` is likewise 3.4's
concern (not expressible as a preopen), and is intentionally not carried here.

### Folding rules

- **hosts:** no `Http` cap ⇒ `DenyAll`. Any `Http{HostSet::Any}` ⇒ `Any`. Else `Only(∪ exact
  hosts)`. (`canon_caps` usually collapses to one `Http` cap; the fold is still order-independent.)
  Branch on `HostSet::is_any()` first — `exact_hosts()` returns empty for `Any`.
- **preopens:** one grant per fs cap; dedupe identical `(guest_path, access)`; if the same dir
  appears as both RO and RW, RW wins.
- **hardware / Custom / host-mediated / in-guest** caps ⇒ no grant (skipped), matching
  `generate_world` dropping them.
- Any `Disposition::Unsupported` cap ⇒ error (belt-and-suspenders; `generate_world` already
  rejected these at build).

## Host wiring

- Add deps: `wasmtime-wasi = "45"`, `wasmtime-wasi-http = "45"` (must match the pinned
  `wasmtime = "45"`; note the RUSTSEC-2026-0222 acceptance in `deny.toml` already scopes wasmtime
  to the dev graph — confirm no new advisory/license entry is needed).
- `HostState` gains `ResourceTable`, `WasiCtx`, `WasiHttpCtx`, and a `HostAccess` (for the http
  filter). Implement `IoView` (`table`), `WasiView` (`ctx`), `WasiHttpView` — the last overrides
  `send_request` to reject an authority failing `self.hosts.permits(authority)` with
  `ErrorCode::HttpRequestDenied` (no socket opened).
- Build `WasiCtx` from `WasiGrants`: `WasiCtxBuilder::preopened_dir(host_path, guest_path,
  DirPerms, FilePerms)` per grant (perms from `access`); network left un-inherited (deny by
  default) — wasi:http egress is gated solely by the `WasiHttpView` filter.
- Link `wasmtime_wasi::add_to_linker_sync` + `wasmtime_wasi_http::add_only_http_to_linker_sync`
  alongside the existing `Runner::add_to_linker`.

## Public API

New entry, keeping the determinism-conformance path byte-identical:

```rust
pub fn run_component_with_caps(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    caps: &[Capability],
    sandbox_root: &Path,
) -> Result<String, WasmHostError>;
```

`run_component(bytes, prompt, cassette)` stays as a thin wrapper = "no caps ⇒ `DenyAll` + no
preopens" so existing callers and `WasmProfile` determinism are untouched. A convenience
`run_module_with_caps(module, …)` may derive caps from an `IrModule` using the E3.2 walk.

New `WasmHostError` variants as needed (e.g. `WasiConfig(#[source] anyhow::Error)` for
preopen/dir-open failures, `UnsupportedCap { reason }`). thiserror at the boundary; anyhow
internally.

## Testing

**TDD — the negative-enforcement test is written first** and drives the API.

1. **Runtime enforcement (filesystem, deterministic, offline)** — new `tests/wasi_fs_enforcement.rs`:
   - A minimal `wasm32-wasip2` **fs-probe** fixture implementing the `runner` world; its `run`
     does `std::fs::read(prompt)` and returns `Ok("read N bytes")` / `Err(io error)`. Built
     in-test via the existing `build_guest_component` shell-out pattern (`#[ignore]`, like the
     current guest-building integration tests).
   - Preopen a tempdir as `/data` (grant). Assertions:
     - granted: `run("/data/ok.txt")` ⇒ `Ok`.
     - **un-granted: `run("/etc/secret")` ⇒ `Err`** (no preopen ⇒ no descriptor). This is the
       roadmap's "un-granted path is NOT reachable from the guest at runtime."
2. **Grant derivation (pure unit tests)** in `src`:
   - `wasi_grants_from_caps`: fs.read/fs.write → correct preopens + access; glob prefix rule;
     RW-wins dedupe; hardware/in-guest skipped; `Http{Any}` ⇒ `Any`, exact hosts ⇒ `Only`, no net
     ⇒ `DenyAll`.
   - `HostAccess::permits`: DenyAll/Any/Only membership, incl. host:port authorities.
3. Existing `run_component` unit tests and `wit_host_drift` stay green (no behavior change).

## Risks / notes

- **wasmtime-wasi v45 API drift:** `IoView`/`WasiView` split and `add_only_http_to_linker_sync`
  signatures verified during execution against the pinned 45.x. If wasi-http linking proves
  disproportionately heavy for plumbing no current guest consumes, the http filter may ship as
  the unit-tested `HostAccess` + `WasiHttpView` wiring without a live guest (already Option A).
- **No current guest uses wasi:fs or wasi:http** — inference is host-mediated via
  `tau:host/host::complete`. These grants are forward-looking; the fs-probe fixture is synthetic,
  authored solely to exercise enforcement.
- Branch: `feat/epic-3-3-wasi-ctx-config`. PR to `main`; merge queue on.
