# EPIC 3.4 — Drop the in-guest net-egress gate on wasm — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the wasm profile, skip the in-guest per-tool network-egress capability check so the host `EgressPolicy` (EPIC 3.3) is the sole net-egress gate; native/OS-gated runs are unchanged.

**Architecture:** A shell-set `egress_host_mediated` flag rides the existing `ToolDispatcher` → `RunOptions` rail (the same rail as clock/random/checkpointing). The wasm `GuestDispatcher` sets it `true`; native `ForwardingDispatcher` uses the default `false`. At the two tool-dispatch sites in `stream.rs`, a new `capability.rs` helper delegates `Capability::Network` caps to the host (emitting a `capability.egress_delegated` trace event) and checks all other caps in-guest as before.

**Tech Stack:** Rust (`no_std + alloc` kernel `tau-runtime-core`; std host `tau-runtime-tokio`; wasm guest `tau-wasm-guest`), `tracing`, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-09-epic-3-4-drop-in-guest-gate-design.md`

## Global Constraints

- **CARGO discipline (CLAUDE.md).** Every cargo command: `timeout <sec> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo <cmd> -p <crate>`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Prefer `cargo nextest run`. Before every push run `cargo fmt -p <crate> --check` (separate required rustfmt gate). If another cargo process shares `target/agent-e34`, use `target/agent-e34-2`.
- **Scope fence.** Skip applies **only** to `Capability::Network`. Do NOT touch `run.rs:123` (agent-start lattice) or `interpreter/attenuate.rs:85` (subflow attenuation), or any non-`Network` cap. Do NOT modify `tau-wasm-host` (already the enforcer). Do NOT add a guest network tool.
- **Vocabulary is a cross-crate mirror.** A new event must be added to BOTH `tau-observe::vocabulary` AND `tau-runtime-core::vocabulary` (+ its `PAIRS`), and registered in `tau-runtime-tokio/tests/vocabulary_drift.rs` (`lookup_observe` arm, `IDENTS`, and `OBSERVE_TOTAL_EXPECTED`), or the drift test fails.
- **Conventional commits**, imperative, scoped `feat(epic-3-4): …`. Commit with explicit identity to avoid the lefthook identity-corruption trap: `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "…"`. Rust changes may keep the pre-commit hook; docs-only commits may use `--no-verify`.

---

### Task 1: Register the `capability.egress_delegated` vocabulary event

Cross-crate registration only — no behaviour yet. The delegating helper (Task 2) emits this event, so it must exist and be drift-clean first.

**Files:**
- Modify: `crates/tau-observe/src/vocabulary.rs` (add const near `EV_CAPABILITY_DENY:70`)
- Modify: `crates/tau-runtime-core/src/vocabulary.rs` (add const near `EV_CAPABILITY_DENY:77`; add `PAIRS` entry near `("EV_CAPABILITY_DENY", …)`)
- Test: `crates/tau-runtime-tokio/tests/vocabulary_drift.rs` (add `lookup_observe` arm, `IDENTS` entry, bump `OBSERVE_TOTAL_EXPECTED` 38→39)

**Interfaces:**
- Produces: `tau_runtime_core::vocabulary::EV_CAPABILITY_EGRESS_DELEGATED: &str = "capability.egress_delegated"` and the identical `tau_observe::vocabulary::EV_CAPABILITY_EGRESS_DELEGATED`.

- [ ] **Step 1: Add the const to `tau-observe`**

In `crates/tau-observe/src/vocabulary.rs`, after the `EV_CAPABILITY_DENY` line:

```rust
/// Event: a tool's network capability was delegated to the host/WASI
/// egress boundary (wasm profile) instead of being checked in-guest.
pub const EV_CAPABILITY_EGRESS_DELEGATED: &str = "capability.egress_delegated";
```

If that file has a literal-equality test listing (`assert_eq!(EV_CAPABILITY_DENY, "capability.deny")` near line 159), add:

```rust
    assert_eq!(EV_CAPABILITY_EGRESS_DELEGATED, "capability.egress_delegated");
