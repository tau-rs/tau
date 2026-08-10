# EPIC 3.4 — Drop the in-guest wasm capability gate (Wasi caps only) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the wasm target, stop the in-guest runtime capability gate from denying `Disposition::Wasi` caps (net.http, fs.read/write) — their enforcement is owned by the generated WIT world (3.2) + host `WasiCtx` (3.3); keep gating `Disposition::InGuest` caps and leave native behavior unchanged.

**Architecture:** A single wasm-only pre-filter inside the dispatch-site wrapper `check_capabilities_for_tool` (`tau-runtime-core/src/capability.rs`) classifies each *required* cap by its 3.1 `Disposition` (via `tau_ports::target::wasi_map::map_capability`) and skips `Wasi` caps from the satisfies-scan when `cfg!(target_arch = "wasm32")`. The classification is threaded through a `is_wasm: bool` parameter so both target behaviors are unit-testable from one native `cargo test`. The pure `check_capabilities` — used by root-spawn (`run.rs:123`), tool-spec filtering (`run.rs:322`), and the 4.5 `AttenuatedDispatcher` (`attenuate.rs:85`) — is **untouched**, isolating the change to the two ambient gates (`stream.rs:768`, `:1468`).

**Tech Stack:** Rust, `no_std` (`tau-runtime-core`, `tau-ports`), `wasm32-wasip2` guest, `cargo nextest`.

**Design doc:** `docs/superpowers/specs/2026-08-09-epic-3-4-drop-in-guest-wasm-gate-design.md`

## Global Constraints

