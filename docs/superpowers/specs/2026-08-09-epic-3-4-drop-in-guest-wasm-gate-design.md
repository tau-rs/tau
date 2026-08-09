# EPIC 3.4 — Drop the in-guest capability gate on wasm

**Status:** approved (brainstorm), pending implementation plan
**Date:** 2026-08-09
**Roadmap:** EPIC 3 story 3.4 (`docs/superpowers/plans/vision-roadmap.md`)
**Predecessors on `main`:** 3.1 (#511), 3.2 (#517 + #543 load-bearing world),
3.3 (#533 resolver + #536 host `WasiCtx` + #544 egress follow-ups)
**Related:** subflow-runtime-attenuation (D1-C / 4.5,
`docs/superpowers/specs/2026-07-19-subflow-runtime-attenuation-design.md`);
ADR-0057 (root `[allow]` governance lattice)

## Problem

The wasm guest drives the **same** kernel agent loop as native
(`tau_runtime_core::stream::run_streaming_inner`, reached via
`interpreter::run_ir_streaming`). Inside that loop the runtime capability gate
`check_capabilities_for_tool` fires at two sites, with **no** `cfg`/feature
guarding it — it runs identically on every target:

- `stream.rs:763` — **virtual tools** (tasklist, plan, agent.spawn,
  skill.spawn): required cap from `orchestration::required_capability`.
- `stream.rs:1460` — **plugin/native tools**: required caps from
  `tool.capabilities()`.

On wasm this in-guest software check is now **redundant with, and — given the
guest's empty grant (below) — actively contradictory to** the two enforcement
paths EPIC 3 built:

- **3.2 ABI:** the guest is compiled against a WIT world generated from the
  allow-bounded caps; an ungranted `wasi:*` interface is absent from the world,
  so it emits no bindings and is un-importable at the source ABI.
- **3.3 host `WasiCtx`:** `tau-wasm-host` provisions preopens + an egress
  allow-list from exactly the granted caps; an ungranted host/method/path is
  denied at the host boundary before any socket/descriptor opens (#536, #544).

The roadmap story: *"Drop the in-guest gate on wasm; OS gate stays for
host/native."* Epic DoD: *"an ungranted cap is un-importable at the ABI; wasm
caps == `[allow]`-bounded set."*

### The design hole this spec resolves

"Drop the in-guest gate" **must not** mean "remove the gate wholesale." The gate
covers two disjoint capability classes (per the 3.1 `Disposition` table,
`tau_ports::target::wasi_map`):

| `Disposition` | Examples | WASI realization? | Enforcement owner on wasm |
|---|---|---|---|
| `Wasi` | net.http, fs.read, fs.write | yes | **ABI (3.2) + host `WasiCtx` (3.3)** |
| `InGuest` | agent.spawn, skill.spawn, tasklist, plan | **none** | **in-guest** (must stay) |
| `HostMediated` | `Custom`, hardware | n/a | out of scope for wasm cap gating |
| `Unsupported` | fs.exec, process.spawn | n/a | build-rejected (feature-fit) |

`InGuest` caps have **no ABI/host realization** — dropping their in-guest check
would leave them ungoverned at runtime on wasm. So 3.4 drops the in-guest check
**only for `Disposition::Wasi` caps** and keeps it for the rest.

### Two facts that bound the change (verified on `main`)

1. **The guest's granted set is the empty stub.** `prepare_agent_run` builds a
   `stub_manifest` with `capabilities: []` (`interpreter/agent_loop.rs:381`); the
   guest sets no `granted_capabilities_override` and no `capability_resolver`, so
   `run.rs:303` passes through the empty set. The gate is therefore live but
   **fail-closed**: any cap-requiring tool would be *denied*. It is not exercised
   today only because the E2 guest is cassette-only (single agent, `guest.rs:124`;
   `tau_native_tools` declare no caps, so empty-required passes).
2. **The guest routes no effects through WASI.** `GuestDispatcher::invoke`
   (`dispatcher.rs`) calls `tau_native_tools::invoke` in-process;
   inference/clock/random cross via the three `tau:host/host` imports
   (`host_ports.rs`). No `wasi:http`/`wasi:filesystem` call path exists in the
   production guest, so wasm-ld DCE strips all WASI imports. Making the DoD
   *binary*-observable requires rerouting effects through WASI — that is **story
   3.6 (guest effect ABI)**, out of scope here; 3.4 is 3.6's prerequisite.

The documented `AmbientOpsGate` + `tau.caps` custom-section enforcement
(`interpreter/tool_dispatch.rs:6`, "per D-3") was **never implemented** — no code
writes or reads `tau.caps`. 3.4 does not resurrect it.

## Non-goals

- **No native behavior change.** On host/native the OS sandbox gate stays *and*
  the in-guest check stays (defense in depth). 3.4 touches wasm only.
- **No effect rerouting** through `wasi:http`/`wasi:filesystem` (= story 3.6).
- **No guest grant-threading.** The guest's granted set stays the empty stub; the
  kept `InGuest` gate stays fail-closed. Populating the guest's real
  `[allow]`-bounded grant so an `InGuest` cap is *allowed when granted* would
  reverse the attenuation spec's non-goal #1 ("the top-level agent's own tool
  calls remain ungated at runtime") on the native bundle path too — it is
  entangled with 3.6 (where a guest first exercises such a cap) and is deferred
  there. Safe today: the single-agent cassette guest cannot reach an `InGuest`
  cap.
- **No `AmbientOpsGate`/`tau.caps` implementation.**
- **No IR data-model or format change.** No new error variant on the hot path.
- **No change to `AttenuatedDispatcher`** (the 4.5 surface — see reconciliation).

## Design

### Mechanism — classify required caps by `Disposition` at the gate

The single decision point is `check_capabilities_for_tool`
(`tau-runtime-core/src/capability.rs`), which both `stream.rs` sites call. It
receives the *required* caps for a tool. Add a wasm-only pre-filter that removes
`Disposition::Wasi` required caps before the satisfies-scan:

```rust
// tau-runtime-core/src/capability.rs
//
// On the wasm target, capabilities whose Disposition is `Wasi` are enforced by
// the generated WIT world (EPIC 3.2) + the host WasiCtx (EPIC 3.3), NOT by this
// in-guest check. Skipping them here is what "drop the in-guest gate on wasm"
// (story 3.4) means. Every other disposition (InGuest / HostMediated / unknown)
// is still checked against `granted` — fail-closed. Native builds keep the full
// check (the OS sandbox gate is *additional*, not a replacement).
#[cfg(target_arch = "wasm32")]
fn in_guest_gated(required: &Capability) -> bool {
    use tau_ports::target::wasi_map::{map_capability, Disposition};
    !matches!(map_capability(required).disposition, Disposition::Wasi)
}
#[cfg(not(target_arch = "wasm32"))]
fn in_guest_gated(_required: &Capability) -> bool {
    true
}
```

`check_capabilities_for_tool` filters `required` through `in_guest_gated` before
delegating to the pure `check_capabilities` scan. Consequences:

- `tau-runtime-core` already depends on `tau-ports` (no_std / `serde` path,
  `Cargo.toml:13`); `target::wasi_map::{map_capability, Disposition}` is a
  `#![no_std]` pub export reachable in the wasm guest build. No new dependency,
  no feature flag.
- The 3.1 `Disposition` table is the **single source of truth** for "who owns
  this cap on wasm." 3.4 invents no new taxonomy; if 3.1 later reclassifies a
  cap, the gate follows automatically.
- **Both** call sites (`stream.rs:763` virtual, `:1460` plugin) route through
  `check_capabilities_for_tool`, so the one filter covers both. The virtual-tool
  caps (agent.spawn, skill.spawn, tasklist, plan) are all `InGuest`, so they are
  never skipped — the virtual gate is untouched in practice; plugin `Wasi` caps
  (net.http, fs.*) are the ones delegated.

### Why `cfg(target_arch = "wasm32")` (not a runtime flag or feature)

- **Precise:** it matches how the guest is compiled (`tau-wasm-guest` gates its
  whole body on `target_arch = "wasm32"`).
- **Native bundle stays gated:** `tau run --bundle` drives the *same* interpreter
  compiled for the host arch, so it keeps the full in-guest check — correct,
  because on native there is no ABI/WasiCtx enforcement of `Wasi` caps at that
  layer (the OS sandbox is the gate, and defense-in-depth is desirable).
- **Host embedder unaffected:** `tau-wasm-host` is native arch.

### 3.4 ↔ 4.5 reconciliation (mandatory)

Three distinct gates exist; "the in-guest gate" was ambiguous. Disambiguated:

| Gate | Lives in | Cap class it owns | 3.4 | 4.5 |
|---|---|---|---|---|
| Kernel **ambient** gate (`stream.rs:763`/`1460`) | tau-runtime-core | root ambient authority: `Wasi` vs `InGuest` | **drops `Wasi` on wasm**; keeps `InGuest` | untouched |
| `AttenuatedDispatcher` (`interpreter/attenuate.rs`) | tau-runtime-core | subflow/descendant **relative** attenuation (meet of ancestor envelopes) | **untouched** | **4.5 builds on it** (dynamic-region envelope + bounds counters) |
| ABI world + host `WasiCtx` | 3.2 world / 3.3 host (#536) | `Wasi` caps | **becomes sole owner on wasm** | n/a |

**No real collision.** 3.4 removes *ambient `Wasi`-cap* gating on wasm; 4.5 adds
*relative dynamic-region* gating. They touch different cap classes and different
code. The `AttenuatedDispatcher` — the D1-C attenuation surface 4.5 extends — is
explicitly out of 3.4's removal scope: it enforces `InGuest`-and-`Wasi` caps
alike *relative to an ancestor's declared envelope*, which is orthogonal to
whether the *ambient* root grant is gated in-guest. On wasm, a subflow's `Wasi`
cap is still (a) bounded by the ABI/WasiCtx ambient ceiling **and** (b)
attenuated relative to its ancestors by `AttenuatedDispatcher` — both hold after
3.4.

> Consistency check with the attenuation spec's non-goal #1 ("root agent's own
> tool calls remain ungated at runtime; build-time governance covers them"): on
> the interpreter path the root is *already* effectively ungated (empty stub
> grant → the kept `InGuest` gate only ever fail-closed-denies, never allows).
> 3.4 does not change that; it only stops the gate from denying `Wasi` caps that
> the ABI/WasiCtx will own once 3.6 routes them. `InGuest`-cap ambient gating
> becomes *load-bearing* only when 3.6 threads the real grant — tracked there.

## Testing

No new effect tooling; reuse the ABI/host machinery already on `main`.

**Unit (tau-runtime-core, wasm-cfg-aware):**
- `in_guest_gated`: `Disposition::Wasi` cap → `false` on wasm, `true` on native;
  every other disposition → `true` on both. (Compile both arms;
  `#[cfg(target_arch = "wasm32")]` unit assertions run under the guest test lane,
  native assertions under the host lane.)
- `check_capabilities_for_tool` on a wasm build: a `net.http` required cap with
  an *empty* granted set returns `None` (allowed — delegated); an `agent.spawn`
  required cap with an empty granted set returns `Some` (still denied).
- On a native build: both return `Some` (unchanged).

**DoD proof (reuse + one new):**
1. **World-text negative** — reuse 3.2's `build_wasm_world_dod` (ungranted
   `wasi:*` iface absent from the generated world). ABI half of "un-importable."
2. **Host `WasiCtx` enforcement** — reuse #536/#544
   `wasi_http_enforcement.rs` / `wasi_fs_enforcement.rs` (ungranted host/method/
   path denied at the host boundary). This is "wasm caps == `[allow]`-bounded
   set" for `Wasi` caps, offline.
3. **New — `InGuest` stays gated on wasm** — a guest/interpreter test proving an
   `InGuest` cap (agent.spawn) is still denied in-guest while a `Wasi` cap
   (net.http) is *not* denied in-guest. Proves the drop is `Wasi`-only, not
   wholesale. Placement: `tau-runtime-core` wasm test lane (or a
   `tau-ir-conformance` fixture if a full run is cleaner) — resolved in the plan.

## Open items for the plan

- Exact shape of the `check_capabilities_for_tool` filter (filter-in-place vs a
  `retain` on a borrowed slice — it currently takes `&[Capability]`; may need a
  small owned `Vec` on the wasm arm only, or an iterator adaptor to avoid
  allocation on the native hot path).
- Where the "`InGuest` stays gated on wasm" test lives (core wasm unit lane vs
  conformance fixture) and whether it needs a guest binary or can assert on
  `check_capabilities_for_tool` directly.
- Confirm no existing test asserts the *old* wasm behavior (a `Wasi` cap denied
  in-guest on wasm) that would now flip — grep the wasm test lanes during
  planning.
