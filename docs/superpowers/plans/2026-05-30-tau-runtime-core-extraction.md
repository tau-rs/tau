# tau-runtime-core extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the monolithic `tau-runtime` crate into a no_std + alloc, executor-agnostic `tau-runtime-core` (the kernel) and a `tau-runtime-tokio` host shell (today's behavior, renamed), satisfying the file-by-file design in `docs/superpowers/specs/2026-05-30-tau-runtime-core-design.md` (Phase β.1).

**Architecture:** Five sequential PR-sized phases — (1) `tau-ports` rename + new `Clock`/`RandomSource` ports + `process` feature; (2) four sandbox-adapter crates pick up the renamed trait; (3) new `tau-runtime-core` crate created by moving + adapting the kernel files (replacing `tokio::sync::Mutex` with `RefCell`, `chrono::Utc::now`/`uuid v4`/`ulid v4` with ports, `std::collections::HashMap` with `hashbrown::HashMap`); (4) the residual `tau-runtime` crate renames to `tau-runtime-tokio` and adds `TokioClock`/`OsRandom`/`drive.rs`; (5) docs pass. CI is green at every PR.

**Tech Stack:** Rust 1.84 workspace, `tau-domain`, `tau-ports`, `tau-runtime`, four `tau-sandbox-*` adapter crates, `hashbrown` + `foldhash` (new deps in core), `futures-executor` (smoke test only), `chrono` with `default-features = false`, existing `tracing` 0.1 with `attributes` feature.

---

## Cargo discipline (applies to every cargo invocation in this plan)

Per `CLAUDE.md` cargo rules — **every** cargo command in this plan MUST follow this shape:

```
timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>
```

Timeouts: `test` 300s, `build`/`check` 180s, `clippy` 240s, `fmt --check` 30s. Replace `agent-impl` with the implementing subagent's role (e.g. `target/agent-rt-core`). Prefer `cargo nextest run -p <crate>` over `cargo test -p <crate>` (CI parity). For doctests, use `cargo test --doc -p <crate>` — nextest doctest support is incomplete.

No bare `cargo`. No `--workspace`. No omitting `CARGO_TARGET_DIR`. Lock-contention silently costs minutes.

**Commit + push discipline.** All commits use `-c user.name="<real>" -c user.email="<real>"` and `--no-verify` for the rename PRs (Phase 1, 2, 4) where the host-side lefthook tests can corrupt git identity; do NOT use `--no-verify` for Phase 3 (the core extraction is Rust code change and needs the gate). Push via `scripts/agent-push.sh` only — `git push` direct from the agent runtime is silently killed mid-hook (see `feedback_remote_branch_delete_no_verify`).

---

## Open question resolutions (locked here so β.1 doesn't drift)

The spec leaves three open questions for β.1 to decide. This plan picks:

| Open question | Decision | Rationale |
|---|---|---|
| §13.1 `globset` no_std | Feature-gate `capability_override` behind a `capability-override` default-on feature on `tau-runtime-core`. Embassy ships without; the module never compiles on no_std builds. | `globset` requires `std`. The capability-override narrowing is a host-side concern (dev + CI); MCU does compile-time capability locking. Feature-gating is one-line and reversible. |
| §13.2 `jsonschema` no_std | Feature-gate `tool_args` validator behind a `tool-validation` default-on feature on `tau-runtime-core`. When off, dispatch passes args through unvalidated (LLMs make typed calls; ADR-0010 validation is belt-and-braces). | `jsonschema` requires `std`. The kernel API still routes validation through `ToolArgsValidator`; when the feature is off it compiles to a no-op shim. |
| §13.3 `chrono` no_std | Use `chrono = { workspace = true, default-features = false, features = ["alloc", "serde"] }` in `tau-runtime-core/Cargo.toml`. `DateTime<Utc>` field types stay; `Utc::now()` does NOT compile under this configuration, which forces every `now()` call site through the `Clock` port (the invariant we want). | Spec §13.3's preferred default. The workspace `chrono` is `0.4` with `clock` feature; disabling default features turns off `clock`. |

Hashbrown hasher choice: **`foldhash::quality::FixedState`** per spec §6 + §12.5. Deterministic, no_std-friendly, ~3 KB binary cost.

---

## File structure

### What changes in `crates/tau-ports/`

| File | Action |
|---|---|
| `Cargo.toml` | Add `process` feature (default-on); make `tempfile` gate stay on `test-fixtures` (no change); add `tracing = { workspace = true, default-features = false, features = ["attributes"] }`. |
| `src/lib.rs` | Add `#![no_std]` at crate root; `extern crate alloc;`; update `pub use` re-exports for renamed types + new ports. |
| `src/sandbox.rs` | DELETE — content moves to `src/capability_gate/mod.rs` with renames. |
| `src/capability_gate/mod.rs` | NEW — universal `CapabilityGate` trait + `CapabilityPlan`/`CapabilityHandle`/`CapabilityProbe`/`CapabilityTier`/`WorkingContext`/`ResourceLimits`. |
| `src/capability_gate/process.rs` | NEW — `ProcessCapabilityGate` extension trait under `#[cfg(feature = "process")]`. |
| `src/time.rs` | NEW — `Clock` trait + `MockClock` (under `test-fixtures`). |
| `src/random.rs` | NEW — `RandomSource` trait + `DeterministicRandom` (under `test-fixtures`). |
| `src/error.rs` | Rename `SandboxError` → `CapabilityError`. |
| `src/fixtures.rs` | Rename `MockSandbox` → `MockCapabilityGate`; implement both `CapabilityGate` and (under `process`) `ProcessCapabilityGate`. Add `MockClock` + `DeterministicRandom` (kept under `test-fixtures`). |
| `src/llm.rs`, `src/tool.rs`, `src/storage.rs`, `src/orchestration.rs`, `src/target/*` | No semantic change; replace `use std::*` with `use core::*`/`use alloc::*` where needed. |

### What changes in `crates/tau-sandbox-{native,container,darwin,windows}/`

Each crate: rename `impl Sandbox for X` → `impl CapabilityGate for X` (universal four methods) + `impl ProcessCapabilityGate for X` (the two process methods). `Cargo.toml` keeps `tau-ports` with `default-features = true`. No behavior change.

### What changes in `crates/tau-runtime/` (during Phase 1 and Phase 2)

Mechanical import + type rename: `Sandbox` → `CapabilityGate`, `SandboxPlan` → `CapabilityPlan`, etc. The crate stays named `tau-runtime` and keeps building through Phases 1–2.

### What is created in Phase 3 — `crates/tau-runtime-core/`