```

- [ ] **Step 2: Add the const + `PAIRS` entry to `tau-runtime-core`**

In `crates/tau-runtime-core/src/vocabulary.rs`, after `EV_CAPABILITY_DENY:77`:

```rust
/// Event: a tool's network capability was delegated to the host/WASI
/// egress boundary (wasm profile) instead of being checked in-guest.
/// Emitted by the in-guest dispatch gate when `egress_host_mediated`.
pub const EV_CAPABILITY_EGRESS_DELEGATED: &str = "capability.egress_delegated";
```

And in the `PAIRS` array, immediately after the `("EV_CAPABILITY_DENY", EV_CAPABILITY_DENY),` entry:

```rust
    (
        "EV_CAPABILITY_EGRESS_DELEGATED",
        EV_CAPABILITY_EGRESS_DELEGATED,
    ),
```

- [ ] **Step 3: Update the drift test**

In `crates/tau-runtime-tokio/tests/vocabulary_drift.rs`:
- In `lookup_observe`, after the `"EV_CAPABILITY_DENY" => o::EV_CAPABILITY_DENY,` arm:

```rust
        "EV_CAPABILITY_EGRESS_DELEGATED" => o::EV_CAPABILITY_EGRESS_DELEGATED,
```

- In the `IDENTS` array, after `"EV_CAPABILITY_DENY",`:

```rust
        "EV_CAPABILITY_EGRESS_DELEGATED",
```

- Bump the total:

```rust
const OBSERVE_TOTAL_EXPECTED: usize = 39;
```

- [ ] **Step 4: Run the drift test — expect PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-tokio vocabulary_drift`
Expected: PASS (all three drift tests green). If `total_observe_count_matches` fails, the const was added on only one side or the bump/`IDENTS`/arm is out of sync.

- [ ] **Step 5: Verify core vocab literal test still passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-core vocabulary`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-e34 cargo fmt -p tau-observe --check
timeout 30 env CARGO_TARGET_DIR=target/agent-e34 cargo fmt -p tau-runtime-core --check
git add crates/tau-observe/src/vocabulary.rs crates/tau-runtime-core/src/vocabulary.rs crates/tau-runtime-tokio/tests/vocabulary_drift.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-4): add capability.egress_delegated vocabulary event"
```

---

### Task 2: Add the delegating capability helper

Pure logic in the kernel, unit-tested directly by return value. No call sites yet.

**Files:**
- Modify: `crates/tau-runtime-core/src/capability.rs` (add `check_capabilities_for_tool_delegating` after `check_capabilities_for_tool:132`; add a test in the `mod tests` at `:401`)

**Interfaces:**
- Consumes: `EV_CAPABILITY_EGRESS_DELEGATED` (Task 1); existing `check_capabilities_for_tool`, `capability_satisfies`, `capability_kind_str`.
- Produces:
  ```rust
  pub fn check_capabilities_for_tool_delegating<'a>(
      tool_name: &str,
      granted: &[Capability],
      required: &'a [Capability],
      egress_host_mediated: bool,
  ) -> Option<&'a Capability>
  ```
  Semantics: when `!egress_host_mediated`, identical to `check_capabilities_for_tool`. When `egress_host_mediated`, each `Capability::Network` in `required` is delegated (emits `capability.egress_delegated`, not checked in-guest); all other caps are checked as before; returns the first missing non-net cap, or `None`.

- [ ] **Step 1: Write the failing test**

In `crates/tau-runtime-core/src/capability.rs`, inside `mod tests` (near the Network section at `:519`), add. `cap(&str)` is the existing TOML→`Capability` test helper used throughout this module:

```rust
    // -------------------- EPIC 3.4: host-mediated egress --------------------

    #[test]
    fn net_cap_beyond_grant_is_denied_when_not_host_mediated() {
        let granted = [cap(r#"[cap]
kind = "net.http"
hosts = ["allowed.example"]
methods = ["GET"]
"#)];
        let required = [cap(r#"[cap]
kind = "net.http"
hosts = ["blocked.invalid"]
methods = ["GET"]
"#)];
        // flag off = native/OS-gated: in-guest check still enforces.
        let missing = check_capabilities_for_tool_delegating(
            "fetch", &granted, &required, false,
        );
        assert!(missing.is_some(), "native must still deny an out-of-grant net cap");
    }

    #[test]
    fn net_cap_beyond_grant_is_delegated_when_host_mediated() {
        let granted = [cap(r#"[cap]
kind = "net.http"
hosts = ["allowed.example"]
methods = ["GET"]
"#)];
        let required = [cap(r#"[cap]
kind = "net.http"
hosts = ["blocked.invalid"]
methods = ["GET"]
"#)];
        // flag on = wasm/host-mediated: net cap delegated, not checked in-guest.
        let missing = check_capabilities_for_tool_delegating(
            "fetch", &granted, &required, true,
        );
        assert!(missing.is_none(), "host-mediated egress must not deny in-guest");
    }

    #[test]
    fn non_net_cap_beyond_grant_is_denied_even_when_host_mediated() {
        let granted: [Capability; 0] = [];
        let required = [cap(r#"[cap]
kind = "fs.read"
paths = ["/etc"]
"#)];
        // fs is not delegated: still checked in-guest regardless of the flag.
        let missing = check_capabilities_for_tool_delegating(
            "reader", &granted, &required, true,
        );
        assert!(missing.is_some(), "non-net caps are never delegated");
    }
```

- [ ] **Step 2: Run tests — expect FAIL (unresolved name)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-core delegating`
Expected: FAIL / compile error `cannot find function check_capabilities_for_tool_delegating`.

- [ ] **Step 3: Implement the helper**

In `crates/tau-runtime-core/src/capability.rs`, after `check_capabilities_for_tool` (ends `:132`):

```rust
/// EPIC 3.4: dispatch-site capability gate that delegates network egress
/// to the host/WASI boundary when the shell enforces it there.
///
/// When `!egress_host_mediated` (native / OS-gated shells) this is exactly
/// [`check_capabilities_for_tool`]. When `egress_host_mediated` (wasm: the
/// host `EgressPolicy` is configured from the same allow-bounded caps and
/// rejects at the socket), each `Capability::Network` in `required` is
/// delegated — recorded via `capability.egress_delegated` and NOT checked
/// in-guest — while every other capability is still checked. Returns the
/// first missing non-delegated capability, or `None` if all are covered.
pub fn check_capabilities_for_tool_delegating<'a>(
    tool_name: &str,
    granted: &[Capability],
    required: &'a [Capability],
    egress_host_mediated: bool,
) -> Option<&'a Capability> {
    use crate::vocabulary as v;
    if !egress_host_mediated {
        return check_capabilities_for_tool(tool_name, granted, required);
    }
    tracing::debug!(
        name = v::EV_CAPABILITY_REQUIRED_LOADED,
        required_count = required.len(),
    );
    tracing::debug!(
        name = v::EV_CAPABILITY_GRANTED_LOADED,
        granted_count = granted.len(),
    );
    for req in required {
        if matches!(req, Capability::Network(_)) {
            tracing::info!(
                name = v::EV_CAPABILITY_EGRESS_DELEGATED,
                tool_name = %tool_name,
                delegated_kind = %capability_kind_str(req),
            );
            continue;
        }
        if !granted.iter().any(|g| capability_satisfies(g, req)) {
            let kind = capability_kind_str(req);
            tracing::warn!(
                name = v::EV_CAPABILITY_DENY,
                tool_name = %tool_name,
                missing_kind = %kind,
            );
            return Some(req);
        }
    }
    tracing::info!(
        name = v::EV_CAPABILITY_ALLOW,
        tool_name = %tool_name,
    );
    None
}
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-core delegating`
Expected: PASS (3/3).

- [ ] **Step 5: Run the full capability suite (regression)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-core capability`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-e34 cargo fmt -p tau-runtime-core --check
git add crates/tau-runtime-core/src/capability.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-4): add host-mediated egress-delegating capability helper"
```

---

### Task 3: Thread `egress_host_mediated` through options → dispatcher → dispatch sites

Wires the flag and switches both `stream.rs` gate call sites to the delegating helper. Integration-tested end-to-end on the tokio host.

**Files:**
- Modify: `crates/tau-runtime-core/src/options.rs` (add field to `RunOptions:58`; set default in the `Default` impl)
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` (add trait method after the trait open at `:54`)
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs` (set the option in `prepare_agent_run`, near the clock/random wiring at `:562-567`)
- Modify: `crates/tau-runtime-core/src/stream.rs` (call sites at `:767` and `:1467`)
- Test: `crates/tau-runtime-tokio/tests/egress_delegation.rs` (new)

**Interfaces:**
- Consumes: `check_capabilities_for_tool_delegating` (Task 2); `EV_CAPABILITY_EGRESS_DELEGATED` (Task 1).
- Produces: `RunOptions.egress_host_mediated: bool`; `ToolDispatcher::egress_host_mediated(&self) -> bool` (default `false`).

- [ ] **Step 1: Add the `RunOptions` field**

In `crates/tau-runtime-core/src/options.rs`, add to the `RunOptions` struct (place after an existing `bool`/simple field; the struct is `#[non_exhaustive]`):