- **CARGO (CLAUDE.md):** never bare `cargo`. Always `env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`, wrapped in `timeout` (test 300s, build/check 180s, clippy 240s, fmt 30s). Prefer `cargo nextest run`. For this plan `<role>` = `impl` (and `<role>` = the reviewer's purpose for review subagents).
- **rustfmt is a SEPARATE required CI gate.** Run `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check` before every push. clippy/nextest green ≠ fmt-clean.
- **No native behavior change.** `cfg!(target_arch = "wasm32")` is `false` on native → the filter is a no-op there; the OS sandbox gate + full in-guest check stay.
- **No IR / bundle-format change. No new dependency** (`tau-ports` is already a `tau-runtime-core` dep on the `no_std`/`serde` path). **No `AmbientOpsGate`/`tau.caps` work. No effect rerouting** (that is story 3.6). **No change to `check_capabilities`, `run.rs`, `attenuate.rs`, or `stream.rs`.**
- **`Disposition` is the single source of truth** for "who owns this cap on wasm" — do not hand-enumerate cap kinds.
- Remote is `tau-rs/tau`. Merge-queue: enroll bare `gh pr merge <n> --auto` (`--squash`/`-d` rejected).

---

### Task 1: `Disposition`-classification predicate `in_guest_gated_on`

**Files:**
- Modify: `crates/tau-runtime-core/src/capability.rs` (add private fn just above `check_capabilities_for_tool` at line ~80; add unit tests in the existing `#[cfg(test)] mod tests`)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `tau_ports::target::wasi_map::{map_capability, Disposition}` (already a reachable `no_std` export); `tau_domain::Capability`.
- Produces: `fn in_guest_gated_on(required: &Capability, is_wasm: bool) -> bool` — `false` iff `is_wasm && disposition == Wasi`; `true` otherwise. Consumed by Task 2.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/tau-runtime-core/src/capability.rs` (the `cap(toml_str)` helper at line 414 is already in scope):

```rust
// -------------------- in_guest_gated_on (story 3.4) --------------------

#[test]
fn wasi_caps_are_not_in_guest_gated_on_wasm() {
    let net = cap("[cap]\nkind = \"net.http\"\nhosts = [\"api.example.com\"]\nmethods = [\"GET\"]\n");
    let fs_r = cap("[cap]\nkind = \"fs.read\"\npaths = [\"/tmp/**\"]\n");
    let fs_w = cap("[cap]\nkind = \"fs.write\"\npaths = [\"/tmp/**\"]\n");
    // On wasm the ABI (3.2) + host WasiCtx (3.3) own these → not in-guest gated.
    assert!(!in_guest_gated_on(&net, true));
    assert!(!in_guest_gated_on(&fs_r, true));
    assert!(!in_guest_gated_on(&fs_w, true));
    // On native the in-guest check stays for all dispositions.
    assert!(in_guest_gated_on(&net, false));
    assert!(in_guest_gated_on(&fs_r, false));
    assert!(in_guest_gated_on(&fs_w, false));
}

#[test]
fn in_guest_caps_stay_gated_on_every_target() {
    let spawn = cap("[cap]\nkind = \"agent.spawn\"\nallowed_kinds = [\"worker\"]\n");
    let plan = cap("[cap]\nkind = \"plan\"\nmode = \"write\"\n");
    // InGuest disposition has no ABI/host realization → gated on wasm AND native.
    assert!(in_guest_gated_on(&spawn, true));
    assert!(in_guest_gated_on(&spawn, false));
    assert!(in_guest_gated_on(&plan, true));
    assert!(in_guest_gated_on(&plan, false));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core -E 'test(in_guest_gated_on) + test(in_guest_caps_stay_gated)'`
Expected: FAIL — `cannot find function in_guest_gated_on in this scope`.

- [ ] **Step 3: Add the predicate**

Insert immediately above `check_capabilities_for_tool` (i.e. after `check_capabilities`, ~line 80) in `crates/tau-runtime-core/src/capability.rs`:

```rust
/// Whether a *required* capability is enforced by the in-guest runtime gate on
/// the given target.
///
/// On wasm, `Disposition::Wasi` capabilities (net.http, fs.read/write) are
/// enforced by the generated WIT world (EPIC 3.2 — an ungranted interface is
/// absent from the world, hence un-importable) and the host `WasiCtx`
/// (EPIC 3.3 — preopens + egress allow-list), so the in-guest software check
/// for them is redundant and — against the guest's empty stub grant — wrong.
/// Story 3.4 drops it for exactly those. Every other disposition (`InGuest`,
/// `HostMediated`, unknown future variant) has no ABI/host realization and
/// stays in-guest gated. On native (`is_wasm == false`) the OS sandbox is the
/// enforcement path and the in-guest check stays for ALL dispositions
/// (defense in depth).
///
/// `is_wasm` is threaded in (rather than read from `cfg!` here) so both target
/// behaviors are unit-testable from a single native `cargo test`. `Disposition`
/// (EPIC 3.1) is the single source of truth — do not hand-enumerate cap kinds.
fn in_guest_gated_on(required: &Capability, is_wasm: bool) -> bool {
    use tau_ports::target::wasi_map::{map_capability, Disposition};
    !(is_wasm && matches!(map_capability(required).disposition, Disposition::Wasi))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core -E 'test(in_guest_gated_on) + test(in_guest_caps_stay_gated)'`
Expected: PASS (2 tests).

Note: the fn is `dead_code` until Task 2 wires it, but the module carries a top-level `#![allow(dead_code)]` (`capability.rs:27`), so this compiles clean under `-D warnings`. Do NOT split Task 1 from Task 2 across separate commits if your reviewer runs `-D warnings` on each commit in isolation — see Task 2 Step 5 (they land together).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/capability.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(epic-3-4): add in_guest_gated_on Disposition classifier"
```

---

### Task 2: Wire the wasm skip into `check_capabilities_for_tool`

**Files:**
- Modify: `crates/tau-runtime-core/src/capability.rs` — the body of `check_capabilities_for_tool` (replace the `let missing = check_capabilities(granted, required);` line at ~109); add wrapper tests in `mod tests`.
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `in_guest_gated_on` (Task 1); the existing `capability_satisfies` (`capability.rs:43`).
- Produces: no signature change. `check_capabilities_for_tool<'a>(tool_name, granted, required) -> Option<&'a Capability>` now filters `Wasi` required caps on wasm. Behavior consumed unchanged by `stream.rs:768` / `:1468`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/tau-runtime-core/src/capability.rs`:

```rust
// -------------- check_capabilities_for_tool wasm skip (story 3.4) ---------

#[test]
fn for_tool_native_denies_ungranted_wasi_cap() {
    // Native (this test binary) keeps the full gate: net.http with an EMPTY
    // grant is denied. cfg!(wasm32) is false here.
    let granted: Vec<Capability> = alloc::vec![];
    let required = alloc::vec![cap(
        "[cap]\nkind = \"net.http\"\nhosts = [\"api.example.com\"]\nmethods = [\"GET\"]\n"
    )];
    assert!(check_capabilities_for_tool("http_get", &granted, &required).is_some());
}

#[test]
fn for_tool_still_denies_ungranted_in_guest_cap() {
    // agent.spawn is InGuest → gated on every target, empty grant → denied.
    let granted: Vec<Capability> = alloc::vec![];
    let required = alloc::vec![cap(
        "[cap]\nkind = \"agent.spawn\"\nallowed_kinds = [\"worker\"]\n"
    )];
    assert!(check_capabilities_for_tool("spawn_worker", &granted, &required).is_some());
}
```

Then add a target-behavior test that exercises the wasm arm natively by driving the
underlying scan through `in_guest_gated_on` directly (the wrapper reads `cfg!`, so a
native `cargo test` cannot flip it — assert the *composed* logic instead):

```rust
#[test]
fn wasm_arm_skips_wasi_but_denies_in_guest() {
    // Simulate the wrapper's wasm-arm scan: filter by in_guest_gated_on(_, true)
    // then run the same unsatisfied predicate the wrapper uses. Empty grant.
    let granted: Vec<Capability> = alloc::vec![];
    let net = cap("[cap]\nkind = \"net.http\"\nhosts = [\"h\"]\nmethods = [\"GET\"]\n");
    let spawn = cap("[cap]\nkind = \"agent.spawn\"\nallowed_kinds = [\"w\"]\n");
    let required = alloc::vec![net, spawn];
    let missing = required
        .iter()
        .filter(|req| in_guest_gated_on(req, true))
        .find(|req| !granted.iter().any(|g| capability_satisfies(g, req)));
    // net.http filtered out (ABI/host owns it); agent.spawn remains and is missing.
    match missing {
        Some(Capability::Agent(_)) => {}
        other => panic!("expected agent.spawn as first missing on wasm arm, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the tests to verify the wasm-arm test fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core -E 'test(for_tool_native_denies) + test(for_tool_still_denies) + test(wasm_arm_skips)'`
Expected: the two `for_tool_*` PASS immediately (native path already denies — they lock in "no native regression"); `wasm_arm_skips_wasi_but_denies_in_guest` PASS too (it calls `in_guest_gated_on` from Task 1 directly). If any fail, stop and diagnose — the classifier or `capability_satisfies` import is wrong.

> Rationale: the wrapper's own wasm behavior can only be observed on a wasm build; the composed-logic test above proves the exact filter+scan the wrapper runs, natively. The real wasm binary path is covered by the reused #536/#544 host-enforcement tests (Task 3).

- [ ] **Step 3: Wire the filter into the wrapper**

In `crates/tau-runtime-core/src/capability.rs`, replace the single line inside `check_capabilities_for_tool` (currently `let missing = check_capabilities(granted, required);`, ~line 109) with:

```rust
    // Story 3.4: on wasm, Disposition::Wasi caps are enforced by the generated
    // WIT world (3.2) + host WasiCtx (3.3), not in-guest — skip them here.
    // `cfg!` (not `#[cfg]`) so native compiles to a constant `false` and the
    // filter is a provable no-op off-wasm. The pure `check_capabilities`
    // (root-spawn, tool-spec filtering, AttenuatedDispatcher) is intentionally
    // NOT changed — only this dispatch-site wrapper drops the Wasi gate.
    let is_wasm = cfg!(target_arch = "wasm32");
    let missing = required
        .iter()
        .filter(|req| in_guest_gated_on(req, is_wasm))
        .find(|req| !granted.iter().any(|g| capability_satisfies(g, req)));
```

Leave the surrounding tracing events (`EV_CAPABILITY_REQUIRED_LOADED` etc.) unchanged — `required.len()` still reports the caps the tool declared; only the deny decision changes.

- [ ] **Step 4: Run the full crate test suite + fmt + clippy**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets
```
Expected: all PASS. In particular the pre-existing `check_capabilities_*` tests are untouched (they call the pure fn), and the two `for_tool_native_*` tests confirm native denials still fire.

- [ ] **Step 5: Grep for any test asserting the OLD wasm behavior, then commit**

```bash
git grep -n "check_capabilities_for_tool" -- crates/ | grep -i "wasm\|target_arch"
```
Expected: no match (no existing test pins a `Wasi`-cap-denied-on-wasm behavior that would now flip). If a match exists, update it to expect the new delegation and note it in the commit body.

```bash
git add crates/tau-runtime-core/src/capability.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(epic-3-4): drop in-guest gate for Wasi caps on wasm

check_capabilities_for_tool (the two ambient dispatch gates,
stream.rs:768/:1468) skips Disposition::Wasi required caps on wasm —
enforcement is the generated WIT world (3.2) + host WasiCtx (3.3).
InGuest caps stay gated on every target; native unchanged. The pure
check_capabilities (root-spawn, tool-spec filter, AttenuatedDispatcher/
4.5) is untouched."
```

---

### Task 3: DoD evidence — wasm guest build + reused enforcement tests

**Files:**
- No production code. Verification-only task: build the guest for wasm and run the existing host-enforcement + world-text DoD tests to confirm nothing regressed and the 3.4 delegation holds end-to-end.

**Interfaces:**
- Consumes: 3.2 `build_wasm_world_dod`, #536/#544 `wasi_http_enforcement.rs` / `wasi_fs_enforcement.rs`.
- Produces: green evidence for the PR body (world-text negative + host-boundary denial = "wasm caps == `[allow]`-bounded set" for `Wasi`; Task 2's unit tests = "InGuest stays gated").

- [ ] **Step 1: Confirm the wasm guest still compiles against the shared core change**

Run (matches the guest's target + features):
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-wasm-guest --target wasm32-wasip2
```
Expected: PASS. This proves the `cfg!(target_arch = "wasm32")` arm + the `tau_ports::target::wasi_map` import compile in the `no_std` wasm build (the one path native `cargo test` does not cover).

- [ ] **Step 2: Run the host-side WASI enforcement tests (the runtime DoD for `Wasi` caps)**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-wasm-host --run-ignored all -E 'test(egress_is_denied) + test(fs)'
```
Expected: PASS — ungranted host/method (and fs path) denied at the host boundary, offline. If the fixture test names differ, list them first with `--run-ignored all --list` and adjust the filter.

- [ ] **Step 3: Run the 3.2 world-text DoD test (the ABI negative)**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli -E 'test(build_wasm_world_dod)'
```
Expected: PASS — ungranted `wasi:*` interface absent from the generated world. (If this test lives in a different crate, `git grep -l build_wasm_world_dod` and target that crate.)

- [ ] **Step 4: No commit (verification only).** Record the three green results in the PR body under a "DoD evidence" heading, mapping each to the design doc's Testing section (1 world-text, 2 host-WasiCtx, 3 InGuest-gated unit tests from Task 2).

---

## Self-Review

**1. Spec coverage:**
- "drop in-guest check for `Wasi` caps on wasm" → Task 2 (wrapper filter). ✓
- "keep `InGuest` gated" → Task 1 `in_guest_caps_stay_gated_on_every_target` + Task 2 `for_tool_still_denies_ungranted_in_guest_cap`. ✓
- "`Disposition` = single source of truth" → `in_guest_gated_on` reads `map_capability().disposition`. ✓
- "native unchanged" → `cfg!` → `false`; Task 2 Step 4 runs full suite; `for_tool_native_denies` locks it. ✓
- "`AttenuatedDispatcher`/pure `check_capabilities` untouched" → change is wrapper-only; Task 2 Step 3 comment + Step 5 grep. ✓
- "no effect rerouting / no `tau.caps` / no grant-threading / no IR change" → not in any task (deferred to 3.6). ✓
- DoD proof (world-text, host-WasiCtx, InGuest-gated) → Task 3 + Task 2. ✓
- 3.4↔4.5 reconciliation → design doc (shipped in 3.4's earlier docs commit `1bb99e6c`); no code task needed. ✓

**2. Placeholder scan:** No TBD/TODO; every code step has real code; test bodies are concrete. ✓

**3. Type consistency:** `in_guest_gated_on(&Capability, bool) -> bool` used identically in Tasks 1 & 2; `check_capabilities_for_tool` signature unchanged; `cap(&str) -> Capability` and `capability_satisfies` match the existing module. ✓

**Known deviation from the spec sketch (intentional refinement):** the spec showed `#[cfg(target_arch = "wasm32")]` fn arms; the plan uses a single `is_wasm: bool`-threaded predicate + `cfg!(...)` macro instead — behaviorally identical, but unit-testable for *both* targets from one native run (resolves spec Open-item #1). Alloc-free (iterator filter preserves the `&'a Capability` borrow), resolving the "owned Vec vs retain" open item.