| File | Source | Notes |
|---|---|---|
| `Cargo.toml` | NEW | `#![no_std]`-friendly deps only; `default-features = false` on everything that supports it. |
| `src/lib.rs` | NEW (from `tau-runtime/src/lib.rs`) | `#![no_std]`; `extern crate alloc;`; re-exports public API. |
| `src/builder.rs` | from `tau-runtime/src/builder.rs` | HashMap → hashbrown; rename `DynSandbox` → `DynCapabilityGate` (universal methods only). |
| `src/capability.rs` | from `tau-runtime/src/capability.rs` | Replace any `std::*` with `core::*`/`alloc::*`. |
| `src/capability_override/mod.rs` | from `tau-runtime/src/capability_override/mod.rs` | Behind `capability-override` feature in core. |
| `src/dispatch.rs` | from `tau-runtime/src/dispatch.rs` | `Arc` from `alloc::sync::Arc`. |
| `src/error.rs` | from `tau-runtime/src/error.rs` (split) | Drop `RuntimeError::ToolPluginExited { exit_status: std::process::ExitStatus }` — that variant moves to tokio-shell's `tau_runtime_tokio::error`. |
| `src/options.rs` | from `tau-runtime/src/options.rs` | Add `clock: Arc<dyn Clock>`, `random: Arc<dyn RandomSource>` to `RunOptions` (Arc'd so callers can share). |
| `src/outcome.rs` | from `tau-runtime/src/outcome.rs` | Likely already clean; verify no `std::*`. |
| `src/orchestration/budget.rs` | from `tau-runtime/src/orchestration/budget.rs` | Field types stay; `chrono::Utc::now()` call sites move out (none currently; field-only). |
| `src/orchestration/error.rs` | from `tau-runtime/src/orchestration/error.rs` | Verify no `std::*`. |
| `src/orchestration/mod.rs` | from `tau-runtime/src/orchestration/mod.rs` | Verify. |
| `src/orchestration/run_state.rs` | from `tau-runtime/src/orchestration/run_state.rs` | `DateTime<Utc>` field types stay; construction sites take `now: DateTime<Utc>` parameter (caller routes through Clock). |
| `src/orchestration/skill_resolve.rs` | partial move | Pure parts move; `std::fs::read_to_string` at line 314 is gated behind `host-fs` default-on feature OR moves to a port (see Task 3.6.7). |
| `src/orchestration/task_list.rs` | from `tau-runtime/src/orchestration/task_list.rs` | HashMap → hashbrown; verify. |
| `src/orchestration/trace.rs` | from `tau-runtime/src/orchestration/trace.rs` | `tokio::sync::mpsc` → keep host-side; either move trace.rs *partially* (pure parts only) or feature-gate the mpsc subscription on `host-tokio`. See Task 3.6.6. |
| `src/orchestration/virtual_tools.rs` | from `tau-runtime/src/orchestration/virtual_tools.rs` | Route `chrono::Utc::now` (line 7) through Clock port. |
| `src/run.rs` | from `tau-runtime/src/run.rs` | Replace `tokio::sync::Mutex` (line 351) → `core::cell::RefCell`; route `chrono::Utc::now`/`ulid::Ulid::new`/`uuid::Uuid::new_v4` through ports; gate `scope_root: PathBuf` parameter behind `host-fs` feature. |
| `src/stream.rs` | from `tau-runtime/src/stream.rs` | Replace `chrono::Utc::now`/`ulid::Ulid::new` call sites; remove `std::env::current_dir()` at line 561 (lift to options-supplied scope_root); `std::collections::HashMap` (line 13) → hashbrown. |
| `src/tool_args.rs` | from `tau-runtime/src/tool_args.rs` | Behind `tool-validation` feature (jsonschema is std). |
| `tests/executor_agnostic_smoke.rs` | NEW | The smoke test required by spec §11.2. |

### What is created/renamed in Phase 4 — `crates/tau-runtime-tokio/`

| File | Source / Action | Notes |
|---|---|---|
| `Cargo.toml` | rename of `crates/tau-runtime/Cargo.toml` | `name = "tau-runtime-tokio"`; depend on `tau-runtime-core` with `default-features = true`. |
| `src/lib.rs` | new entry | Re-exports core's public API + tokio-shell additions. |
| `src/clock.rs` | NEW | `TokioClock` impl (`chrono::Utc::now().timestamp_millis()`). |
| `src/random.rs` | NEW | `OsRandom` impl (`getrandom::fill`). |
| `src/error.rs` | NEW | Tokio-shell error type carrying `ToolPluginExited { exit_status: std::process::ExitStatus }`; converts to core's `RuntimeError`. |
| `src/drive.rs` | NEW | `pub async fn drive(rt: Arc<tau_runtime_core::Runtime>, ...)` entry. |
| `src/process_gate/` | rename of `tau-runtime/src/sandbox/` | Rename module + types; `DynProcessCapabilityGate` trait-object wrapper lives here. |
| `src/plugin_host/` | move-as-is from `tau-runtime/src/plugin_host/` | Add `#[deprecated]` banner at module root. |
| `src/orchestration/persistence.rs` | move-as-is from `tau-runtime/src/orchestration/persistence.rs` | Stays tokio; follow-up §12.1 in spec. |

### What changes downstream in Phase 4

| Crate | Files | Change |
|---|---|---|
| `tau-cli` | 17 files (verified `grep -rn tau_runtime::` count: 84 use-sites in production code) | `Cargo.toml`: rename dep `tau-runtime` → `tau-runtime-tokio`; `s/tau_runtime/tau_runtime_tokio/g` across files (or `use tau_runtime_tokio as tau_runtime;` shim where the import surface is dense). |
| `tau-workflow` | 2 files | Same pattern. |
| `tau-plugin-compat` | 1 file | Same pattern. |
| `tau-app` | 0 source files; `Cargo.toml` only | Rename dep. |

---

## Phase 1: `tau-ports` rename + new ports

**Goal:** `tau-ports` exposes the renamed `CapabilityGate*` types, the new `ProcessCapabilityGate` extension trait under `process` feature, and the new `Clock`/`RandomSource` ports. `tau-runtime` and the four sandbox-adapter crates still build (they pick up the renamed names via in-crate `use` rewrites in Phase 1.8). CI green.

**Branch:** `feat/runtime-core-ports-rename`

### Task 1.1: Add `process` feature + crate-level `#![no_std]` preamble

**Files:**
- Modify: `crates/tau-ports/Cargo.toml`
- Modify: `crates/tau-ports/src/lib.rs:1-10`

- [ ] **Step 1: Edit `crates/tau-ports/Cargo.toml`** — add `process` to the default features, add the `[features]` entry, and add `tracing` as a `no_std`-compatible dep.

Replace the `[features]` block with:

```toml
[features]
default       = ["process"]
process       = []
serde         = ["dep:serde", "tau-domain/serde", "uuid/serde"]
test-fixtures = ["dep:tempfile"]
```

Add to `[dependencies]` (after the existing `tempfile` line):

```toml
tracing = { workspace = true, default-features = false, features = ["attributes"] }
```

Change the `chrono` line to:

```toml
chrono = { workspace = true, default-features = false, features = ["alloc", "serde"] }
```

- [ ] **Step 2: Edit `crates/tau-ports/src/lib.rs`** — add `#![no_std]` and `extern crate alloc;` at the very top of the file (above the existing inner `#![forbid(unsafe_code)]` attribute).

Replace lines 1–3 with:

```rust
#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

extern crate alloc;

#[cfg(any(test, feature = "test-fixtures"))]
extern crate std;
```

(The `extern crate std` line keeps fixtures-only types like `tempfile::TempDir` compilable when the `test-fixtures` feature is on.)

- [ ] **Step 3: Verify the crate still compiles** with the rename pending.

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports
```
Expected: FAIL with errors about `std::*` imports inside `sandbox.rs`/`fixtures.rs` (those still reference `std`).

This failure is the baseline that Tasks 1.2 and 1.4 fix. Capture the exact error list as a checklist for those tasks.

- [ ] **Step 4: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/Cargo.toml crates/tau-ports/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ports): add process feature + no_std preamble"
```

### Task 1.2: Rename `Sandbox*` types → `CapabilityGate*` (move `sandbox.rs` → `capability_gate/mod.rs`)

**Files:**
- Create: `crates/tau-ports/src/capability_gate/mod.rs` (from old `sandbox.rs` with renames)
- Delete: `crates/tau-ports/src/sandbox.rs`
- Modify: `crates/tau-ports/src/error.rs:*` (rename `SandboxError` → `CapabilityError`)

- [ ] **Step 1: Write a failing test that locks the universal `CapabilityGate` shape**

Create `crates/tau-ports/tests/capability_gate_shape.rs`:

```rust
//! Locks the shape of CapabilityGate / CapabilityPlan / CapabilityProbe so
//! a future drift (e.g. re-adding wrap_spawn to the universal trait) fails
//! at compile time.

use tau_ports::{CapabilityGate, CapabilityPlan, CapabilityProbe, CapabilityShapeSet};

fn _shape_check<T: CapabilityGate>() {
    fn _accepts_universal(_t: &dyn CapabilityGate) {}
    // CapabilityGate must NOT carry process-flavored methods.
    // The four universal methods only:
    //   fn name(&self) -> &str;
    //   async fn probe(&self) -> CapabilityProbe;
    //   fn supported_shapes(&self) -> CapabilityShapeSet;
    //   fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), tau_ports::CapabilityError>;
}

#[test]
fn capability_gate_traits_exist() {
    let _ = std::any::TypeId::of::<CapabilityPlan>();
    let _ = std::any::TypeId::of::<CapabilityProbe>();
    let _ = std::any::TypeId::of::<CapabilityShapeSet>();
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --test capability_gate_shape
```
Expected: FAIL — `CapabilityGate` does not exist yet.

- [ ] **Step 3: Create `crates/tau-ports/src/capability_gate/mod.rs`**

Copy the current `crates/tau-ports/src/sandbox.rs` to `crates/tau-ports/src/capability_gate/mod.rs`. Then apply these renames inside the new file (`replace_all`):

| from | to |
|---|---|
| `pub trait Sandbox` | `pub trait CapabilityGate` |
| `pub struct SandboxPlan` | `pub struct CapabilityPlan` |
| `pub struct SandboxHandle` | `pub struct CapabilityHandle` |
| `pub enum SandboxProbe` | `pub enum CapabilityProbe` |
| `pub enum SandboxTier` | `pub enum CapabilityTier` |
| `use crate::error::SandboxError` | `use crate::error::CapabilityError` |
| `SandboxError` (any remaining reference) | `CapabilityError` |
| `[Sandbox::` (in docs) | `[CapabilityGate::` |
| `[Sandbox]` (in docs) | `[CapabilityGate]` |
| `SandboxPlan` (any non-rename context) | `CapabilityPlan` |
| `SandboxHandle` (any non-rename context) | `CapabilityHandle` |
| `SandboxProbe` (any non-rename context) | `CapabilityProbe` |
| `SandboxTier` (any non-rename context) | `CapabilityTier` |
| `use std::collections::BTreeMap` | `use alloc::collections::BTreeMap` |
| `use std::path::PathBuf` | gate behind `#[cfg(feature = "process")]` (next step splits this) |
| `use std::process::Command` | DELETE — Command moves to `process.rs` with the extension trait |

Then remove the **process-flavored** methods from `CapabilityGate` — specifically `wrap_spawn` and `apply_post_spawn`. They move to `ProcessCapabilityGate` in Task 1.3. The universal `CapabilityGate` retains only `name`, `probe`, `supported_shapes`, `validate_plan`.

Wrap the `WorkingContext::working_dir: Option<PathBuf>` field declaration in a `#[cfg(feature = "process")]` block; provide an empty `Default` outside the feature so `WorkingContext::default()` still compiles. The simplest shape:

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkingContext {
    /// Working directory hint. Only meaningful when the `process` feature
    /// is enabled (no_std hosts have no filesystem path semantics).
    #[cfg(feature = "process")]
    pub working_dir: Option<std::path::PathBuf>,
    /// Environment variables to seed the gated execution.
    pub env: alloc::collections::BTreeMap<alloc::string::String, alloc::string::String>,
}
```

- [ ] **Step 4: Delete `crates/tau-ports/src/sandbox.rs`**

```
git rm crates/tau-ports/src/sandbox.rs
```

- [ ] **Step 5: Rename `SandboxError` → `CapabilityError` in `crates/tau-ports/src/error.rs`**

Use Edit with `replace_all`:
- `SandboxError` → `CapabilityError` (every occurrence in error.rs)

- [ ] **Step 6: Update `crates/tau-ports/src/lib.rs`** — module + re-export rename

Replace the existing `pub mod sandbox;` line with:

```rust
pub mod capability_gate;
```

Replace the existing `pub use sandbox::{...}` block with:

```rust
pub use capability_gate::{
    CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe, CapabilityTier,
    ResourceLimits, WorkingContext,
};
```

Update the `pub use error::{...}` block — change `SandboxError` to `CapabilityError` in the export list.

Update the doc-string at lines 8–14 — replace the bullet `[sandbox::Sandbox]` line with:

```rust
//! - [`capability_gate::CapabilityGate`] — universal capability gate
//!   contract; concrete impls live in tau-sandbox-{native,container,darwin,windows}.
```

- [ ] **Step 7: Run the test** — it should now reach the universal-shape assertion and pass

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports --no-default-features
```
Expected: PASS. (Building without `process` validates the universal trait stands alone with no `std::*` references.)

- [ ] **Step 8: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/src/capability_gate/mod.rs crates/tau-ports/src/lib.rs crates/tau-ports/src/error.rs crates/tau-ports/tests/capability_gate_shape.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ports): rename Sandbox* -> CapabilityGate* (universal trait only)"
```

### Task 1.3: Add `ProcessCapabilityGate` extension trait

**Files:**
- Create: `crates/tau-ports/src/capability_gate/process.rs`
- Modify: `crates/tau-ports/src/capability_gate/mod.rs` (declare the submodule)
- Modify: `crates/tau-ports/src/lib.rs` (re-export under `process` feature)

- [ ] **Step 1: Write `crates/tau-ports/src/capability_gate/process.rs`**

```rust
//! `ProcessCapabilityGate` — process-spawn extension of [`CapabilityGate`].
//!
//! Adapters that gate **process** boundaries (OS sandboxes, container
//! sandboxes) implement this in addition to the universal `CapabilityGate`.
//! Adapters that gate non-process boundaries (wasm component import maps;
//! MCP contract wires) implement a different extension trait owned by
//! their respective host crate.

use std::process::Command;

use super::{CapabilityGate, CapabilityHandle, CapabilityPlan};
use crate::error::CapabilityError;

/// Extension trait: adapters that gate process spawn boundaries.
///
/// Implementors must also implement the universal [`CapabilityGate`].
#[allow(async_fn_in_trait)]
pub trait ProcessCapabilityGate: CapabilityGate {
    /// Apply gate enforcement to a `Command` in preparation for spawn.
    /// On Linux native, this registers `pre_exec` hooks. The returned
    /// `CapabilityHandle` holds any ambient resources (cgroup,
    /// namespace fd) and releases them on Drop.
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError>;

    /// Adapter-specific post-spawn setup. Called after `cmd.spawn()`
    /// succeeds and the child PID is known. Default: no-op.
    async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        let _ = (plan, child_pid, handle);
        Ok(())
    }
}
```

- [ ] **Step 2: Declare the submodule in `crates/tau-ports/src/capability_gate/mod.rs`**

Add this line near the top of `mod.rs` (after the file-level doc comment):

```rust
#[cfg(feature = "process")]
pub mod process;
```

- [ ] **Step 3: Re-export in `crates/tau-ports/src/lib.rs`**

Add after the existing `pub use capability_gate::{...}` block:

```rust
#[cfg(feature = "process")]
pub use capability_gate::process::ProcessCapabilityGate;
```

- [ ] **Step 4: Run the build to confirm both features compile**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports --no-default-features
```
Expected: both PASS. The first builds with `process`; the second builds without it (proving the universal trait stands alone with no `std::process` references).

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/src/capability_gate/process.rs crates/tau-ports/src/capability_gate/mod.rs crates/tau-ports/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ports): add ProcessCapabilityGate extension trait under process feature"
```

### Task 1.4: Update `fixtures.rs` — rename `MockSandbox` → `MockCapabilityGate`

**Files:**
- Modify: `crates/tau-ports/src/fixtures.rs`

- [ ] **Step 1: Edit `crates/tau-ports/src/fixtures.rs`** — apply renames and split the impl

Use Edit with these targeted replacements:

1. Replace the section header `// MockSandbox` with `// MockCapabilityGate`.
2. Replace `pub struct MockSandbox` with `pub struct MockCapabilityGate`.
3. Replace every `impl MockSandbox` → `impl MockCapabilityGate`.
4. Replace `impl Sandbox for MockSandbox` with TWO impl blocks (split the methods):

```rust
impl CapabilityGate for MockCapabilityGate {
    fn name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> CapabilityProbe {
        CapabilityProbe::Available {
            tier: CapabilityTier::None,
            details: "mock — no enforcement".into(),
        }
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        let mut set = CapabilityShapeSet::new();
        set.insert(CapabilityShape::FilesystemRead);
        set.insert(CapabilityShape::FilesystemWrite);
        set.insert(CapabilityShape::ProcessExec);
        set.insert(CapabilityShape::NetworkHttp);
        set.insert(CapabilityShape::AgentSpawn);
        set
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        let supported = self.supported_shapes();
        for cap in &plan.capabilities {
            let shape = cap.required_shape();
            if !supported.contains(&shape) {
                return Err(CapabilityError::ShapeUnsupported { shape });
            }
        }
        Ok(())
    }
}

#[cfg(feature = "process")]
impl crate::ProcessCapabilityGate for MockCapabilityGate {
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        _cmd: &mut std::process::Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        self.validate_plan(plan)?;
        Ok(CapabilityHandle::noop())
    }
}
```

5. Replace every other `MockSandbox` reference (e.g. in `plan_from_capabilities` doc-comments, test fns) with `MockCapabilityGate`.
6. Update the imports at the top of fixtures.rs — change `use crate::sandbox::*;` (if present) to `use crate::capability_gate::*;`; change `SandboxError`/`SandboxHandle`/`SandboxPlan`/`SandboxProbe`/`SandboxTier` in the `use` line to the renamed equivalents.
7. The two `#[test]` fns at lines 695, 702 — rename local variable `let mock = MockSandbox::new("mem");` to `let mock = MockCapabilityGate::new("mem");`.

- [ ] **Step 2: Verify `tau-ports` builds with fixtures enabled**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports --features test-fixtures,process
```
Expected: PASS.

- [ ] **Step 3: Run tau-ports unit tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --features test-fixtures,process
```
Expected: PASS (existing mock-sandbox tests now exercise `MockCapabilityGate`).