```rust
    /// EPIC 3.4: true when the host shell enforces network egress at the
    /// WASI/host boundary (wasm: wasi:http + `EgressPolicy`, built from the
    /// same allow-bounded caps). The in-guest per-tool net check is then
    /// redundant and is skipped, leaving the host as the sole net-egress
    /// gate. Native/OS-gated shells leave this false. Default: false.
    pub egress_host_mediated: bool,
```

In the `impl Default for RunOptions` block, add:

```rust
            egress_host_mediated: false,
```

- [ ] **Step 2: Verify options still build**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo build -p tau-runtime-core`
Expected: PASS (no missing-field error in `Default`).

- [ ] **Step 3: Add the `ToolDispatcher` trait method**

In `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs`, inside `pub trait ToolDispatcher` (opens at `:54`), add a defaulted method:

```rust
    /// EPIC 3.4: true when this shell enforces network egress at the host
    /// boundary (wasm: wasi:http + `EgressPolicy`), so the in-guest per-tool
    /// net check is skipped. Default false (native / OS-gated shells).
    fn egress_host_mediated(&self) -> bool {
        false
    }
```

- [ ] **Step 4: Populate the option in `prepare_agent_run`**

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs`, after the clock/random wiring (`:562-567`), add:

```rust
    // EPIC 3.4: propagate the shell's egress-enforcement disposition.
    run_options.egress_host_mediated = dispatcher.egress_host_mediated();
```

- [ ] **Step 5: Switch both dispatch call sites to the delegating helper**

In `crates/tau-runtime-core/src/stream.rs`, at the virtual-tool site (`:767-773`), change the call from `check_capabilities_for_tool(...)` to:

```rust
                        let missing = dispatch_span.in_scope(|| {
                            crate::capability::check_capabilities_for_tool_delegating(
                                &tool_use.name,
                                &granted_capabilities,
                                required_slice,
                                options.egress_host_mediated,
                            )
                        });
```

And at the plugin/native-tool site (`:1467-1473`):

```rust
                let missing = dispatch_span.in_scope(|| {
                    crate::capability::check_capabilities_for_tool_delegating(
                        &tool_use.name,
                        &granted_capabilities,
                        required,
                        options.egress_host_mediated,
                    )
                });
```

(`options` is the `run_streaming_inner` parameter already in scope at both sites.)

