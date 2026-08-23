# EPIC 3.6 — Guest effect ABI (net-only) — Design

**Date:** 2026-08-10
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md` EPIC 3, story 3.6
**Predecessors (all MERGED):** 3.2 load-bearing world (#517/#543), 3.3 host WasiCtx (#533/#536/#544/#546), 3.4 drop in-guest gate (#557)
**Scope:** net-only. fs.read/fs.write deferred to a 3.6-b follow-on.

## Problem

EPIC 3's DoD: *"an ungranted cap is un-importable at the ABI; wasm caps == the `[allow]`-bounded set."*

3.2 and 3.4 met that at two layers only:

- **3.2 (world text):** `tau build wasm` compiles the guest against a capability-EXACT WIT world. An ungranted interface is absent from the world text fed to `wit_bindgen::generate!`, so it is un-importable *in source*.
- **3.3 (host WasiCtx):** `tau-wasm-host::run_component_with_caps` builds a wasmtime `WasiCtx` (preopens + a `wasi:http` host+method egress gate) from the allow-bounded caps.
- **3.4 (gate partition):** on `wasm32` the in-guest dispatch gate skips `Disposition::Wasi` caps, so a rerouted net/fs effect reaches the host WasiCtx instead of being denied by the guest's empty-stub grant.

The gap 3.6 closes: the guarantee is **not binary-observable and the host gate is not live**. The production guest (`crates/tau-wasm-guest`) routes *no* effects through WASI — tools go to `tau_native_tools::invoke` in-process; inference/clock/random cross the three `tau:host/host` imports. Because nothing calls `wasi:http`/`wasi:filesystem`, `wasm-ld` DCE strips **every** WASI import (granted or not) from the compiled component. So:

- `wit_component::decode(<out>.wasm)` shows no WASI imports regardless of grant → the "un-importable at the ABI" claim is unverifiable in the binary.
- The host WasiCtx egress gate never runs on the real guest — its only exerciser is the synthetic `http-probe` test fixture.

3.4 is 3.6's prerequisite: without dropping the Wasi-cap in-guest gate, a rerouted effect would be denied by the guest's empty-stub grant before reaching the host.

## Goal

Route the guest's `net.http` effect through `wasi:http` using the guest's **own cap-derived generated bindings**, so that:

1. **Binary-observable:** a granted `net.http` produces a component whose imports include `wasi:http/*`; an ungranted one produces a component with no `wasi:http` import. Provable via `wit_component::decode`.
2. **Live host enforcement:** an ungranted host/method is denied at the host `WasiCtx`/`EgressPolicy` at **runtime**, through the real production guest driven by real IR — not just the offline synthetic `http-probe`.

## Non-goals

- fs.read / fs.write effects (→ 3.6-b, reuses the identical cfg-gate + generated-bindings pattern).
- A live/positive HTTP connect assertion (offline can't open a socket; see Testing).
- Multi-agent or pipeline-in-wasm execution (guest stays single-agent, `guest.rs`).
- Any change to `tau_native_tools`, the 3.4 dispatch gate (`check_capabilities_for_tool`), or `AttenuatedDispatcher`.
- Resurrecting the never-implemented `tau.caps` custom-section / `AmbientOpsGate`.

## Key architectural decisions

### D1. Generated bindings, not the external `wasi` crate

The `http-probe` fixture (`crates/tau-wasm-host/tests/fixtures/http-probe`) uses the external `wasi` crate 0.14 **only because** its world is the frozen `runner` (tau:host, no WASI) — it needs external bindings to reach `wasi:http`. The production guest is different:

- Its world is cap-derived and already vendors `wasi:http`/`wasi:filesystem` into `wit-gen/deps/` (3.2).
- `wit_bindgen::generate!({ world: "tau:generated/runner", path: "wit-gen", generate_all })` already emits guest-side bindings for **every** imported interface, including `wasi:http/outgoing-handler` when `net.http` is granted.

Critically, the production guest **already exports `cabi_realloc`** itself (`guest.rs`, required for the no_std wasip2 component). Pulling in the external `wasi` crate would emit a *second* `cabi_realloc` → duplicate-symbol link error (the exact failure the 3.3 memo warns about). So generated bindings are **forced, not merely preferred**. They are also no_std-native (the same wit-bindgen path already used for `tau:host/host`), which is what makes the no_std concern low-risk.

### D2. The `Fetch` effect arm lives in `GuestDispatcher`, not `tau_native_tools`

`tau_native_tools` is the shared no_std single-source crate compiled for **both** native and wasm targets — it cannot reference `wasi:http`. The wasi-backed effect must live in the guest crate, which owns the generated `wasi::http` bindings. It goes in `GuestDispatcher::invoke` (`crates/tau-wasm-guest/src/dispatcher.rs`) as a new match arm, checked **before** the existing `tau_native_tools::invoke` fallthrough.

### D3. Key the effect arm on the declared native fn name, resolved from the module

The interpreter builds `DispatcherTool { tool_id, tool_impl, .. }` where `tool_id` is the arbitrary user-chosen tool-ref key (e.g. `fetch`) and the stable native contract is `ToolImpl::Native { fn_ref.name }` (e.g. `Fetch`). `GuestDispatcher::invoke` receives only `tool_id`. To key the effect on the stable contract (not the arbitrary ref key), `GuestDispatcher` gains `module: Arc<tau_ir::Module>` (already available at the `guest.rs` construction site) and resolves `tool_id → workflow.tools[tool_id].impl_` to read `fn_ref.name`, matching `"Fetch"`.

The existing `tau_native_tools::invoke(tool_id.0)` fallthrough is **unchanged** — it keeps keying on `tool_id.0`, preserving the current cassette scenarios (`read_temp`/`set_fan` etc.). Only the new wasi arm resolves the native fn name.

### D4. `build.rs` cfg gate drives static reachability

`crates/tau-wasm-guest/build.rs` already assembles `wit-gen/` and has the world text in hand. It emits `cargo:rustc-cfg=tau_cap_net_http` **iff** the world contains `wasi:http` (plus the matching `cargo:rustc-check-cfg`). The `Fetch` arm is `#[cfg(tau_cap_net_http)]`:

- Granted → cfg on → arm compiled → statically reachable from the `run` export → `wasi:http` import survives DCE.
- Ungranted → world has no `wasi:http` → cfg off → arm not compiled → no `wasi:http` import (and the arm's generated-binding reference wouldn't compile anyway, since the interface is absent from the world — the cfg gate is what prevents that).

DCE is based on **static** reachability from exports, not runtime reachability: the arm need not actually fire for the import to survive. This is what makes binary-observability hold without a live network call.

## Data flow

```
tau.toml  [tools.fetch] native="Fetch" capabilities=[{kind="net.http", hosts=[...]}]
   │  tau build wasm
   ▼
generate_world(caps) → wit-gen/ world  import wasi:http/outgoing-handler@0.2.3;
   │
build.rs: world contains "wasi:http"  →  cargo:rustc-cfg=tau_cap_net_http
   │
guest compiled (generate_all → crate::wasi::http bindings present)
   │  runtime: run_component_with_caps(wasm, prompt, cassette, caps, sandbox_root)
   ▼
run → run_ir_streaming → agent turn → cassette tool_use "fetch"
   → DispatcherTool::invoke → GuestDispatcher::invoke(ToolId("fetch"), args)
       resolve module.workflow.tools["fetch"].impl_ → ToolImpl::Native{fn_ref.name="Fetch"}
       #[cfg(tau_cap_net_http)] native=="Fetch":
          wasi::http::outgoing_handler::handle(request)   ← generated, no_std
          host WasiHttpHooks::send_request runs during dispatch:
             host/method permitted?  yes → socket ; no → Err(HttpRequestDenied) before any socket
   → Ok {"status":u16,"body":string}  |  Err → ToolInvocationResult{error: Some("<code>: <detail>")}
```

## Effect tool contract (`Fetch`)

```
input   { "url": string, "method"?: string (default "GET") }
output  { "status": u16, "body": string }               // on ToolInvocationResult.body
error   ToolInvocationResult.error = Some("<EgressPolicy code>: <detail>")   // e.g. carries HttpRequestDenied
```

- Missing/invalid `url` → error result, no WASI call.
- Denied host/method → host returns before any socket; guest surfaces the exact `HttpRequestDenied` code.
- No panics on the effect path; serialization/parse failures become error results.

## Soundness invariants (keep 3.4 sound)

- **No re-added in-guest cap gate.** `check_capabilities_for_tool` still skips `Disposition::Wasi` on wasm (#557). Enforcement is the host WasiCtx only.
- **`InGuest` caps stay gated** (agent/skill.spawn, tasklist, plan) — not routed through WASI.
- **`AttenuatedDispatcher` untouched** (`interpreter/attenuate.rs`, 4.5 surface).
- **`tau_native_tools` unchanged** (shared native+wasm crate).
- **Fail-closed preserved:** `map_capability` catch-all `_ => HostMediated`; only `net.http` is `Wasi` in scope here.

## Spike gate (blocking, plan Task 0)

Confirm `generate_all` produces **callable, no_std-linking** guest bindings for `wasi:http/outgoing-handler`, and a minimal `Fetch` arm compiles and links to a valid component under `cargo build -p tau-wasm-guest --target wasm32-wasip2 --release` (release surfaces the `_rdl_*` allocator LTO bug if any).

- **PASS** (high confidence — same wit-bindgen path as `tau:host/host`): proceed with the full design.
- **FAIL:** fall back to Tier-2 — document the binding gap, keep the world-text DoD only, do not ship the runtime reroute. The epic's binary-observable DoD would then remain open pending a bindgen fix.

## Testing (part of done)

All wasm-lane tests are `#[ignore]`d (they build the `wasm32-wasip2` guest) and run with `--run-ignored`.

1. **Strengthen the existing DoD** — `crates/tau-cli/tests/build_wasm_world_dod.rs`. Today its `wit_component::decode` assertions are all *negative* and vacuous (DCE strips everything). Add a **positive** binary assertion: the `net-http` fixture's compiled component imports `wasi:http/*`. `trivial` still imports no `wasi:`. This is the binary-observable DoD going live. No fixture changes required (`net-http` already declares `[tools.fetch] native="Fetch"` with `net.http`).

2. **New live-enforcement round-trip** — a new `#[ignore]` wasm-lane test (`tau-cli` or `tau-wasm-host`). `tau build wasm` a net.http-granting project → `run_component_with_caps` with a cassette firing `Fetch` at an **ungranted** host → assert the exact `HttpRequestDenied` code surfaces through the real production guest. Denial-only (offline; positive/live-connect omitted — a granted host can't open a socket offline, indistinguishable from denial without a mock server, and the DoD is about denial/un-importability).

3. **Guest unit coverage** — arg parsing and error mapping where testable natively; the wasm-only arm is exercised by (1) + (2).

## Files touched

- `crates/tau-wasm-guest/build.rs` — emit `tau_cap_net_http` cfg from the world text.
- `crates/tau-wasm-guest/src/dispatcher.rs` — `GuestDispatcher` gains `module`; new `#[cfg(tau_cap_net_http)]` `Fetch` arm over generated `wasi::http` bindings.
- `crates/tau-wasm-guest/src/guest.rs` — pass `module` into `GuestDispatcher::new`.
- `crates/tau-cli/tests/build_wasm_world_dod.rs` — add the positive binary-observable assertion.
- New round-trip test (module TBD in plan: `tau-cli` or `tau-wasm-host`).
- Possibly a new fixture for the round-trip cassette (or reuse `net-http` + an inline cassette).

## Open items for the plan

- Exact module for the round-trip test (`tau-cli` vs `tau-wasm-host`) and whether it reuses the `net-http` fixture or adds a small one.
- Precise generated binding path for `wasi:http/outgoing-handler` under the `tau:generated/runner` world (resolved by the spike).
- Whether `GuestDispatcher` holds `Arc<Module>` or a precomputed `tool_id → native_fn_name` map (map avoids holding the whole module; decide in plan).