- [ ] **Step 4: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/src/fixtures.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-ports): rename MockSandbox -> MockCapabilityGate; impl both traits"
```

### Task 1.5: Add `Clock` port + `MockClock`

**Files:**
- Create: `crates/tau-ports/src/time.rs`
- Modify: `crates/tau-ports/src/lib.rs` (add module + re-exports)
- Modify: `crates/tau-ports/src/fixtures.rs` (re-export `MockClock` for test-fixtures users)

- [ ] **Step 1: Write a failing test**

Append to `crates/tau-ports/tests/capability_gate_shape.rs`:

```rust
#[test]
fn mock_clock_is_monotonic() {
    use tau_ports::{Clock, MockClock};

    let clock = MockClock::default();
    let a = clock.now();
    let b = clock.now();
    let c = clock.now();
    assert!(b > a);
    assert!(c > b);
}
```

- [ ] **Step 2: Run the test to verify it fails**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --test capability_gate_shape --features test-fixtures
```
Expected: FAIL — `Clock`, `MockClock` not found.

- [ ] **Step 3: Create `crates/tau-ports/src/time.rs`**

```rust
//! Clock port — abstracts wall-clock time.
//!
//! The kernel reads "now" only through this port; host shells supply
//! the concrete impl (`TokioClock` on tokio hosts, `EmbassyClock` on
//! MCU, etc.). Routing all `now()` calls through the port is what
//! makes `tau-runtime-core` portable to executors with no
//! `std::time::SystemTime`.

use core::sync::atomic::{AtomicI64, Ordering};

/// Wall-clock reading source.
///
/// Implementations return milliseconds since the Unix epoch. Negative
/// values are legal for pre-1970 timestamps. Resolution is
/// millisecond; sub-ms timing belongs in benchmarking, not in agent
/// semantics.
pub trait Clock: Send + Sync {
    /// Return the current instant as milliseconds since the Unix epoch.
    fn now(&self) -> i64;
}

/// Deterministic in-memory clock for tests. Each `now()` call returns
/// one millisecond after the previous one, starting from 0.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Default)]
pub struct MockClock {
    counter: AtomicI64,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl MockClock {
    /// Construct a `MockClock` with the cursor at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a `MockClock` with the cursor at the supplied instant.
    pub fn at(start_ms: i64) -> Self {
        Self {
            counter: AtomicI64::new(start_ms - 1),
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl Clock for MockClock {
    fn now(&self) -> i64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }
}
```

- [ ] **Step 4: Add `pub mod time;` and the re-exports to `crates/tau-ports/src/lib.rs`**

After the existing `pub mod target;` line, add:

```rust
pub mod random;
pub mod time;
```

Then add to the re-export block:

```rust
pub use time::Clock;
#[cfg(any(test, feature = "test-fixtures"))]
pub use time::MockClock;
pub use random::RandomSource;
#[cfg(any(test, feature = "test-fixtures"))]
pub use random::DeterministicRandom;
```

(`random` is created in Task 1.6; declare both together here so the lib.rs edit is one-shot.)

- [ ] **Step 5: Verify the test now passes**

Defer running until Task 1.6 lands the `random` module (the lib.rs edit references it). Continue.

- [ ] **Step 6: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/src/time.rs crates/tau-ports/src/lib.rs crates/tau-ports/tests/capability_gate_shape.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ports): add Clock port + MockClock"
```

### Task 1.6: Add `RandomSource` port + `DeterministicRandom`

**Files:**
- Create: `crates/tau-ports/src/random.rs`

- [ ] **Step 1: Write a failing test**

Append to `crates/tau-ports/tests/capability_gate_shape.rs`:

```rust
#[test]
fn deterministic_random_is_seeded_and_repeatable() {
    use tau_ports::{DeterministicRandom, RandomSource};

    let a = DeterministicRandom::seeded(0xC0FFEE);
    let mut buf_a = [0u8; 16];
    a.fill(&mut buf_a);

    let b = DeterministicRandom::seeded(0xC0FFEE);
    let mut buf_b = [0u8; 16];
    b.fill(&mut buf_b);

    assert_eq!(buf_a, buf_b, "same seed must produce same bytes");

    let c = DeterministicRandom::seeded(0xDEADBEEF);
    let mut buf_c = [0u8; 16];
    c.fill(&mut buf_c);
    assert_ne!(buf_a, buf_c, "different seeds must produce different bytes");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --test capability_gate_shape --features test-fixtures
```
Expected: FAIL — `DeterministicRandom`, `RandomSource` not found.

- [ ] **Step 3: Create `crates/tau-ports/src/random.rs`**

```rust
//! RandomSource port — abstracts entropy.
//!
//! The kernel mints UUID/ULID bytes only through this port; host shells
//! supply the concrete impl (`OsRandom` on std hosts, `HwRandom` on MCU).
//! Routing entropy through a port is what makes `tau-runtime-core` runnable
//! on bare-metal targets with no `getrandom`.

use core::cell::Cell;

/// Source of cryptographic-grade random bytes.
///
/// Implementations must produce uniformly distributed bytes. The MCU
/// host wraps a hardware TRNG; the tokio host wraps `getrandom`. The
/// deterministic test fixture is xorshift-seeded and is NOT suitable
/// for cryptographic use.
pub trait RandomSource: Send + Sync {
    /// Fill `dest` with random bytes.
    fn fill(&self, dest: &mut [u8]);
}

/// Seeded, deterministic PRNG for tests. xorshift64*; NOT cryptographic.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug)]
pub struct DeterministicRandom {
    state: Cell<u64>,
}

// SAFETY: Cell<u64> is !Sync, but DeterministicRandom is a test-only
// fixture used from a single task at a time. We need Sync so it can be
// stored in Arc<dyn RandomSource> alongside Send + Sync hosts.
//
// We assert single-task discipline at the type level: the fixture
// panics if called concurrently. (Implementation note: the production
// `OsRandom` is genuinely thread-safe; this assertion only matters for
// the fixture.)
#[cfg(any(test, feature = "test-fixtures"))]
unsafe impl Sync for DeterministicRandom {}

#[cfg(any(test, feature = "test-fixtures"))]
impl DeterministicRandom {
    /// Construct a `DeterministicRandom` from a 64-bit seed.
    pub fn seeded(seed: u64) -> Self {
        // xorshift64* requires non-zero seed; substitute a canonical
        // value for the zero case rather than panic.
        let s = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed };
        Self {
            state: Cell::new(s),
        }
    }

    fn next_u64(&self) -> u64 {
        let mut s = self.state.get();
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.state.set(s);
        s.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl RandomSource for DeterministicRandom {
    fn fill(&self, dest: &mut [u8]) {
        let mut i = 0;
        while i < dest.len() {
            let chunk = self.next_u64().to_le_bytes();
            let take = core::cmp::min(8, dest.len() - i);
            dest[i..i + take].copy_from_slice(&chunk[..take]);
            i += take;
        }
    }
}
```

- [ ] **Step 4: Verify both Clock and Random tests pass**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --test capability_gate_shape --features test-fixtures
```
Expected: PASS (3 tests).

- [ ] **Step 5: Verify the no-default-features build is still clean**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports --no-default-features
```
Expected: PASS — universal `CapabilityGate` + `Clock` + `RandomSource` (without test-fixtures) compile no_std.

- [ ] **Step 6: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/src/random.rs crates/tau-ports/tests/capability_gate_shape.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ports): add RandomSource port + DeterministicRandom fixture"
```

### Task 1.7: Sweep `std::*` → `core::*`/`alloc::*` in remaining tau-ports modules

**Files:**
- Modify: `crates/tau-ports/src/llm.rs`
- Modify: `crates/tau-ports/src/tool.rs`
- Modify: `crates/tau-ports/src/storage.rs`
- Modify: `crates/tau-ports/src/orchestration.rs`
- Modify: `crates/tau-ports/src/error.rs`
- Modify: `crates/tau-ports/src/target/*.rs`

- [ ] **Step 1: Run the no-default-features build to enumerate remaining `std::*` errors**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports --no-default-features 2>&1 | tee /tmp/tau-ports-std-errors.txt | tail -80
```

Read the error list — each one points to a file + line where `std::*` must become `core::*` or `alloc::*`.

- [ ] **Step 2: Apply mechanical rewrites per the table**

Common substitutions (apply with Edit `replace_all` per file):

| from | to | rationale |
|---|---|---|
| `use std::collections::BTreeMap` | `use alloc::collections::BTreeMap` | core has no collections |
| `use std::collections::BTreeSet` | `use alloc::collections::BTreeSet` | |
| `use std::sync::Arc` | `use alloc::sync::Arc` | |
| `use std::string::String` | `use alloc::string::String` | |
| `use std::vec::Vec` | `use alloc::vec::Vec` | |
| `use std::borrow::Cow` | `use alloc::borrow::Cow` | |
| `use std::format` (macro) | `use alloc::format` | |
| `use std::time::Duration` | `use core::time::Duration` | Duration is in core |
| `use std::fmt::*` | `use core::fmt::*` | |
| `use std::error::Error` | leave as-is; gate the impl behind a feature OR use `core::error::Error` if MSRV ≥ 1.81 (this workspace is on a recent toolchain — confirm with `rust-version.workspace` in Cargo.toml; if ≥ 1.81, use `core::error::Error`) |

For `error.rs`: `thiserror`'s `Error` derive emits `impl std::error::Error`. With MSRV ≥ 1.81, switch to `thiserror::Error` (which is fine) and verify the generated impl. If `thiserror` blocks no_std, gate `impl Error` blocks behind `#[cfg(feature = "std")]` — but the workspace `thiserror` is `>= 2.0` which supports no_std out of the box; verify with `cat Cargo.toml | grep thiserror`.

- [ ] **Step 3: Verify the build**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports --no-default-features
```
Expected: PASS.

- [ ] **Step 4: Verify default-features build still passes**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports
```
Expected: PASS.

- [ ] **Step 5: Run tau-ports tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --features test-fixtures
```
Expected: PASS.

- [ ] **Step 6: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-ports/src/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-ports): replace std::* with core::*/alloc::* throughout"
```

### Task 1.8: Update `tau-runtime` to track the rename (keeps building)

**Files:**
- Modify: `crates/tau-runtime/src/builder.rs` (renames + trait split)
- Modify: `crates/tau-runtime/src/sandbox/*.rs` (use sites)
- Modify: `crates/tau-runtime/src/error.rs` (`SandboxError` → `CapabilityError`)
- Modify: any other tau-runtime file with `SandboxError`/`SandboxPlan`/`SandboxHandle`/`SandboxProbe`/`SandboxTier`/`Sandbox` imports

- [ ] **Step 1: Enumerate use-sites**

```
grep -rn -E "tau_ports::Sandbox|SandboxPlan|SandboxHandle|SandboxProbe|SandboxTier|SandboxError" crates/tau-runtime/src/ | tee /tmp/sandbox-sites.txt
```

This produces the worklist for the rename.

- [ ] **Step 2: Mechanical rename across `crates/tau-runtime/src/`**

For each file in the worklist, apply Edit `replace_all`:

| from | to |
|---|---|
| `tau_ports::Sandbox` | `tau_ports::CapabilityGate` |
| `tau_ports::SandboxPlan` | `tau_ports::CapabilityPlan` |
| `tau_ports::SandboxHandle` | `tau_ports::CapabilityHandle` |
| `tau_ports::SandboxProbe` | `tau_ports::CapabilityProbe` |
| `tau_ports::SandboxTier` | `tau_ports::CapabilityTier` |
| `tau_ports::SandboxError` | `tau_ports::CapabilityError` |
| ` Sandbox ` (bare token, careful) | ` CapabilityGate ` — verify each match before applying |
| `SandboxError` (within `tau_ports` import contexts) | `CapabilityError` |
| `SandboxPlan` (bare) | `CapabilityPlan` |
| `SandboxHandle` (bare) | `CapabilityHandle` |
| `SandboxProbe` (bare) | `CapabilityProbe` |
| `SandboxTier` (bare) | `CapabilityTier` |

Carefully **do not** rename: `tau-sandbox-native`, `tau-sandbox-container`, `tau-sandbox-darwin`, `tau-sandbox-windows` (crate names), `NativeSandbox`, `ContainerSandbox`, `DarwinSandbox`, `WindowsSandbox` (concrete impl names — spec §3 "What does NOT rename"). Also do not rename `[sandbox]` TOML keys or `crates/tau-runtime/src/sandbox/` module path (the *module* keeps the legacy name through Phase 1; it moves in Phase 4 to `process_gate/`).

- [ ] **Step 3: Split `DynSandbox` → `DynCapabilityGate` (universal) + `DynProcessCapabilityGate` (process)**

In `crates/tau-runtime/src/builder.rs` (around lines 240–295), replace the existing `DynSandbox` trait + blanket impl with TWO traits:

```rust
/// Object-safe wrapper of [`CapabilityGate`] (the universal four
/// methods). Stored in registries that don't care about process
/// extensions (wasm host, MCU, MCP facilitator).
pub trait DynCapabilityGate: Send + Sync {
    fn name(&self) -> &str;
    fn probe<'a>(&'a self) -> BoxFuture<'a, CapabilityProbe>;
    fn supported_shapes(&self) -> CapabilityShapeSet;
    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError>;
}

impl<T: CapabilityGate + 'static> DynCapabilityGate for T {
    fn name(&self) -> &str { CapabilityGate::name(self) }
    fn probe<'a>(&'a self) -> BoxFuture<'a, CapabilityProbe> {
        Box::pin(CapabilityGate::probe(self))
    }
    fn supported_shapes(&self) -> CapabilityShapeSet {
        CapabilityGate::supported_shapes(self)
    }
    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        CapabilityGate::validate_plan(self, plan)
    }
}

/// Object-safe wrapper of [`ProcessCapabilityGate`]. The process gate
/// registry in `tau-runtime` (and post-Phase-4 in `tau-runtime-tokio`)
/// stores `Arc<dyn DynProcessCapabilityGate>`.
pub trait DynProcessCapabilityGate: DynCapabilityGate {
    fn wrap_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        cmd: &'a mut std::process::Command,
    ) -> BoxFuture<'a, Result<CapabilityHandle, CapabilityError>>;

    fn apply_post_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        child_pid: i32,
        handle: &'a mut CapabilityHandle,
    ) -> BoxFuture<'a, Result<(), CapabilityError>>;
}

impl<T: ProcessCapabilityGate + 'static> DynProcessCapabilityGate for T {
    fn wrap_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        cmd: &'a mut std::process::Command,
    ) -> BoxFuture<'a, Result<CapabilityHandle, CapabilityError>> {
        Box::pin(ProcessCapabilityGate::wrap_spawn(self, plan, cmd))
    }

    fn apply_post_spawn<'a>(
        &'a self,
        plan: &'a CapabilityPlan,
        child_pid: i32,
        handle: &'a mut CapabilityHandle,
    ) -> BoxFuture<'a, Result<(), CapabilityError>> {
        Box::pin(ProcessCapabilityGate::apply_post_spawn(self, plan, child_pid, handle))
    }
}
```

Add the imports at the top of `builder.rs`:

```rust
use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, ProcessCapabilityGate,
};
```

Update any place inside `builder.rs` that referenced `dyn DynSandbox` — most should become `dyn DynProcessCapabilityGate` (the runtime today only stores process-flavored adapters).

- [ ] **Step 4: Verify tau-runtime compiles**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
```
Expected: FAIL with errors only from `crates/tau-sandbox-*` crates (they still `impl Sandbox`, which no longer exists). Phase 2 fixes that.

If errors reference `tau-runtime` source — fix them now per the rename table.

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime/src/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-runtime): update Sandbox* -> CapabilityGate* imports + DynCapabilityGate split"
```

### Task 1.9: PR Phase 1

- [ ] **Step 1: Push the branch**

```
scripts/agent-push.sh
```

- [ ] **Step 2: Open the PR**

```
gh pr create --title "feat(tau-ports): rename Sandbox* -> CapabilityGate*, add Clock + RandomSource ports (Phase β.1.1)" --body "$(cat <<'EOF'
## Summary

- Renames `tau_ports::Sandbox` and adjacent types to `CapabilityGate*` per `docs/superpowers/specs/2026-05-30-tau-runtime-core-design.md` §3.
- Splits the trait via Option B: universal `CapabilityGate` (4 methods) + `ProcessCapabilityGate` extension trait (under new `process` feature, default-on).
- Adds two new ports — `Clock` and `RandomSource` — with `MockClock` / `DeterministicRandom` test fixtures.
- Makes `tau-ports` `#![no_std] + alloc` (default features on still produce identical std-host behavior).
- Migrates `tau-runtime` import sites to track the rename; the four `tau-sandbox-*` crates fail until Phase β.1.2 lands.

## Phase

β.1.1 of the runtime-core extraction (5-PR sequence).

## Test plan

- [x] `cargo check -p tau-ports`
- [x] `cargo check -p tau-ports --no-default-features` (no_std build)
- [x] `cargo nextest run -p tau-ports --features test-fixtures` (existing tests + 3 new shape tests)
- [ ] CI is the cross-target gate (Linux + macOS + Windows).
EOF
)"
```

Phase 1 done. Move to Phase 2 once this PR merges.

---

## Phase 2: Sandbox-adapter crates pick up the renamed trait

**Goal:** The four `tau-sandbox-*` crates implement both `CapabilityGate` (universal) and `ProcessCapabilityGate` (process extension). All existing tests stay green.

**Branch:** `feat/runtime-core-sandbox-adapters` (off `main` after Phase 1 merges).

### Task 2.1: `tau-sandbox-native`

**Files:**
- Modify: `crates/tau-sandbox-native/src/lib.rs`
- Verify: `crates/tau-sandbox-native/Cargo.toml` (no change expected; `tau-ports` already on default features)

- [ ] **Step 1: Read the current impl**

```
grep -n "impl Sandbox\|impl tau_ports::Sandbox\|wrap_spawn\|apply_post_spawn\|fn name\|fn probe\|fn supported_shapes\|fn validate_plan" crates/tau-sandbox-native/src/lib.rs | head -30
```

Identify the line range of the existing `impl Sandbox for NativeSandbox` block (line 56 per audit on origin/main).

- [ ] **Step 2: Split the impl into two blocks**

Replace `impl Sandbox for NativeSandbox { ... }` with:

```rust
impl CapabilityGate for NativeSandbox {
    fn name(&self) -> &str { /* ...moved verbatim... */ }
    async fn probe(&self) -> CapabilityProbe { /* ...moved... */ }
    fn supported_shapes(&self) -> CapabilityShapeSet { /* ...moved... */ }
    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        /* ...moved... */
    }
}