- [ ] **Step 6: Build the kernel**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo build -p tau-runtime-core`
Expected: PASS.

- [ ] **Step 7: Write the failing integration test**

Create `crates/tau-runtime-tokio/tests/egress_delegation.rs`. Model the harness on the existing `crates/tau-runtime-tokio/tests/run_capability_denied.rs` (for building a run whose tool requires a net cap the agent does not grant, and asserting the `RunEvent` outcome) and `crates/tau-runtime-tokio/tests/tracing_emission.rs` (for capturing tracing events). Read both before writing. The two assertions:

```rust
// 1. Behavioural: with a dispatcher whose egress_host_mediated() == true,
//    a tool requiring net.http beyond the grant is NOT policy-denied — the
//    run proceeds past the capability gate (host would reject at the socket).
// 2. Observability: the run emits a `capability.egress_delegated` event and
//    NO `capability.deny` for that tool.
// 3. Parity: the same setup with egress_host_mediated() == false yields a
//    PolicyDenied outcome (RunEvent) and a `capability.deny` event.
```

Write concrete `#[tokio::test]` functions following those two precedents' exact subscriber-capture and run-drive scaffolding (a custom `ToolDispatcher` wrapper that overrides `egress_host_mediated()`; assert on the collected `RunEvent`s and captured event names).

- [ ] **Step 8: Run the integration test — expect FAIL first, then implement/adjust until PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-tokio egress_delegation`
Expected: FAIL until the wrapper + assertions compile and the wiring (Steps 1-5) makes them pass. Iterate to green.

- [ ] **Step 9: Regression — capability-denied path still denies on native**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-runtime-tokio run_capability_denied`
Expected: PASS (default flag = false is unchanged native behaviour).

- [ ] **Step 10: Commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-e34 cargo fmt -p tau-runtime-core --check
timeout 30 env CARGO_TARGET_DIR=target/agent-e34 cargo fmt -p tau-runtime-tokio --check
git add crates/tau-runtime-core/src/options.rs crates/tau-runtime-core/src/interpreter/tool_dispatch.rs crates/tau-runtime-core/src/interpreter/agent_loop.rs crates/tau-runtime-core/src/stream.rs crates/tau-runtime-tokio/tests/egress_delegation.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-4): delegate net egress to host on egress_host_mediated runs"
```

---

### Task 4: Set the flag `true` in the wasm guest dispatcher

**Testability note (read first):** `tau-wasm-guest` is **wasm32-only** — its
`Cargo.toml` gates the entire dependency graph under
`[target.'cfg(target_arch = "wasm32")'.dependencies]` and every module is
`#[cfg(target_arch = "wasm32")]`; on the host target the crate is empty, and
`tau-runtime-core` is not even a dependency there. So `GuestDispatcher` cannot
carry a host-run unit test. The behavioural effect of the flag (delegation) is
already proven host-side by Task 3's integration test (a stub dispatcher with
`egress_host_mediated() == true`). This task is the trivial one-line structural
override; it is verified by a **wasm-target compile**, not a host unit test.
This is an explicit, accepted test gap for a wasm-only one-liner.

**Files:**
- Modify: `crates/tau-wasm-guest/src/dispatcher.rs` (`impl ToolDispatcher for GuestDispatcher` at `:41`)

**Interfaces:**
- Consumes: `ToolDispatcher::egress_host_mediated` (Task 3).

- [ ] **Step 1: Override the method**

In `crates/tau-wasm-guest/src/dispatcher.rs`, inside `impl ToolDispatcher for GuestDispatcher` (`:41`), add:

```rust
    fn egress_host_mediated(&self) -> bool {
        // wasm: net egress goes through wasi:http, gated host-side by
        // EgressPolicy (EPIC 3.3). The in-guest net check is redundant.
        true
    }
```

- [ ] **Step 2: Verify the guest compiles for the wasm target**

Ensure the target is installed (`rustup target add wasm32-wasip2`), then:

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo check -p tau-wasm-guest --target wasm32-wasip2`
Expected: PASS. If `wasm32-wasip2` is not installed and cannot be added, note it and rely on CI's wasm lane to compile the guest.

- [ ] **Step 3: Commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-e34 cargo fmt -p tau-wasm-guest --check
git add crates/tau-wasm-guest/src/dispatcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(epic-3-4): GuestDispatcher reports host-mediated egress"
```

