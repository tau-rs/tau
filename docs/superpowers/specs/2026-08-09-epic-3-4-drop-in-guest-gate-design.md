# EPIC 3.4 — Drop the in-guest net-egress gate on wasm

**Status:** Design approved, pre-implementation
**Date:** 2026-08-09
**Roadmap line:** 3.4 — *"Drop the in-guest gate on wasm; OS gate stays for host/native."*
**Epic DoD (EPIC 3):** an ungranted cap is un-importable at the ABI; wasm caps == `[allow]`-bounded set.

## Problem

Network egress on the wasm profile is currently checked **twice**:

1. **In-guest**, by `tau-runtime-core`'s per-tool capability check
   `check_capabilities_for_tool` → `net_satisfies`
   (`crates/tau-runtime-core/src/capability.rs:95` / `:184`), invoked at the
   two tool-dispatch sites `crates/tau-runtime-core/src/stream.rs:767`
   (virtual/orchestration tools) and `:1467` (plugin/native tools). Because the
   wasm guest links `tau-runtime-core` (`no_std`) and drives `run_ir_streaming`,
   this check runs **inside the guest**.
2. **Host-side**, by the EPIC 3.3 `EgressPolicy::permits`
   (`crates/tau-wasm-host/src/wasi.rs:33`) + `WasiHttpHooks::send_request`
   (`crates/tau-wasm-host/src/lib.rs:176-193`), which reject an unauthorized
   host/method on the `wasi:http` path — offline, before any socket is opened.

