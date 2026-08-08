# EPIC 3.3 — Configure host `WasiCtx` from allow-bounded caps (design)

**Date:** 2026-08-08
**Branch:** `feat/epic-3-3-wasi-ctx-host`
**Crate surface:** `tau-wasm-host` (the wasmtime embedder), consuming
`tau_ports::target::WasiConfiguration` (EPIC 3.3 data fold, PR #533).

## Problem

EPIC 3 lowers capabilities per target and, on wasm, generates the WIT world
from the allow-bounded cap set (3.1 mapping table #511; 3.2 world-gen #517;
3.3 data fold `resolve_wasi_config → WasiConfiguration` #533). What is still
missing is the **host side**: the running wasmtime component gets no `WasiCtx`
at all. `tau-wasm-host` today only satisfies the three `tau:host/host` stub
imports; it links no WASI and grants no preopens or network. So a guest built
with, say, an `fs.read "/data/**"` cap has no way to reach `/data` at runtime,
and — the security-relevant direction — nothing *stops* a future guest from
reaching a host or path its caps never authorized, because there is no
`WasiCtx` policy in force.

**3.3 goal:** build the wasmtime `WasiCtx` (+ the `wasi:http` egress gate) from
the *same* allow-bounded caps, so the component grants exactly its declared
hosts/preopens and nothing more.

## Scope

In scope (host WasiCtx wiring + enforcement test only):

- Consume `WasiConfiguration` (`allowed_hosts: HostSet`, `methods`,
  `preopens: Vec<ResolvedPreopen>`) — **no re-derivation** of the cap fold; that
  logic lives in `tau-ports` (#533).
- `fs` preopens → `WasiCtxBuilder::preopened_dir` (RO for `fs.read`, RW for
  `fs.write`).
- `network` hosts → a host-side `wasi:http` egress gate that denies any
  authority not in `allowed_hosts` (and any method not in `methods`).
- `hardware` → no WASI grant (the fold already drops it: `HostMediated` /
  `Unsupported` carry no `WasiConfig`).
- Enforcement test proving an un-granted host/path is not reachable.

Explicitly **out of scope**:

- The in-guest gate (3.4 — and 3.4 collides with 4.5; leave it).
- WIT-world generation (3.2, done).
- `verify --bundle` reproducibility of the WIT (3.5).
- Wiring a production caller in `tau run` / `tau build wasm` to feed the
  `WasiConfiguration` through. The seam (fold at build time where the
  `IrModule` is in hand, per `world_from_module`) is documented but not built
  here — there is no production caller of `run_component` yet, so this slice
  delivers the host capability and its test.

## Approach

### Entry points (minimise churn)

`run_component` stays exactly as-is in signature and becomes a **no-caps
wrapper** — the determinism-conformance entry (`WasmProfile`) that grants no
WASI. The three existing callers
(`build_wasm_e2e.rs`, `fan_monitor_simple.rs`, `roundtrip.rs`) are unchanged.

A new entry takes the already-folded config:

```rust
pub fn run_component_with_wasi(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    wasi: &WasiConfiguration,   // tau_ports::target — #533's canonical fold
) -> Result<String, WasmHostError>;

// no-caps wrapper == deny-all egress, zero preopens
pub fn run_component(bytes, prompt, resp) =
    run_component_with_wasi(bytes, prompt, resp, &WasiConfiguration::deny_all());
```

Passing `&WasiConfiguration` (not `&[Capability]`) keeps the hexagonal
boundary clean: `tau-wasm-host` consumes the fold, never performs it. The
deny-all default is `resolve_wasi_config(core::iter::empty())`
(`allowed_hosts: Exact(∅)`, no preopens) — semantically identical to today's
no-WASI behaviour. (If `tau-ports` lacks a `deny_all()` constructor we add one
there, or fold `empty()` locally.)

### Components

**`tau-wasm-host/src/wasi.rs` (new)** — the pure, unit-testable seam. No
wasmtime types leak in.

- `struct HttpHostGate { allowed: HostSet, methods: Option<BTreeSet<HttpMethod>> }`
  - `fn allows(&self, authority: &str, method: &HttpMethod) -> bool`
    - host: `allowed.is_any() || allowed.exact_hosts().contains(authority)`
      (authority is `host` or `host:port`; exact-string match — port is part of
      the authority, matching #533/E3.1 semantics).
    - method: `methods.is_none() || methods.as_ref().unwrap().contains(method)`.
  - Built from a `&WasiConfiguration` via `HttpHostGate::from(cfg)`.
- `fn preopen_dirs(cfg: &WasiConfiguration) -> Vec<(&str host_dir, PreopenAccess)>`
  — the exact preopen set the builder will grant (identity-mapped: guest path ==
  host_dir, since #533's `host_dir` is already absolute + glob-resolved). This
  is what the test asserts against — the same list the real builder consumes.

**`tau-wasm-host/src/lib.rs`** — wasmtime wiring:

- `HostState` grows `table: ResourceTable`, `wasi: WasiCtx`,
  `http: WasiHttpCtx`, `gate: HttpHostGate`.
- `impl WasiView for HostState` / `impl WasiHttpView for HostState` — the http
  view hands the linker the `HttpHostGate` as its request hook.
- `impl WasiHttpHooks for HttpHostGate::send_request` — the egress gate: read
  the outgoing request's authority + method; if `!gate.allows(..)`, return
  `ErrorCode::HttpRequestDenied` **before wasmtime opens a socket**; else
  `default_send_request`. (Exact trait/type names verified against
  wasmtime-wasi-http **47** during planning — reference wiring is 45.)
- `fn build_wasi_ctx(cfg: &WasiConfiguration) -> Result<WasiCtx, WasmHostError>`
  — one `.preopened_dir(host_dir, host_dir, dir_perms, file_perms)` per
  `ResolvedPreopen`; RO→(`DirPerms::READ`,`FilePerms::READ`),
  RW→(all, all). No network inherited; no stdio/env/args inherited. A missing
  `host_dir` surfaces as `WasmHostError::WasiConfig` (we do **not** silently
  `create_dir_all` — caps name dirs that are expected to exist).
- `run_component_with_wasi` builds ctx + gate, adds
  `wasmtime_wasi::p2::add_to_linker_sync` + `add_only_http_to_linker_sync` +
  `Runner::add_to_linker` to the linker, instantiates, drives `run`.
  Full-WASI linking is inert for a guest that imports none (the guest's world,
  fixed by 3.2 from its caps, determines what it can import); it only *enforces*
  for guests that do import `wasi:http` / `wasi:filesystem`.

### Dependencies

Add to `tau-wasm-host/Cargo.toml`: `wasmtime-wasi` and `wasmtime-wasi-http`,
both pinned to the workspace `wasmtime` major (**47**, matching `Cargo.lock`
47.0.3). No workspace-wide version change.

### Error handling

`thiserror` at the crate boundary. `WasmHostError` gains `WasiConfig(#[source]
anyhow::Error)` (preopen/build failure) and `UnsupportedCap { reason }` (a cap
that folds to `Disposition::Unsupported` — belt-and-suspenders; 3.2 build gate
should already reject it). `anyhow` internal. `send_request` denial reaches the
guest as its normal `wasi:http` error arm — from the guest's view the
un-granted host is simply unreachable. `#![forbid(unsafe_code)]` (workspace
lint).

## Testing — Option A (boundary/predicate at the real enforcement point)

The chosen depth (see decision log below): test the *actual* gate objects the
live linker uses, not a re-derivation, and not a full guest fixture. Written
TDD, the deny test first (red before green).

`wasi.rs` unit tests:

1. **`ungranted_host_denied`** (first, red): `HttpHostGate` from
   `allowed_hosts = Exact({"api.example.com"})` →
   `allows("api.example.com", &Get) == true`,
   `allows("evil.example.com", &Get) == false`. This gate is the exact object
   `WasiHttpHooks::send_request` consults, so the assertion is the runtime
   enforcement mechanism.
2. **`deny_all_denies_every_host`**: `Exact(∅)` denies all authorities.
3. **`any_host_permits_all`**: `HostSet::Any` permits any authority.
4. **`method_outside_set_denied`**: `methods = Some({Get})` →
   `allows(h, &Post) == false`; `methods = None` permits every method.
5. **`preopens_exactly_granted`**: `preopen_dirs` of a config with one RW
   `/work` yields exactly `[("/work", ReadWrite)]`; an un-granted `/etc` never
   appears; empty preopens → empty.
6. **`deny_all_config_grants_nothing`**: `WasiConfiguration::deny_all()` →
   gate denies every authority, `preopen_dirs` empty (proves the wrapper
   default preserves today's no-WASI behaviour).

`lib.rs` wiring smoke test:

7. **`run_component_with_wasi_no_grants_matches_run_component`**: an empty/
   deny-all config reaches the same validation→`Load` failure as
   `run_component` on empty bytes — proves `build_wasi_ctx` builds a clean
   no-preopen ctx and the dual linker adds don't break instantiation.

Existing `run_component` roundtrip/fan_monitor/e2e tests must stay green
unchanged (guest imports no WASI → unaffected by the permissive linker).

## Isolation & clarity

`HttpHostGate::allows` and `preopen_dirs` are pure functions of
`&WasiConfiguration`: understandable and testable without wasmtime
instantiation, yet they are the *same* objects the live linker/gate consult.
`build_wasi_ctx` is the only wasmtime-touching unit and has one job. The fold
(`resolve_wasi_config`) is owned by `tau-ports`; this crate never re-derives it.

## Decision log

- **Consume #533's `WasiConfiguration`, don't re-derive.** A prior abandoned
  branch (`feat/epic-3-3-wasi-ctx-config`, no PR, wasmtime 45) re-implemented
  the fold as `wasi_grants_from_caps`/`WasiGrants`/`HostAccess` with a
  `sandbox_root` remap. #533 landed the canonical fold *after* that branch
  forked, making the re-derivation redundant and its `sandbox_root`/
  `glob_prefix_dir` logic unnecessary (`host_dir` is already absolute +
  glob-resolved). We consume `WasiConfiguration` directly. The prior branch's
  `wasi.rs`/`lib.rs` WASI-wiring and fs-fixture are kept only as *reference*
  (`.context/e33-prior-ref/`).
- **Test depth = Option A** (boundary/predicate), not Option B (fs-fixture
  round-trip) or C (+http listener). For `wasi:http` the host predicate *is*
  the only enforcement point that exists (wasmtime-wasi-http has no built-in
  hostname allow-list), so testing it directly is faithful. For fs, asserting
  the constructed preopen set == granted dirs (and nothing else) captures the
  "nothing more" acceptance without a wasm build. A full guest round-trip is
  disproportionate for a host-wiring slice.
- **Network maps to `wasi:http`, not `wasi:sockets`.** The 3.1 table (#511)
  chose `wasi:http/outgoing-handler`; the roadmap's "socket config" wording is
  loose. Enforcement is therefore the `WasiHttpView` host gate, not
  `WasiCtxBuilder::socket_addr_check`.
- **Full-WASI linking is safe.** Adding all of preview2 to the linker grants
  nothing to a guest whose world (3.2, cap-bounded) doesn't import it, so
  determinism for cap-less guests is preserved; conditional per-interface
  linking would be extra code for no capability gain.