---

### Task 5: Document the wasm egress delegation + confirm host enforcement test

**Files:**
- Modify: `docs/explanation/capabilities-and-consent.md` (§"Enforcement: where the kernel stands", `:207`)

**Interfaces:** none (docs).

- [ ] **Step 1: Add the note**

In `docs/explanation/capabilities-and-consent.md`, within/after the "Enforcement: where the kernel stands" section (`:207`), add a short paragraph:

```markdown
### wasm profile: egress is enforced at the host boundary

On the wasm target the runtime does **not** re-check network egress
in-guest. A tool's `net.http` capability is enforced by the host: the
embedder's `EgressPolicy` (built from the same allow-bounded capability
set) rejects an unauthorized host or method on the `wasi:http` path,
before any socket opens. The in-guest dispatch gate records this handoff
with a `capability.egress_delegated` trace event and defers — the host is
the sole net-egress gate. Non-network capabilities (filesystem, process,
agent spawn, task-list, plan, skill) are still checked in-guest on every
target. (EPIC 3.4)
```

- [ ] **Step 2: Build the book (docs gate)**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines, no linkcheck errors. Then `rm -rf docs/book`.

- [ ] **Step 3: Confirm the host enforcement test is unaffected (proof host is sole gate)**

This test proves the host actually rejects at the socket; it must remain green (it is `#[ignore]`, wasm lane, needs `wasm32-wasip2`). If the toolchain is present:

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo nextest run -p tau-wasm-host --run-ignored all wasi_http_enforcement`
Expected: PASS. If `wasm32-wasip2` is not installed locally, note it and rely on CI's wasm lane.

- [ ] **Step 4: Commit**

```bash
git add docs/explanation/capabilities-and-consent.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(epic-3-4): note wasm delegates net egress to the host boundary"
```

---

## Pre-PR verification

- [ ] **Clippy (both changed crates):**
  `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e34 cargo clippy -p tau-runtime-core --all-targets` and same for `-p tau-runtime-tokio`, `-p tau-wasm-guest`. Expected: no warnings (`-D warnings` in CI).
- [ ] **fmt across all touched crates:** `cargo fmt -p <crate> --check` for `tau-observe`, `tau-runtime-core`, `tau-runtime-tokio`, `tau-wasm-guest`.
- [ ] **Branch decision (open):** work is on `drop-in-guest-http-gate`. Either open the PR from it, or (only before any PR exists) create the task-named `feat/epic-3-4-drop-in-guest-gate` branch. Do NOT rename after a PR exists — GitHub auto-closes it.
- [ ] **Open PR → main.** `gh pr create --base main` against remote `tau-rs/tau`. Merge queue is on: enroll bare `gh pr merge <N> --squash --auto` (NO `--delete-branch`); if `--squash` is rejected ("merge strategy set by the merge queue"), re-enroll bare `gh pr merge <N> --auto`.
- [ ] **PR body:** summary, test plan, escape-hatch checklist (none), ADR check (none — implements existing 3.4 story), and the 3.4↔4.5 disjoint-disposition note.

## Self-review notes (traceability to spec)

- Spec §Mechanism → Tasks 1 (event), 2 (helper), 3 (options+dispatcher+call sites), 4 (guest override).
- Spec §"The skip + delegated event" → Task 2 helper + Task 1 event.
- Spec §Soundness / §Interaction with 4.5 → enforced by the scope fence (net-only skip; `run.rs`/`attenuate.rs` untouched) and captured in Task 5 docs.
- Spec §Test plan → Task 2 (unit), Task 3 (integration + native parity), Task 1 (drift), Task 4 (guest), Task 5 (host enforcement unchanged).
- Spec §"Files touched" table maps 1:1 to Tasks 1-5 file lists.