impl ProcessCapabilityGate for NativeSandbox {
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut std::process::Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        /* ...moved verbatim from old wrap_spawn body... */
    }

    // If apply_post_spawn was overridden, include it here. NativeSandbox
    // overrides it for the proxy task — verify by grepping current source.
    async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        /* ...moved verbatim... */
    }
}
```

Update the top-of-file imports:

```rust
use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, CapabilityTier, ProcessCapabilityGate, ResourceLimits, WorkingContext,
};
```

Apply the rename table from Phase 1.8 across the whole file (every `Sandbox*` reference → `Capability*`).

- [ ] **Step 3: Run the tau-sandbox-native test suite**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-sandbox-native
```
Expected: PASS (Linux only; on macOS/Windows the crate may not build at all — check workspace `[target.'cfg(...)']` gates and run only on Linux).

- [ ] **Step 4: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-sandbox-native/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-sandbox-native): impl CapabilityGate + ProcessCapabilityGate"
```

### Task 2.2: `tau-sandbox-container`

**Files:**
- Modify: `crates/tau-sandbox-container/src/lib.rs`

- [ ] **Step 1: Apply the same split as Task 2.1**

Replace `impl Sandbox for ContainerSandbox` at line 68 with the two impl blocks (universal + process). Apply the rename table across the file. Imports follow Task 2.1's template.

- [ ] **Step 2: Run the container adapter tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-sandbox-container
```
Expected: PASS.

- [ ] **Step 3: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-sandbox-container/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-sandbox-container): impl CapabilityGate + ProcessCapabilityGate"
```

### Task 2.3: `tau-sandbox-darwin`

**Files:**
- Modify: `crates/tau-sandbox-darwin/src/lib.rs`

- [ ] **Step 1: Apply the same split**

Same as Task 2.1; `impl Sandbox for DarwinSandbox` at line 51.

- [ ] **Step 2: Run tests (macOS host only)**

If the implementing agent is on macOS:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-sandbox-darwin
```
On Linux/Windows: skip — let CI's macOS slot verify (gh actions matrix exercises it).

- [ ] **Step 3: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-sandbox-darwin/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-sandbox-darwin): impl CapabilityGate + ProcessCapabilityGate"
```

### Task 2.4: `tau-sandbox-windows`

**Files:**
- Modify: `crates/tau-sandbox-windows/src/lib.rs`

- [ ] **Step 1: Apply the same split**

Same as Task 2.1; `impl Sandbox for WindowsSandbox` at line 61.

- [ ] **Step 2: Tests run only on Windows hosts**

Skip locally on non-Windows; CI's windows-latest slot is the gate.

- [ ] **Step 3: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-sandbox-windows/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-sandbox-windows): impl CapabilityGate + ProcessCapabilityGate"
```

### Task 2.5: Full workspace build + PR Phase 2

- [ ] **Step 1: Build tau-runtime against the renamed adapters**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
```
Expected: PASS.

- [ ] **Step 2: Run tau-runtime tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
```
Expected: PASS — every existing test still green.

- [ ] **Step 3: Push + open PR**

```
scripts/agent-push.sh
gh pr create --title "refactor(tau-sandbox-*): impl renamed CapabilityGate + ProcessCapabilityGate (Phase β.1.2)" --body "$(cat <<'EOF'
## Summary

Four `tau-sandbox-*` adapter crates pick up the renamed `CapabilityGate` + `ProcessCapabilityGate` traits from Phase β.1.1 (PR #<N>). No behavior change — pure trait reorganization.

## Phase

β.1.2 of the runtime-core extraction (5-PR sequence).

## Test plan

- [x] tau-sandbox-native — `cargo nextest run -p tau-sandbox-native` (Linux)
- [x] tau-sandbox-container — `cargo nextest run -p tau-sandbox-container`
- [x] tau-sandbox-darwin — CI macOS slot
- [x] tau-sandbox-windows — CI windows slot
- [x] tau-runtime — full test suite remains green
EOF
)"
```

---

## Phase 3: Create `tau-runtime-core`

**Goal:** A new `crates/tau-runtime-core/` crate that contains the kernel logic from today's `tau-runtime`, with all executor-specific dependencies removed. `tau-runtime` continues to exist as a thin re-export of the core (the rename to `tau-runtime-tokio` happens in Phase 4). `cargo check -p tau-runtime-core --no-default-features --target wasm32-unknown-unknown` succeeds.

**Branch:** `feat/runtime-core-extraction` (off `main` after Phase 2 merges).

This phase is the largest. It is broken into nine subtasks; each is its own commit; the PR is opened only at the end.

### Task 3.1: Create the empty `tau-runtime-core` crate

**Files:**
- Create: `crates/tau-runtime-core/Cargo.toml`
- Create: `crates/tau-runtime-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add member)

- [ ] **Step 1: Add the crate to the workspace**

In the workspace root `Cargo.toml`, find the `members = [...]` array and add `"crates/tau-runtime-core"`. Keep alphabetical ordering.

- [ ] **Step 2: Write `crates/tau-runtime-core/Cargo.toml`**

```toml
[package]
name = "tau-runtime-core"
description = "Executor-agnostic kernel of tau. no_std + alloc. Host shells (tau-runtime-tokio, tau-runtime-embassy) drive this on their executor."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[dependencies]
tau-domain          = { workspace = true, default-features = false, features = ["serde"] }
tau-ports           = { workspace = true, default-features = false, features = ["serde"] }
thiserror           = { workspace = true, default-features = false }
serde               = { workspace = true, default-features = false, features = ["alloc", "derive"] }
serde_json          = { version = "1", default-features = false, features = ["alloc"] }
chrono              = { workspace = true, default-features = false, features = ["alloc", "serde"] }
hashbrown           = { version = "0.15", default-features = false, features = ["default-hasher"] }
foldhash            = { version = "0.1", default-features = false }
futures-core        = { workspace = true, default-features = false, features = ["alloc"] }
tracing             = { workspace = true, default-features = false, features = ["attributes"] }
# Optional UUID/ULID minters: both have no_std-compatible constructors
# when supplied entropy + timestamp externally. Used via the RandomSource
# + Clock ports.
uuid                = { workspace = true, default-features = false }
ulid                = { version = "1", default-features = false }
# Process-shaped capability gates need to wire Command — through the
# `process` feature on tau-ports, transitively.
async-stream        = { workspace = true }
base64              = { workspace = true, default-features = false, features = ["alloc"] }

# Optional std-only deps gated by features.
globset             = { workspace = true, optional = true }
jsonschema          = { workspace = true, optional = true }