The host `EgressPolicy` is configured (EPIC 3.3, PR #536) from **the same
allow-bounded caps** that bound the agent's grant. The in-guest net check is
therefore redundant with the host gate at runtime, and redundant with the
build-time capability lattice (EPIC 1.3/1.5) statically. `tau-ports`'
capability→WASI mapping already encodes the intended disposition:
`crates/tau-ports/src/target/wasi_map.rs` classifies network as
`Disposition::Wasi` (bounded by a WASI import + host config), while
`Disposition::InGuest` is *only* tasklist/plan/agent.spawn/skill.spawn. 3.4
makes that table honest for net egress.

## Goal

On the wasm profile, skip the in-guest per-tool **net-egress** check, leaving
the host `EgressPolicy` as the sole net-egress gate. Native/OS-gated runs are
unchanged (the OS sandbox stays the enforcement point there, per the roadmap
line). No behavioural change to any non-`Network` capability, on any target.

## Non-goals

- **No new guest network tool.** `tau-native-tools` has no fetch/http tool
  today (only `read_temp` / `set_fan`, both deterministic). Adding one — and
  thereby making the `wasi:http` import binary-observable ("load-bearing") — is
  EPIC 5 scope and would overlap open PR #543. 3.4 is a *removal*, not a new
  capability.
- **No host-side change.** `tau-wasm-host` already enforces; it is untouched.
- **No change to the agent/subflow lattice.** `run.rs:123` (agent-start
  `check_capabilities`) and `interpreter/attenuate.rs:85` (subflow attenuation)
  are the agent/subflow capability lattice, not per-tool egress dispatch. They
  stay.

## Design

### Mechanism: a shell-set flag threaded through the dispatcher

The interpreter (`tau-runtime-core`) is target-agnostic at dispatch time.
`RunOptions` (`crates/tau-runtime-core/src/options.rs:58`,
`#[non_exhaustive]`) already carries shell-specific facts injected by the host
shell via the `ToolDispatcher` trait (clock, random, checkpointing, context
registry — see `prepare_agent_run`, `crates/tau-runtime-core/src/interpreter/agent_loop.rs:427`).
The egress signal rides the same rail.

**1. `RunOptions` flag** — `options.rs`:

```rust
/// True when the host shell enforces network egress at the WASI/host
/// boundary (wasm: wasi:http + EgressPolicy, built from the same
/// allow-bounded caps). The in-guest per-tool net check is then
/// redundant and is skipped, leaving the host as the sole net-egress
/// gate. Native/OS-gated shells leave this false. (EPIC 3.4)
pub egress_host_mediated: bool,   // Default: false
```

**2. Dispatcher seam** — the `ToolDispatcher` trait
(`crates/tau-runtime-core/src/interpreter/tool_dispatch.rs`):

```rust
/// EPIC 3.4: the shell enforces net egress at the host boundary, so the
/// in-guest per-tool net check is skipped. Default false (OS-gated / native).
fn egress_host_mediated(&self) -> bool { false }
```

Populated in `prepare_agent_run` (`agent_loop.rs`, alongside the existing
`dispatcher.clock()` / `.random()` / `.checkpointing()` wiring):

```rust
run_options.egress_host_mediated = dispatcher.egress_host_mediated();
```

**3. Guest override** — `crates/tau-wasm-guest/src/dispatcher.rs`,
`GuestDispatcher`:

```rust
fn egress_host_mediated(&self) -> bool { true }
```

The native `ForwardingDispatcher` (`tau-cli`) uses the trait default `false`.

### The skip + delegated event

A single helper in `capability.rs`, called at both dispatch sites in place of
the bare `check_capabilities_for_tool`:

```rust
/// EPIC 3.4: when `egress_host_mediated`, network capabilities are
/// enforced at the host/WASI boundary, not in-guest. Partition `required`
/// into net vs non-net; emit `capability.egress_delegated` for each net
/// cap; run the existing satisfies-check on the non-net remainder only.
/// When `!egress_host_mediated`, this is exactly `check_capabilities_for_tool`.
pub fn check_capabilities_for_tool_delegating<'a>(
    tool_name: &str,
    granted: &[Capability],
    required: &'a [Capability],
    egress_host_mediated: bool,
) -> Option<&'a Capability>;
```

- `stream.rs:1467` (plugin/native tools, `required = tool.capabilities()`) can
  carry `Capability::Network` → this is where the skip matters.
- `stream.rs:767` (virtual/orchestration tools,
  `required = orchestration::required_capability(..)`) never carries a
  `Network` cap, so the helper is a no-op there; applied for symmetry and to
  keep a single gate entry point.

`net_satisfies` and `check_capabilities_for_tool` **stay** — they remain the
enforcement path on native (flag off) and are reachable at build time.

### New vocabulary event

`crates/tau-runtime-core/src/vocabulary.rs`:

```rust
pub const EV_CAPABILITY_EGRESS_DELEGATED: &str = "capability.egress_delegated";
```

Emitted in place of `capability.allow` / `capability.deny` for a net cap that
was delegated to the host. Rationale: the in-guest trace stays honest — it
records that this run did **not** decide net egress in-guest; the host boundary
did. A silent skip would hide exactly where a security-relevant decision moved;
reusing `capability.allow` would falsely assert an in-guest allow decision that
never happened.

## Soundness

Skipping the in-guest net check on wasm does not weaken enforcement:

- **Runtime:** the host `EgressPolicy` is configured from the same
  allow-bounded caps (EPIC 3.3 / #536). An agent with no net grant yields
  `allowed_hosts = Exact({})` (deny-all); any actual request is rejected at the
  host `send_request` before a socket opens. An agent with a bounded grant
  rejects any authority/method outside it, at the socket.
- **Build time:** the tool-declares-vs-agent-grants net lattice is also checked
  by the EPIC 1.3/1.5 capability subset law at build. The dispatch-time
  in-guest net check is thus redundant with both the host runtime gate and the
  build gate.

Note the two checks compare different things: the in-guest check is *static*
(tool's declared cap ⊆ agent's grant); the host check is *dynamic* (the actual
wire authority+method ⊆ agent's grant). Dropping the static in-guest net check
loses no enforcement because the host configured from the same grant rejects
the dynamic request, and the static relation is still enforced at build.

## Interaction with EPIC 4.5 (roadmap-mandated cross-reference)

The roadmap flags a mandatory 3.4↔4.5 cross-reference: 3.4 *removes* an
in-guest wasm gate while 4.5 *adds* a runtime gate for dynamic regions; on wasm
they interact.

**Resolution:** 3.4 drops the in-guest check **only for `Capability::Network`**
(a `Disposition::Wasi` cap). 4.5's runtime gate targets `Disposition::InGuest`
dynamic-region caps (agent.spawn membership/attenuation/bounds). These sets are
disjoint. All `Disposition::InGuest` caps keep their in-guest gate on every
target after 3.4, so 4.5 builds on an unchanged surface. The
`capability.egress_delegated` event makes the `Wasi`-vs-`InGuest` split
observable in the trace, which is the natural anchor for 4.5's own decision
events. The D1-C attenuation handoff touches `attenuate.rs`, which 3.4 does not
modify.

## Dependency on PR #543 (EPIC 3.2 load-bearing WIT world)

Independent. #543 changes guest WIT compilation
(`tau-wasm-guest/build.rs`, `wit-gen/`, `cmd_build_wasm.rs`); 3.4 changes
runtime-core gate logic and the guest dispatcher. No shared files. 3.4 does not
block on #543 merging.

## Test plan

Core proof is hermetic — no wasm build required.

- **runtime-core (unit):** `check_capabilities_for_tool_delegating` — a
  `Network` cap not covered by the grant is *delegated and skipped* (returns
  `None`, emits `capability.egress_delegated`) when `egress_host_mediated`;
  *denied* (returns the missing cap) when not. Non-`Network` caps behave
  identically regardless of the flag.
- **runtime-core (integration, streaming):** drive the interpreter with a
  dispatcher whose `egress_host_mediated()` is `true`, an agent whose grant
  does **not** cover a tool's `net.http` requirement, and assert **no
  `capability.deny`**, the tool dispatches, and `capability.egress_delegated`
  is emitted. Flip the flag off → `capability.deny` (native-parity regression
  guard).
- **guest:** `GuestDispatcher::egress_host_mediated()` returns `true`. Note:
  `tau-wasm-guest` is wasm32-only (empty on the host target), so this is
  compile-verified on `wasm32-wasip2`, not host-unit-tested; the flag's
  behavioural effect is proven host-side by the runtime-core integration test
  above. Explicit, accepted test gap for a wasm-only one-liner.
- **vocabulary drift:** add `capability.egress_delegated` to
  `crates/tau-runtime-tokio/tests/vocabulary_drift.rs`.
- **host enforcement unchanged:** `crates/tau-wasm-host/tests/wasi_http_enforcement.rs`
  stays green — the proof that the host actually rejects an unauthorized
  host/method at the socket (`#[ignore]`, wasm lane).
- **docs:** a short note that the wasm profile delegates net egress to the host
  boundary (target page chosen during plan writing — the capability-enforcement
  / sandbox explanation page).

## Files touched

| File | Change |
|---|---|
| `crates/tau-runtime-core/src/options.rs` | add `egress_host_mediated: bool` (default false) |
| `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` | add `ToolDispatcher::egress_host_mediated()` default-false method |
| `crates/tau-runtime-core/src/interpreter/agent_loop.rs` | set `run_options.egress_host_mediated` from dispatcher in `prepare_agent_run` |
| `crates/tau-runtime-core/src/capability.rs` | add `check_capabilities_for_tool_delegating` helper |
| `crates/tau-runtime-core/src/stream.rs` | call the delegating helper at `:767` and `:1467` |
| `crates/tau-runtime-core/src/vocabulary.rs` | add `EV_CAPABILITY_EGRESS_DELEGATED` |
| `crates/tau-wasm-guest/src/dispatcher.rs` | `GuestDispatcher::egress_host_mediated() → true` |
| `crates/tau-runtime-tokio/tests/vocabulary_drift.rs` | assert new event in the vocab set |
| docs (page TBD) | note wasm delegates net egress to the host |

## Escape-hatch / ADR check

- No `Custom` / `InternalError` escape hatch added, modified, or removed.
- No ADR required (implements the existing EPIC 3.4 roadmap story; no new
  cross-cutting architectural decision — the disposition split it enforces is
  already ADR-anchored in the 3.1 mapping table).