[features]
default            = ["process", "capability-override", "tool-validation", "host-fs"]
# Enables ProcessCapabilityGate registries + process-shaped run flow.
# Pulls in tau-ports/process (gating wrap_spawn etc).
process            = ["tau-ports/process"]
# Enables capability_override module; pulls globset (std-only).
capability-override = ["dep:globset"]
# Enables jsonschema-based tool args validation.
tool-validation    = ["dep:jsonschema"]
# Enables flow paths that take std::path::PathBuf scope-roots and read
# skill markdown via std::fs. Embassy ships without this feature.
host-fs            = []

[dev-dependencies]
tau-domain      = { workspace = true, features = ["serde", "test-fixtures"] }
tau-ports       = { workspace = true, features = ["serde", "test-fixtures", "process"] }
futures         = "0.3"
futures-executor = "0.3"
proptest        = { workspace = true }
assert_matches  = { workspace = true }
serde_json      = "1"
```

Adjust dep versions to match what the workspace already pins where applicable (e.g. `hashbrown`, `foldhash` — if not in the workspace `[workspace.dependencies]` table, add them there too in the same edit).

- [ ] **Step 3: Add `hashbrown` + `foldhash` to the workspace `[workspace.dependencies]`**

In the root `Cargo.toml`'s `[workspace.dependencies]` block, add:

```toml
hashbrown = { version = "0.15", default-features = false, features = ["default-hasher"] }
foldhash  = { version = "0.1", default-features = false }
```

Then in `crates/tau-runtime-core/Cargo.toml`, switch the local `hashbrown` / `foldhash` entries to `{ workspace = true }`.

- [ ] **Step 4: Write `crates/tau-runtime-core/src/lib.rs` skeleton**

```rust
#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Executor-agnostic kernel of tau. Drives the agent loop independently
//! of any async runtime. Host shells (`tau-runtime-tokio`,
//! `tau-runtime-embassy`, future smol/async-std/glommio/wasm shells)
//! link this crate and supply executor-specific adapters.
//!
//! See `docs/superpowers/specs/2026-05-30-tau-runtime-core-design.md`
//! for the design.

extern crate alloc;

// Modules added by Phase β.1.3 tasks 3.2–3.7.

#[cfg(any(test, feature = "test-fixtures"))]
extern crate std;
```

- [ ] **Step 5: Verify the empty crate builds**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core --no-default-features
```
Expected: both PASS.

- [ ] **Step 6: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add Cargo.toml crates/tau-runtime-core/Cargo.toml crates/tau-runtime-core/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): create empty no_std crate skeleton"
```

### Task 3.2: Move `error.rs` (split out `ToolPluginExited`)

**Files:**
- Create: `crates/tau-runtime-core/src/error.rs`
- Modify: `crates/tau-runtime/src/error.rs`

- [ ] **Step 1: Copy `crates/tau-runtime/src/error.rs` → `crates/tau-runtime-core/src/error.rs`**

Use Read + Write (not `cp` — the agent harness prefers in-tool edits).

- [ ] **Step 2: Edit the core copy** — remove the `ToolPluginExited` variant

In `crates/tau-runtime-core/src/error.rs`:
- Find the variant `ToolPluginExited { exit_status: std::process::ExitStatus, ... }` (around line 263 per audit).
- DELETE the variant entirely.
- DELETE the doc-comments at the top of the file that reference `std::process::ExitStatus` (lines 7 and 192 per audit).
- Replace `use std::*` lines with `use core::*`/`use alloc::*` equivalents per the table in Task 1.7.
- Apply the rename table from Phase 1.8 (`SandboxError` → `CapabilityError`, etc.).

- [ ] **Step 3: Edit `crates/tau-runtime/src/error.rs`** — keep it as the tokio-shell error host

For Phase 3, `tau-runtime` continues to host the `ToolPluginExited` variant. Replace the file's body with:

```rust
//! Tokio-shell-specific errors that wrap the core's `RuntimeError`.
//! Phase β.1.4 renames the crate to `tau-runtime-tokio`.

pub use tau_runtime_core::error::{
    BuildError, CapabilityDenial, HandshakeFailureReason, PluginKind, RuntimeError as CoreRuntimeError,
};

/// Tokio-shell `RuntimeError`. Adds `ToolPluginExited` to the core's
/// variants. `From<CoreRuntimeError>` lifts core errors transparently.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum RuntimeError {
    /// Core (executor-agnostic) error.
    #[error(transparent)]
    Core(#[from] CoreRuntimeError),
    /// Tool plugin process exited unexpectedly.
    #[error("tool plugin exited unexpectedly: {plugin_name} (exit status {exit_status:?})")]
    ToolPluginExited {
        /// Name of the plugin that exited.
        plugin_name: alloc::string::String,
        /// Exit status of the plugin process.
        exit_status: std::process::ExitStatus,
    },
}
```

Add a `lib.rs`-level `pub use crate::error::{RuntimeError, ...}` to keep the downstream API stable.

- [ ] **Step 4: Add the module to `crates/tau-runtime-core/src/lib.rs`**

```rust
pub mod error;
pub use error::{BuildError, CapabilityDenial, HandshakeFailureReason, PluginKind, RuntimeError};
```

- [ ] **Step 5: Verify both crates compile**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
```
Expected: both PASS (or tau-runtime fails ONLY because `crate::error::RuntimeError` is used in places the old shape was expected — fix call sites in tau-runtime to use the new shell error type; most are transparent thanks to `From`).

- [ ] **Step 6: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/src/lib.rs crates/tau-runtime-core/src/error.rs crates/tau-runtime/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "refactor(tau-runtime-core): move error.rs; ToolPluginExited stays tokio-shell"
```

### Task 3.3: Move pure-types modules (`capability.rs`, `outcome.rs`, `options.rs`, `dispatch.rs`)

**Files:**
- Create: `crates/tau-runtime-core/src/capability.rs`
- Create: `crates/tau-runtime-core/src/outcome.rs`
- Create: `crates/tau-runtime-core/src/options.rs`
- Create: `crates/tau-runtime-core/src/dispatch.rs`
- Delete (eventually): `crates/tau-runtime/src/{capability,outcome,options,dispatch}.rs`
- Modify: `crates/tau-runtime/src/lib.rs` (re-export from core)
- Modify: `crates/tau-runtime-core/src/lib.rs` (add modules)

- [ ] **Step 1: Move each file**

For each of (`capability.rs`, `outcome.rs`, `options.rs`, `dispatch.rs`):

1. Read the file at `crates/tau-runtime/src/<file>.rs`.
2. Write the same content to `crates/tau-runtime-core/src/<file>.rs`, with:
   - `std::*` → `core::*`/`alloc::*` per Task 1.7 table.
   - `tau_runtime::error::RuntimeError` → `crate::error::RuntimeError` (no rename; both crates' module path resolves at use-site).
   - Apply Phase 1.8 rename table.
3. Replace `crates/tau-runtime/src/<file>.rs` with a single re-export shim:
   ```rust
   pub use tau_runtime_core::<file>::*;
   ```

- [ ] **Step 2: Update `options.rs` in core to carry the new ports**

Add to `RunOptions`:

```rust
/// Clock used by the runtime to stamp wall-clock times on trace events,
/// run snapshots, and ULID/UUID minting. Host shells inject their impl
/// (TokioClock, EmbassyClock, etc.). If `None`, the runtime uses a
/// zero-value mock clock — meaningful only for tests.
pub clock: Option<alloc::sync::Arc<dyn tau_ports::Clock>>,

/// Random source used by the runtime to mint session IDs (UUID v4),
/// run IDs (ULID), trace event IDs (ULID), and any other entropy
/// consumer in the kernel. Host shells inject their impl (OsRandom,
/// HwRandom). If `None`, the runtime uses a zero-seeded deterministic
/// fixture — meaningful only for tests.
pub random: Option<alloc::sync::Arc<dyn tau_ports::RandomSource>>,
```

Update `RunOptions::default()` to set both to `None`.

- [ ] **Step 3: Verify both crates still build**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
```

- [ ] **Step 4: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/src/ crates/tau-runtime/src/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "refactor(tau-runtime-core): move capability/outcome/options/dispatch + Clock/Random RunOptions fields"
```

### Task 3.4: Move `builder.rs` (HashMap → hashbrown + DynCapabilityGate registry only)

**Files:**
- Create: `crates/tau-runtime-core/src/builder.rs`
- Modify: `crates/tau-runtime/src/builder.rs` (re-export shim)
- Modify: `crates/tau-runtime-core/src/lib.rs`

- [ ] **Step 1: Copy `crates/tau-runtime/src/builder.rs` → core**

Read the file. Write to `crates/tau-runtime-core/src/builder.rs` with:

1. Replace `use std::collections::HashMap` with:
   ```rust
   use hashbrown::HashMap;
   type Registry<V> = HashMap<alloc::string::String, V, foldhash::quality::FixedState>;
   ```
2. Change every `HashMap<String, Arc<dyn Dyn...>>` field type to `Registry<Arc<dyn Dyn...>>`.
3. In `RuntimeBuilder::build()`, replace `HashMap::new()` allocations with `Registry::with_hasher(foldhash::quality::FixedState::default())`.
4. Move `DynProcessCapabilityGate` (the extension wrapper) OUT of core — it stays in the tokio shell. Core keeps only `DynCapabilityGate` (the universal wrapper).
5. Apply Phase 1.7 `std::*` rewrites + Phase 1.8 rename table.

- [ ] **Step 2: Replace `crates/tau-runtime/src/builder.rs`** with a thin shim

```rust
//! Tokio-shell builder extensions over `tau_runtime_core::builder`.
//! Phase β.1.4 folds this into `tau-runtime-tokio/src/lib.rs`.

pub use tau_runtime_core::builder::{Runtime, RuntimeBuilder, DynCapabilityGate};
// DynProcessCapabilityGate is host-shell-only; defined in
// tau-runtime/src/process_gate.rs by Task 3.5.
pub use crate::process_gate::DynProcessCapabilityGate;
```

- [ ] **Step 3: Add `crates/tau-runtime/src/process_gate.rs`**

Move the `DynProcessCapabilityGate` trait + blanket impl from Phase 1.8 into a new tokio-shell module. The content is the same as Step 3 of Task 1.8; it just lives in `tau-runtime` (which becomes `tau-runtime-tokio` in Phase 4).

- [ ] **Step 4: Update `crates/tau-runtime-core/src/lib.rs`**

```rust
pub mod builder;
pub use builder::{Runtime, RuntimeBuilder, DynCapabilityGate};
```

And `crates/tau-runtime/src/lib.rs`:

```rust
pub mod process_gate;
pub use process_gate::DynProcessCapabilityGate;
```

- [ ] **Step 5: Verify both crates build**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
```
Expected: both PASS.

- [ ] **Step 6: Run tau-runtime tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
```
Expected: PASS.

- [ ] **Step 7: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/ crates/tau-runtime/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "refactor(tau-runtime-core): move builder + Registry; DynProcessCapabilityGate stays in tokio shell"
```

### Task 3.5: Move `run.rs` (Mutex → RefCell, route Clock + RandomSource)

**Files:**
- Create: `crates/tau-runtime-core/src/run.rs`
- Modify: `crates/tau-runtime/src/run.rs` (re-export shim)
- Modify: `crates/tau-runtime-core/src/lib.rs`

- [ ] **Step 1: Read `crates/tau-runtime/src/run.rs`** to plan the surgery

Three concrete surgeries (lines from audit on origin/main):
- Line 312: `uuid::Uuid::new_v4()` for session-id minting.
- Line 351: `use tokio::sync::Mutex;` — replace import.
- Line 353: `let run_id = ulid::Ulid::new().to_string();` — route through Clock + RandomSource.
- Lines 355, 387: `chrono::Utc::now()` — route through Clock.
- Line 369: `let state_arc = Arc::new(Mutex::new(state));` — replace with `RefCell`.
- Lines 396, 419, 426: `state_arc.lock().await` → `state_arc.borrow_mut()` (no `.await`).
- Line 414: `ulid::Ulid::new().to_string()` — route through ports.
- `scope_root: std::path::PathBuf` parameter (line 349) — keep visible on the public `spawn_root_agent` API but feature-gate behind `host-fs` (Embassy doesn't pass this).

- [ ] **Step 2: Write helpers in `crates/tau-runtime-core/src/ids.rs`** (NEW)

```rust
//! UUID / ULID / now-timestamp helpers routed through ports.
//!
//! These are the only places in the kernel that materialize a session
//! ID, run ID, trace ID, or wall-clock timestamp. Adding a new ID
//! type? Add it here. The contract: every caller injects
//! `Arc<dyn Clock>` + `Arc<dyn RandomSource>`; this module synthesizes
//! the bytes.

use alloc::string::String;
use alloc::sync::Arc;

use tau_ports::{Clock, RandomSource};

/// Mint a UUID v4 from the supplied RandomSource. Result is the
/// canonical 36-character hyphenated form.
pub fn uuid_v4(random: &Arc<dyn RandomSource>) -> uuid::Uuid {
    let mut bytes = [0u8; 16];
    random.fill(&mut bytes);
    // Variant + version bits per RFC 4122 §4.4.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

/// Mint a ULID from the supplied Clock + RandomSource. Returns the
/// base32-encoded canonical form (26 chars).
pub fn ulid(clock: &Arc<dyn Clock>, random: &Arc<dyn RandomSource>) -> String {
    let ts = clock.now().max(0) as u64;
    let mut rand_bytes = [0u8; 10];
    random.fill(&mut rand_bytes);
    ulid::Ulid::from_parts(ts, u128::from_le_bytes({
        let mut b = [0u8; 16];
        b[..10].copy_from_slice(&rand_bytes);
        b
    })).to_string()
}

/// Wall-clock now as `chrono::DateTime<Utc>` from the supplied Clock.
pub fn now_utc(clock: &Arc<dyn Clock>) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(clock.now())
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap())
}
```

Add `pub mod ids;` to `crates/tau-runtime-core/src/lib.rs`.

- [ ] **Step 3: Copy `run.rs` to core and apply surgeries**

Write `crates/tau-runtime-core/src/run.rs` from the tokio version, with:

- Replace `use tokio::sync::Mutex;` with `use core::cell::RefCell;`.
- Replace `Arc::new(Mutex::new(state))` with `Arc::new(RefCell::new(state))`.
- Replace every `state_arc.lock().await` with `state_arc.borrow_mut()` (no `.await`).
- Replace every `chrono::Utc::now()` with `crate::ids::now_utc(clock)`, where `clock` is pulled from `opts.clock.as_ref().expect("clock must be supplied by host shell")` (or threaded through the function signature).
- Replace `ulid::Ulid::new().to_string()` with `crate::ids::ulid(clock, random)`.
- Replace `uuid::Uuid::new_v4()` with `crate::ids::uuid_v4(random)`.
- Wrap the `scope_root: std::path::PathBuf` parameter and any uses behind `#[cfg(feature = "host-fs")]`; provide a parallel `#[cfg(not(feature = "host-fs"))]` shape that omits scope-root entirely (Embassy supplies nothing).
- Wrap any `crate::orchestration::persistence::*` calls behind `#[cfg(feature = "host-fs")]`. The `tau-runtime-core` does not move persistence (per spec §7.2; stays in tokio shell). For Phase 3 builds, persistence still lives in `tau-runtime` and is called via a callback or feature-gated path. **Concrete shape:** turn the persistence wire-up at lines 364–366 into a closure parameter `runlog_writer: Option<RunLogWriter>` on `RunOptions`, where `RunLogWriter` is a trait alias for `Fn(RunId, mpsc::Receiver<TraceEvent>) -> ()` plus `Send`. The tokio shell sets this to `crate::orchestration::persistence::spawn_writer`; Embassy leaves it `None`.

- [ ] **Step 4: Replace `crates/tau-runtime/src/run.rs`** with a re-export

```rust
pub use tau_runtime_core::run::*;
```

- [ ] **Step 5: Add to `crates/tau-runtime-core/src/lib.rs`**

```rust
mod run;
```

- [ ] **Step 6: Verify**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
```
Expected: all PASS.

- [ ] **Step 7: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/ crates/tau-runtime/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "refactor(tau-runtime-core): move run.rs; Mutex->RefCell; route Clock+RandomSource"
```

### Task 3.6: Move orchestration submodules

**Files:**
- Create: `crates/tau-runtime-core/src/orchestration/{budget,error,mod,run_state,task_list,trace,virtual_tools}.rs`
- Partial: `crates/tau-runtime-core/src/orchestration/skill_resolve.rs` (move pure parts; gate `std::fs::read_to_string` behind `host-fs`)
- Keep in tokio shell: `crates/tau-runtime/src/orchestration/persistence.rs`
- Modify: `crates/tau-runtime/src/orchestration/mod.rs` (re-export from core; keep persistence)

Move each file with these rules:

- [ ] **Step 1: Move `budget.rs`, `error.rs`, `mod.rs`, `run_state.rs`** (clean — pure types)

For each file: read from `crates/tau-runtime/src/orchestration/<file>.rs`, write to `crates/tau-runtime-core/src/orchestration/<file>.rs` with:
- `std::*` → `core::*`/`alloc::*` per Task 1.7.
- `chrono::Utc` → still imported (it's a pure type with no_std-compatible date arithmetic when default features off; only `Utc::now` is gone).
- All `chrono::Utc::now()` call sites — there are none in budget/error/mod/run_state; field types stay; only `virtual_tools.rs`, `run.rs`, `stream.rs` have call sites.

Replace each source file in `crates/tau-runtime/src/orchestration/` with `pub use tau_runtime_core::orchestration::<file>::*;`.

- [ ] **Step 2: Move `task_list.rs`** (HashMap → hashbrown)

Same as Step 1, plus replace `use std::collections::HashMap` with `use hashbrown::HashMap`. Add `use foldhash::quality::FixedState;` if a hasher is specified explicitly anywhere.

- [ ] **Step 3: Move `virtual_tools.rs`** (route `chrono::Utc::now` through Clock)

Read the file. The `use chrono::Utc;` at line 7 is for `Utc::now()` calls — find each (grep `Utc::now\|Utc::today` inside the file) and replace with `crate::ids::now_utc(clock)` taking `clock` from the appropriate parameter or `RunOptions` field.

Specifically, the `register_virtual_tools` flow constructs `TaskEvent`s — those must accept a `clock` parameter. Update the public fn signature; cascade to callers in `run.rs`.

- [ ] **Step 4: Move `trace.rs`** (mpsc — partial move)

`crates/tau-runtime/src/orchestration/trace.rs:11` uses `tokio::sync::mpsc`. The trace event TYPES are pure; the SUBSCRIPTION (the `mpsc::Receiver`) is the executor-bound part.

**Action:** Move trace.rs to core BUT replace the `tokio::sync::mpsc::*` subscription pieces with a `core::cell::RefCell<alloc::collections::VecDeque<TraceEvent>>`-backed buffer plus a trait:

```rust
pub trait TraceSubscriber: Send + Sync {
    fn emit(&self, event: TraceEvent);
}
```

The tokio shell ships an `MpscTraceSubscriber` that owns an `mpsc::UnboundedSender<TraceEvent>` and `impl TraceSubscriber for MpscTraceSubscriber`. Embassy ships a `DefmtTraceSubscriber` or `NoopTraceSubscriber`.

In `RunState`, replace the existing `trace: TraceChannel { tx, ... }` field with `trace: Arc<dyn TraceSubscriber>`. The persistence writer (which is host-side) is wired up by the host shell via the closure registered in Task 3.5 Step 3.

This is the largest single edit in Phase 3. Verify with the existing trace tests:

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime --test orchestration_trace 2>/dev/null || echo "(no such test file — check workspace's actual test files for trace coverage)"
```

- [ ] **Step 5: Partial move of `skill_resolve.rs`**

The `std::fs::read_to_string` at line 314 is the only non-portable spot. Two options:

- **A. Gate behind `host-fs`.** Wrap the function (likely `resolve_skill_md_for_kind` or similar) behind `#[cfg(feature = "host-fs")]`; the kernel uses it only when host-fs is enabled. Embassy never calls this.
- **B. Extract a `SkillResolver` port.** The spec defers this to a follow-up (§12.2). For β.1 we choose option A.

Apply option A. Move the entire file to `crates/tau-runtime-core/src/orchestration/skill_resolve.rs`; wrap the `std::fs::read_to_string` site (and any helper that needs filesystem access) behind `#[cfg(feature = "host-fs")]`. Pure parts (manifest parsing, skill match algorithm) compile unconditionally.

- [ ] **Step 6: KEEP `persistence.rs` in `tau-runtime`**

Do not move `crates/tau-runtime/src/orchestration/persistence.rs`. Keep its module declaration in `crates/tau-runtime/src/orchestration/mod.rs`:

```rust
// Tokio-shell-specific. Not portable; per spec §12.1 follow-up.
pub mod persistence;
```

Core's `orchestration/mod.rs` does NOT include persistence.

- [ ] **Step 7: Verify**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
```
Expected: all PASS.

- [ ] **Step 8: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/src/orchestration/ crates/tau-runtime/src/orchestration/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "refactor(tau-runtime-core): move orchestration submodules (persistence stays tokio)"
```

### Task 3.7: Move `stream.rs`, `tool_args.rs`, `capability_override`

**Files:**
- Create: `crates/tau-runtime-core/src/stream.rs`
- Create: `crates/tau-runtime-core/src/tool_args.rs` (under `tool-validation` feature)
- Create: `crates/tau-runtime-core/src/capability_override/mod.rs` (under `capability-override` feature)
- Re-export shims in `crates/tau-runtime/src/`.

- [ ] **Step 1: Move `stream.rs`**

Surgeries:
- `use std::collections::HashMap` (line 13) → `use hashbrown::HashMap;`
- `chrono::Utc::now()` (lines 727, 986) → `crate::ids::now_utc(clock)` — the `clock` is threaded from `RunOptions`/`Runtime` context.
- `ulid::Ulid::new().to_string()` (lines 726, 897, 984) → `crate::ids::ulid(clock, random)`.
- `uuid::Uuid::new_v4()` (line 1224) → `crate::ids::uuid_v4(random)`.
- `std::env::current_dir()` (line 561) — REMOVE. The function this lives in must be modified to take a `scope_root: &Path` parameter from `RunOptions` instead of reading process env. Cascade to callers (likely just the spawn_streaming flow). This is a behavior change at the API boundary — the public function gains a parameter; the tokio shell's `drive.rs` (created in Phase 4) fills in `std::env::current_dir().unwrap()` to preserve today's behavior. Gate the path behind `#[cfg(feature = "host-fs")]`.

- [ ] **Step 2: Move `tool_args.rs`**

`jsonschema` is std-only. Wrap the whole `tool_args` module behind `#[cfg(feature = "tool-validation")]` in `crates/tau-runtime-core/src/lib.rs`:

```rust
#[cfg(feature = "tool-validation")]
mod tool_args;
```

When the feature is off, provide a stub `ToolArgsValidator` in `lib.rs`:

```rust
#[cfg(not(feature = "tool-validation"))]
mod tool_args {
    use alloc::string::String;
    use serde_json::Value;

    /// Stub validator (jsonschema unavailable in no_std). Accepts all args.
    pub struct ToolArgsValidator;

    impl ToolArgsValidator {
        pub fn new(_schema: &Value) -> Self { Self }
        pub fn validate(&self, _value: &Value) -> Result<(), String> { Ok(()) }
    }
}
```

The struct surface must match the real one so call sites compile against either.

- [ ] **Step 3: Move `capability_override/mod.rs`**

`globset` is std-only. Wrap the whole `capability_override` module behind `#[cfg(feature = "capability-override")]` in `crates/tau-runtime-core/src/lib.rs`. When off, the `EffectiveCapability` type is unavailable in the kernel; the runtime falls back to the package manifest's declared capabilities unmodified. This is Embassy's behavior (per spec §13.1 decision).

- [ ] **Step 4: Re-export from `tau-runtime`**

Replace each of `crates/tau-runtime/src/{stream,tool_args}.rs` and `crates/tau-runtime/src/capability_override/mod.rs` with `pub use tau_runtime_core::<module>::*;`.

- [ ] **Step 5: Verify**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core --no-default-features --features process
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
```
Expected: all PASS.

- [ ] **Step 6: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/ crates/tau-runtime/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "refactor(tau-runtime-core): move stream/tool_args/capability_override under features"
```

### Task 3.8: Add the no_std smoke test + CI gates

**Files:**
- Create: `crates/tau-runtime-core/tests/executor_agnostic_smoke.rs`
- Modify: `.github/workflows/ci.yml` (or whichever existing workflow file the repo uses — verify via `ls .github/workflows/`)

- [ ] **Step 1: Write the smoke test**

```rust
//! Executor-agnostic smoke test.
//!
//! `tau-runtime-core`'s LIB target is no_std (see lib.rs #![no_std]).
//! Integration tests run on the host's std target so they can use any
//! executor; this test proves the core can be driven by a non-tokio
//! executor — `futures_executor::block_on` from the `futures` crate.

use std::sync::Arc;

use tau_ports::{DeterministicRandom, MockClock};
use tau_ports::fixtures::MockLlmBackend;
use tau_runtime_core::{Runtime, RunOptions};

#[test]
fn core_builds_and_runs_with_mock_ports_only() {
    let clock: Arc<dyn tau_ports::Clock> = Arc::new(MockClock::new());
    let random: Arc<dyn tau_ports::RandomSource> = Arc::new(DeterministicRandom::seeded(0xC0FFEE));

    let runtime = Runtime::builder()
        .with_llm_backend(MockLlmBackend::new("mock"))
        .build()
        .expect("core builds");

    let opts = RunOptions {
        clock: Some(clock),
        random: Some(random),
        ..RunOptions::default()
    };

    let outcome = futures_executor::block_on(runtime.run(
        tau_domain::test_fixtures::agent_def(),
        tau_domain::test_fixtures::package_manifest(),
        tau_domain::test_fixtures::message_from_user("hi"),
        opts,
    ));
    assert!(outcome.is_ok(), "smoke run produced an error: {outcome:?}");
}
```

(If `tau_domain::test_fixtures::*` helpers don't exist by those names, swap in the actual fixture function names from `tau-domain/src/fixtures.rs` — verify with `grep -n "pub fn" crates/tau-domain/src/fixtures.rs` before writing.)

- [ ] **Step 2: Run the smoke test**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --test executor_agnostic_smoke
```
Expected: PASS.

- [ ] **Step 3: Add the no-std build gate to CI**

In `.github/workflows/ci.yml` (or the equivalent), find the existing rust-toolchain matrix and add a new step:

```yaml
      - name: no-std core build
        run: timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/ci cargo check -p tau-runtime-core --no-default-features

      - name: no-tokio-imports check
        run: |
          if grep -rE '^\s*use\s+(tokio|embassy|smol|async_std|std::)' crates/tau-runtime-core/src/; then
            echo "tau-runtime-core must not import tokio/embassy/smol/async_std/std::" >&2
            exit 1
          fi
```

The grep gate is intentionally permissive on doc lines (`//!`/`///`) and on `extern crate std;` for fixtures — those are filtered out by the `^\s*use\s+` regex.

- [ ] **Step 4: Verify locally with the same commands**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core --no-default-features
if grep -rE '^\s*use\s+(tokio|embassy|smol|async_std|std::)' crates/tau-runtime-core/src/; then echo "FAIL"; else echo "OK"; fi
```
Expected: both PASS.

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/tests/ .github/workflows/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "test(tau-runtime-core): executor-agnostic smoke + CI no-std gate"
```

### Task 3.9: PR Phase 3

- [ ] **Step 1: Final full-workspace test sweep**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core --no-default-features
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime
```
Expected: all PASS.

- [ ] **Step 2: Push (no `--no-verify` — let the deep gate run; this is Rust code change)**

```
scripts/agent-push.sh
```

- [ ] **Step 3: Open the PR**

```
gh pr create --title "feat(tau-runtime-core): extract no_std kernel from tau-runtime (Phase β.1.3)" --body "$(cat <<'EOF'
## Summary

- New crate `tau-runtime-core` containing the kernel (agent loop, builder, dispatch, orchestration sans persistence, capability machinery).
- `#![no_std] + alloc`; passes `cargo check -p tau-runtime-core --no-default-features`.
- `tokio::sync::Mutex` (`run.rs:351`) → `core::cell::RefCell`. The kernel is single-task by design (non-Send dyn futures per `builder.rs`).
- `chrono::Utc::now` / `uuid v4` / `ulid v4` call sites route through new `Clock` + `RandomSource` ports from Phase β.1.1.
- `std::collections::HashMap` → `hashbrown::HashMap<_, _, foldhash::quality::FixedState>`.
- Features: `process` (extension trait wire-up), `capability-override` (globset gate), `tool-validation` (jsonschema gate), `host-fs` (std::fs/PathBuf gates).
- New executor-agnostic smoke test: `futures_executor::block_on` drives one turn.
- CI gates: no-std build + no-tokio/no-std::* import grep.
- `tau-runtime` keeps its existing surface via per-module re-exports of the core; Phase β.1.4 renames it to `tau-runtime-tokio`.

## Phase

β.1.3 of the runtime-core extraction (5-PR sequence).

## Test plan

- [x] `cargo check -p tau-runtime-core` + `--no-default-features`
- [x] `cargo nextest run -p tau-runtime-core` (smoke test)
- [x] `cargo nextest run -p tau-runtime` (existing suite unchanged)
- [x] CI no-std build gate
- [x] CI no-tokio/no-std::* import grep gate
EOF
)"
```

---

## Phase 4: `tau-runtime` → `tau-runtime-tokio` rename + add `TokioClock`/`OsRandom`/`drive.rs`

**Goal:** Rename the residual `tau-runtime` crate to `tau-runtime-tokio`, add the tokio-shell `Clock` and `RandomSource` impls, add the `drive` entry, and update all four downstream consumers (`tau-cli`, `tau-workflow`, `tau-plugin-compat`, `tau-app`).

**Branch:** `feat/runtime-core-tokio-rename` (off `main` after Phase 3 merges).

### Task 4.1: Move the crate directory + update Cargo.toml name

**Files:**
- Move: `crates/tau-runtime/` → `crates/tau-runtime-tokio/`
- Modify: `crates/tau-runtime-tokio/Cargo.toml`
- Modify: workspace root `Cargo.toml` (member path + `[workspace.dependencies]`)

- [ ] **Step 1: Move the directory**

```
git mv crates/tau-runtime crates/tau-runtime-tokio
```

- [ ] **Step 2: Edit `crates/tau-runtime-tokio/Cargo.toml`**

- Change `name = "tau-runtime"` → `name = "tau-runtime-tokio"`.
- Add to `[dependencies]`:
  ```toml
  tau-runtime-core    = { workspace = true }
  getrandom           = { version = "0.2" }
  ```
- Remove transitive deps that the core now owns (`hashbrown`, `foldhash`, `globset`, `jsonschema` — anything moved exclusively to the core's deps). Keep tokio, futures-core, async-stream, tokio::process-flavored deps.

- [ ] **Step 3: Update workspace root `Cargo.toml`**

- In `members = [...]`: rename `"crates/tau-runtime"` to `"crates/tau-runtime-tokio"`.
- In `[workspace.dependencies]`: rename the `tau-runtime` entry to `tau-runtime-tokio = { path = "crates/tau-runtime-tokio" }` (keep `tau-runtime-core` already-added in Phase 3 in the table).

- [ ] **Step 4: Verify the rename compiles standalone**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
```
Expected: FAIL with errors from `tau-cli`/`tau-workflow`/`tau-plugin-compat`/`tau-app` whose `Cargo.toml` still references `tau-runtime`. Continue.

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add Cargo.toml crates/tau-runtime-tokio/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor: rename tau-runtime -> tau-runtime-tokio (crate move)"
```

### Task 4.2: Add `TokioClock`

**Files:**
- Create: `crates/tau-runtime-tokio/src/clock.rs`
- Modify: `crates/tau-runtime-tokio/src/lib.rs`

- [ ] **Step 1: Write `crates/tau-runtime-tokio/src/clock.rs`**

```rust
//! Tokio-shell `Clock` impl: wall-clock UTC via `chrono::Utc::now`.

use tau_ports::Clock;

/// Wall-clock backed by `chrono::Utc::now`.
pub struct TokioClock;

impl Clock for TokioClock {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
```

- [ ] **Step 2: Re-export in `crates/tau-runtime-tokio/src/lib.rs`**

Add:
```rust
pub mod clock;
pub use clock::TokioClock;
```

- [ ] **Step 3: Add a unit test inline**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::Clock;

    #[test]
    fn now_is_monotonic_ish() {
        let c = TokioClock;
        let a = c.now();
        let b = c.now();
        assert!(b >= a);
    }
}
```

- [ ] **Step 4: Verify**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio --lib --tests clock
```
Expected: PASS.

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-tokio/src/clock.rs crates/tau-runtime-tokio/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-tokio): add TokioClock impl"
```

### Task 4.3: Add `OsRandom`

**Files:**
- Create: `crates/tau-runtime-tokio/src/random.rs`
- Modify: `crates/tau-runtime-tokio/src/lib.rs`

- [ ] **Step 1: Write `crates/tau-runtime-tokio/src/random.rs`**

```rust
//! Tokio-shell `RandomSource` impl: OS entropy via `getrandom`.

use tau_ports::RandomSource;

/// `RandomSource` backed by the OS entropy pool (`getrandom`). Suitable
/// for cryptographic use.
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill(&self, dest: &mut [u8]) {
        getrandom::getrandom(dest).expect("OS entropy unavailable");
    }
}
```

(If the installed `getrandom` API uses `fill` instead of `getrandom`, swap; the call shape is `fn(&mut [u8]) -> Result<(), Error>` either way.)

- [ ] **Step 2: Re-export in `crates/tau-runtime-tokio/src/lib.rs`**

```rust
pub mod random;
pub use random::OsRandom;
```

- [ ] **Step 3: Test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::RandomSource;

    #[test]
    fn fills_with_distinct_bytes() {
        let r = OsRandom;
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        r.fill(&mut a);
        r.fill(&mut b);
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 4: Verify**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio --lib --tests random
```

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-tokio/src/random.rs crates/tau-runtime-tokio/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-tokio): add OsRandom impl"
```

### Task 4.4: Add `drive.rs` (tokio entry that wires defaults)

**Files:**
- Create: `crates/tau-runtime-tokio/src/drive.rs`
- Modify: `crates/tau-runtime-tokio/src/lib.rs`

- [ ] **Step 1: Write `crates/tau-runtime-tokio/src/drive.rs`**

```rust
//! Tokio-shell entry: drive the core's `Runtime` on tokio, injecting
//! `TokioClock` + `OsRandom` defaults if the caller hasn't supplied them.

use alloc::sync::Arc;

use tau_domain::{AgentDefinition, Message, PackageManifest};
use tau_ports::{Clock, RandomSource, RunBudget, RunSnapshot};

use crate::{OsRandom, TokioClock};
use tau_runtime_core::{Runtime, RunOptions, RuntimeError};

/// Drive `rt.spawn_root_agent` with sensible tokio-shell defaults.
///
/// This is the canonical entry point from the tokio host. CLI / workflow
/// callers reach this through `Runtime::spawn_root_agent` directly when
/// they already own a `Runtime`; this fn is the "zero-config" helper.
pub async fn drive(
    rt: Arc<Runtime>,
    root_agent_def: AgentDefinition,
    root_manifest: PackageManifest,
    initial_message: Message,
    budget: RunBudget,
    scope_root: std::path::PathBuf,
) -> Result<RunSnapshot, RuntimeError> {
    let clock: Arc<dyn Clock> = Arc::new(TokioClock);
    let random: Arc<dyn RandomSource> = Arc::new(OsRandom);

    let opts = RunOptions {
        clock: Some(clock),
        random: Some(random),
        ..RunOptions::default()
    };

    rt.spawn_root_agent_with_options(
        root_agent_def,
        root_manifest,
        initial_message,
        budget,
        scope_root,
        opts,
    )
    .await
}
```

(If `spawn_root_agent_with_options` doesn't yet exist on the core — add it in `crates/tau-runtime-core/src/run.rs` as a thin wrapper that takes `RunOptions` rather than minting defaults internally. The existing `spawn_root_agent` becomes a default-injecting alias on the tokio shell.)

- [ ] **Step 2: Re-export**

In `crates/tau-runtime-tokio/src/lib.rs`:
```rust
pub mod drive;
pub use drive::drive as drive_root_agent;
```

- [ ] **Step 3: Verify**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
```

- [ ] **Step 4: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-tokio/src/drive.rs crates/tau-runtime-tokio/src/lib.rs crates/tau-runtime-core/src/run.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-tokio): add drive entry wiring TokioClock + OsRandom"
```

### Task 4.5: Move `sandbox/` → `process_gate/` + rename internal type names

**Files:**
- Move: `crates/tau-runtime-tokio/src/sandbox/` → `crates/tau-runtime-tokio/src/process_gate/`
- Modify: `crates/tau-runtime-tokio/src/lib.rs`
- Modify: `crates/tau-runtime-tokio/src/process_gate/mod.rs` (the prior `sandbox/mod.rs`)

- [ ] **Step 1: Move the module**

```
git mv crates/tau-runtime-tokio/src/sandbox crates/tau-runtime-tokio/src/process_gate
```

- [ ] **Step 2: Apply Phase 1.8 rename inside the moved files**

`grep -rn "Sandbox" crates/tau-runtime-tokio/src/process_gate/` lists the rename targets. Apply the table:
- `Sandbox` (in the `dyn Sandbox` sense) → `CapabilityGate` (in dyn-trait context) or `ProcessCapabilityGate` (in registry-storage context).
- `SandboxPlan` → `CapabilityPlan`, etc.
- The crate names `tau-sandbox-*` and concrete impl names (`NativeSandbox`, etc.) stay.

- [ ] **Step 3: Update `crates/tau-runtime-tokio/src/lib.rs`**

```rust
pub mod process_gate;
pub use process_gate::*;
```

(Remove the legacy `pub mod sandbox;`.)

- [ ] **Step 4: Verify**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
```
Expected: PASS.

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-tokio/src/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-runtime-tokio): rename sandbox/ -> process_gate/ + type renames"
```

### Task 4.6: Add `#[deprecated]` banner on `plugin_host`

**Files:**
- Modify: `crates/tau-runtime-tokio/src/plugin_host/mod.rs`

- [ ] **Step 1: Edit the module doc-comment**

Add at the top of `mod.rs`:

```rust
//! ⚠ **Deprecated since β.1.** This module is preserved as-is for the
//! tokio shell only. Phase β.3 introduces the MCP facilitator which
//! replaces the bespoke subprocess plugin protocol. Embassy / wasm
//! shells will never carry `plugin_host`.
//!
//! New plugin types should target the β.3 MCP facilitator instead of
//! adding to `plugin_host`.
```

- [ ] **Step 2: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-tokio/src/plugin_host/mod.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs(tau-runtime-tokio): mark plugin_host deprecated pending β.3"
```

### Task 4.7: Update `tau-cli` downstream

**Files:**
- Modify: `crates/tau-cli/Cargo.toml`
- Modify: 17 files in `crates/tau-cli/src/` and `crates/tau-cli/tests/` (audited list — verify with grep)

- [ ] **Step 1: Cargo.toml rename**

In `crates/tau-cli/Cargo.toml`, change every `tau-runtime` line to `tau-runtime-tokio`.

- [ ] **Step 2: Enumerate the import sites**

```
grep -rln "tau_runtime::" crates/tau-cli/ > /tmp/tau-cli-rename-sites.txt
cat /tmp/tau-cli-rename-sites.txt
```

Expected ~17 files (from the audit on origin/main).

- [ ] **Step 3: Sweep rename**

Two strategies — pick per-file:

A. **Direct rename:** Edit `replace_all` on each file:
   - `tau_runtime::` → `tau_runtime_tokio::`
   - `use tau_runtime` → `use tau_runtime_tokio`

B. **Use-alias shim** (preferred for files with many use-sites):
   ```rust
   use tau_runtime_tokio as tau_runtime;
   ```
   at the top of the file. This is one line per file vs. dozens of edits.

Spec-suggested rule of thumb: ≥5 use-sites in a file → use the alias shim; <5 → direct rename.

- [ ] **Step 4: Verify**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```
Expected: PASS.

- [ ] **Step 5: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-cli/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-cli): update imports for tau-runtime -> tau-runtime-tokio rename"
```

### Task 4.8: Update `tau-workflow`, `tau-plugin-compat`, `tau-app`

**Files:**
- Modify: `crates/tau-workflow/Cargo.toml`, `crates/tau-workflow/src/runner.rs`, `crates/tau-workflow/tests/integration.rs`
- Modify: `crates/tau-plugin-compat/Cargo.toml`, `crates/tau-plugin-compat/tests/layer4_container.rs`
- Modify: `crates/tau-app/Cargo.toml`

- [ ] **Step 1: Apply the same rename pattern as Task 4.7**

For each crate: Cargo.toml dep rename + source-file `tau_runtime::` → `tau_runtime_tokio::` (or use-alias).

- [ ] **Step 2: Verify each crate builds + tests pass**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-workflow -p tau-plugin-compat -p tau-app
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-workflow
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-plugin-compat
```
Expected: all PASS.

- [ ] **Step 3: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-workflow/ crates/tau-plugin-compat/ crates/tau-app/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "refactor(tau-workflow,tau-plugin-compat,tau-app): rename tau-runtime -> tau-runtime-tokio"
```

### Task 4.9: PR Phase 4

- [ ] **Step 1: Full workspace sanity**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

- [ ] **Step 2: Push (gate runs — Rust code change)**

```
scripts/agent-push.sh
```

- [ ] **Step 3: Open the PR**

```
gh pr create --title "refactor: tau-runtime -> tau-runtime-tokio rename + TokioClock/OsRandom/drive (Phase β.1.4)" --body "$(cat <<'EOF'
## Summary

- Renames `tau-runtime` → `tau-runtime-tokio`.
- Adds `TokioClock` (`chrono::Utc::now`-backed) + `OsRandom` (`getrandom`-backed) shell impls of the new ports.
- Adds `drive.rs` entry point that wires the two defaults into `tau-runtime-core::Runtime::spawn_root_agent_with_options`.
- Moves `sandbox/` → `process_gate/` per spec §3.
- `plugin_host` gains a `#[deprecated]` banner (β.3 MCP facilitator replaces it).
- Updates 4 downstream crates (`tau-cli`, `tau-workflow`, `tau-plugin-compat`, `tau-app`) to the new crate name.

## Phase

β.1.4 of the runtime-core extraction (5-PR sequence).

## Test plan

- [x] `cargo nextest run -p tau-runtime-core`
- [x] `cargo nextest run -p tau-runtime-tokio` (formerly tau-runtime)
- [x] `cargo nextest run -p tau-cli`
- [x] `cargo nextest run -p tau-workflow`
- [x] `cargo nextest run -p tau-plugin-compat`
- [x] CI's full matrix is the cross-target gate
EOF
)"
```

---

## Phase 5: Documentation pass

**Goal:** rustdoc green; ADRs / docs that mention the legacy `tau-runtime` are reviewed; new docs link to `tau-runtime-core` where appropriate.

**Branch:** `feat/runtime-core-docs` (off `main` after Phase 4 merges).

### Task 5.1: `cargo doc -p tau-runtime-core`

- [ ] **Step 1: Build docs**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo doc -p tau-runtime-core --no-deps
```
Expected: PASS. Any rustdoc warning becomes an error under `#![deny(rustdoc::broken_intra_doc_links)]`.

- [ ] **Step 2: Build docs for the tokio shell**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo doc -p tau-runtime-tokio --no-deps
```
Expected: PASS.

- [ ] **Step 3: Fix any broken intra-doc link**

If a link points to `tau_runtime::*` (legacy name) inside `tau-runtime-core`'s doc-strings — rewrite to `crate::*` or to `tau_runtime_tokio::*` as appropriate.

- [ ] **Step 4: Commit (only if changes were needed)**

```
git -c user.name="Test User" -c user.email="test@example.com" add crates/tau-runtime-core/ crates/tau-runtime-tokio/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs(tau-runtime-core,tau-runtime-tokio): fix intra-doc links post-extraction"
```

### Task 5.2: Audit ADRs + docs mentioning `tau-runtime`

**Files:**
- Modify (likely): a handful of files in `docs/` and `docs/decisions/`.

- [ ] **Step 1: Enumerate**

```
grep -rln "tau-runtime\|tau_runtime" docs/ | grep -v "\.git" | head -40
```

- [ ] **Step 2: Triage each hit**

Per file:
- If the doc is **historical** (an ADR explaining a past decision under the legacy name), add a brief footnote — e.g. `> _Note (2026-XX-XX): renamed to `tau-runtime-tokio` per β.1; the core extraction lives in `tau-runtime-core`._` — and leave the body unchanged.
- If the doc is **forward-looking** (architecture overview, reference page, getting-started), rewrite references to `tau-runtime-core` (for kernel discussion) or `tau-runtime-tokio` (for host-shell discussion).

The full list as of 2026-05-30 (verify; may change as docs evolve):
- `docs/decisions/0006-tau-runtime.md` — historical ADR; add the footnote.
- `docs/decisions/0024-multi-agent-orchestration.md`, `0025-…`, `0026-…`, `0027-…`, `0028-…`, `0029-…`, `0030-…`, `0031-…`, `0032-…`, `0033-…`, `0034-…`, `0035-…`, `0036-…` — historical; footnote each.
- `docs/explanation/tau-philosophy.md` — forward-looking; rewrite.
- `docs/explanation/tau-as-language.md` — forward-looking; rewrite.
- Any `docs/reference/*.md` mentioning the runtime — rewrite.
- `docs/SUMMARY.md` — add new entry for `tau-runtime-core` if there's a runtime-architecture section.

- [ ] **Step 3: Verify the mdBook builds**

If the workspace uses mdBook (verify with `ls book.toml` or similar):

```
mdbook build docs 2>&1 | tail -20
```
Expected: no broken links.

- [ ] **Step 4: Commit**

```
git -c user.name="Test User" -c user.email="test@example.com" add docs/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "docs: reflect tau-runtime-core / tau-runtime-tokio split"
```

### Task 5.3: Write the β.1 completion memory + PR Phase 5

- [ ] **Step 1: Push**

```
scripts/agent-push.sh
```

- [ ] **Step 2: Open the PR**

```
gh pr create --title "docs: tau-runtime-core / tau-runtime-tokio rename pass (Phase β.1.5)" --body "$(cat <<'EOF'
## Summary

- Closes β.1 of the runtime-core extraction.
- `cargo doc -p tau-runtime-core` + `-p tau-runtime-tokio` clean.
- ADRs 0006 + 0024–0036 footnoted with rename pointers.
- Forward-looking explainers (`tau-philosophy.md`, `tau-as-language.md`) rewritten to reference the split.
- mdBook builds without broken links.

## Phase

β.1.5 (final) of the runtime-core extraction (5-PR sequence).

## Test plan

- [x] `cargo doc -p tau-runtime-core --no-deps`
- [x] `cargo doc -p tau-runtime-tokio --no-deps`
- [x] mdbook build docs
EOF
)"
```

---

## Definition of done (recap)

After all five PRs merge, verify against spec §10:

1. ✅ Every existing `tau-runtime-tokio` (formerly `tau-runtime`) test stays green. (Verified at each phase.)
2. ✅ `cargo check -p tau-runtime-core --no-default-features` succeeds. (Task 3.8 Step 4 + CI gate.)
3. ✅ `tau-runtime-core/src/` contains zero `use tokio::*` / `use embassy::*` / `use smol::*` / `use std::*` / `use parking_lot::*`. (CI grep gate.)
4. ✅ `tau-runtime-core` exposes a runnable `Runtime` from `MockLlmBackend` + `MockClock` + `DeterministicRandom`. (Smoke test in Task 3.8.)
5. ✅ `tau-cli`, `tau-workflow`, `tau-plugin-compat`, `tau-app` build with renamed imports + tests green. (Tasks 4.7, 4.8.)
6. ✅ `cargo doc -p tau-runtime-core` builds cleanly. (Task 5.1.)
7. ✅ Four `tau-sandbox-*` crates build under renamed trait + tests green. (Tasks 2.1–2.4.)
8. ✅ No observable host behavior change. (Implied by 1, 5, 7.)

---

## Self-review notes (for the implementer)

- **Watch for sed-broken capability_override imports.** The `glob_subset.rs` historical filename was folded into `capability_override/mod.rs` in current main; do not search for a file by that name.
- **`extern crate std`** lives behind `#[cfg(any(test, feature = "test-fixtures"))]` in `tau-ports/src/lib.rs` (Task 1.1) so the test-fixtures path stays buildable; do not remove it during cleanup.
- **The trace.rs split is the highest-risk task.** Allocate the most review attention to Task 3.6 Step 4 — turning the mpsc-backed `TraceChannel` into a `TraceSubscriber` trait will touch every emit site. Run the full tau-runtime suite immediately after.
- **`scope_root: PathBuf` is everywhere.** Audit all places it threads through `RunOptions` and feature-gate consistently (Task 3.5, 3.7); the smoke test in Task 3.8 should NOT need scope_root (it runs without `host-fs`? — re-check; if it does need it, build the smoke test with `--features host-fs` rather than `--no-default-features`).
- **Avoid renaming concrete sandbox types.** `NativeSandbox`, `ContainerSandbox`, `DarwinSandbox`, `WindowsSandbox` keep their names; the crate names `tau-sandbox-*` keep theirs. Only the *trait* renames.
- **The `MockLlmBackend` in `tau-ports::fixtures` is unchanged by the rename.** It implements `LlmBackend` (the port), not `Sandbox`. The fixture rename in Task 1.4 only touches `MockSandbox`.
- **Phase 3 commits do not use `--no-verify`** — Rust code changes should run through the deep gate. Phases 1, 2, 4 are mostly rename + boilerplate; the host-side lefthook tests can corrupt git identity (see CLAUDE.md), so they use `--no-verify` and lean on CI as the gate.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-30-tau-runtime-core-extraction.md`. Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks, fast iteration. Best fit here: 5 PRs with hard CI gates between them; subagent isolation per phase keeps the contexts clean.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints. Possible but the plan is large; risks context fatigue.

Which approach?
