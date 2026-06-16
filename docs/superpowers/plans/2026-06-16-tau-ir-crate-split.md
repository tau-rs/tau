# tau-ir Crate Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `tau-ir` into a pure no_std IR crate and a new std-side `tau-ir-lower` crate, so `tau-runtime-core` is structurally incapable of linking `tau-pkg` and builds for `wasm32-wasip2`.

**Architecture:** Move the `lower/` module (`ProjectConfig → IrModule`, the only `tau-pkg` user) and the `IrError` type (renamed `LowerError`, entirely a lowering concern) out of `tau-ir` into `tau-ir-lower`. Pure `tau-ir` keeps IR types + interpreter support and becomes genuinely no_std. Done by copy → repoint consumers → strip, so every commit compiles.

**Tech Stack:** Rust, hexagonal workspace, resolver-2 Cargo workspace, `wasm32-wasip2` + `wasm32-unknown-unknown` targets, `tau-pkg`/`serde_json`/`globset`.

**Spec:** `docs/superpowers/specs/2026-06-16-tau-ir-crate-split-design.md`

---

## Context the implementer needs

- This branch already landed the **SkillResolver port** (commits `7c93ada`, `55857d2`, `1024b15`, `bccbf0c`, `2340362`) which removed core's *direct* `tau-pkg` dep. This plan removes the *transitive* one via `tau-ir`. Do not touch the SkillResolver work.
- **Verified facts** (the plan relies on these):
  - `IrError` is referenced only in `tau-ir/src/error.rs` (def), `lib.rs` (re-export), and `tau-ir/src/lower/` (all constructions). The one out-of-crate **type** reference is `tau-cli/src/cmd/build.rs:356` (`Option<tau_ir::error::IrError>`). All other workspace mentions are comments / `Display` strings.
  - `lower/` references these pure modules (which STAY in `tau-ir`): `ids, module, capability, tool_impl, pipeline, context, node, check, subflow, trigger, template`. It also references `crate::error` (the moving `IrError`) and `crate::lower` (its own submodules).
  - `lower/` files: `mod.rs, parse.rs, resolve.rs, typecheck.rs, capability_fit.rs, mcp_build_error.rs`.
  - Pure `tau-ir` already has its own `TemplateError` (in `template.rs`) — that STAYS; only `IrError` moves.
  - The `#[cfg(feature = "with-std-adapters")] impl From<...> for Message` blocks in `message.rs:163,183` (the `SystemTime` envelope adapters) have **zero** real consumers (grep-clean) — delete them.
  - Lowering consumers (use `tau_ir::lower::`): `tau-cli` (`cmd/build.rs`, `cmd/run.rs`, `cmd/dev/session.rs`), `tau-conformance` (`scenario.rs`), `tau-ir-conformance` (`bundle_mode.rs`, `dev_mode.rs`).
  - Pure-IR-only consumers (NO change): `tau-runtime-core`, `tau-runtime-tokio`, `tau-ts-extract`.

## CARGO RULES (CLAUDE.md — every cargo call)

`timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`. Never bare cargo. Never omit `-p` (except the explicit `--workspace` checks called out below, which still set `CARGO_TARGET_DIR`/`CARGO_INCREMENTAL`). Timeouts: test 300, build/check 180, clippy 240, fmt 30. Cross-target builds: up to 420s first time. Work in `/Users/titouanlebocq/code/tau-worktrees/beta-7-5-skillresolver`. Reviewers use `CARGO_TARGET_DIR=target/agent-review`.

## File Structure (after the split)

- `crates/tau-ir/` — pure: `budget, canonical, capability, check, context, hash, ids, message (payload conv only), module, node, pipeline, subflow, template, tool_impl, trigger`. No `lower`, no `error`(IrError), no `tau-pkg`, no `with-std-adapters`.
- `crates/tau-ir-lower/` — new: `lower/` (6 files) + `error.rs` (`LowerError`). Depends on `tau-ir` + `tau-pkg` + `tau-ports`.

---

### Task 1: Scaffold the `tau-ir-lower` crate

**Files:**
- Create: `crates/tau-ir-lower/Cargo.toml`
- Create: `crates/tau-ir-lower/src/lib.rs`
- Modify: workspace root `Cargo.toml` (members list)

- [ ] **Step 1: Confirm the workspace members list + how members are declared**

Run: `grep -n "members\|crates/tau-ir" Cargo.toml | head`
Expected: a `[workspace] members = [...]` array (likely a glob like `"crates/*"`). If it's a glob `crates/*`, no edit is needed; if it's an explicit list, you'll add `"crates/tau-ir-lower"` in Step 3.

- [ ] **Step 2: Create `crates/tau-ir-lower/Cargo.toml`**

```toml
[package]
name = "tau-ir-lower"
description = "Std-side lowering for the tau workflow IR: tau_pkg::ProjectConfig → tau_ir::IrModule. Split out of tau-ir so the pure IR crate (and tau-runtime-core) stay no_std / wasm-buildable."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[dependencies]
tau-ir     = { workspace = true }
tau-domain = { workspace = true, default-features = false, features = ["serde"] }
tau-ports  = { workspace = true, default-features = false, features = ["serde"] }
tau-pkg    = { workspace = true }
serde      = { workspace = true, default-features = false, features = ["alloc", "derive"] }
serde_json = { workspace = true }
chrono     = { workspace = true, default-features = false, features = ["alloc", "serde"] }
thiserror  = { workspace = true, default-features = false }
hashbrown  = { workspace = true }
foldhash   = { workspace = true }
sha2       = { workspace = true, default-features = false }

[dev-dependencies]
tau-domain = { workspace = true, features = ["serde", "test-fixtures"] }
tau-pkg    = { workspace = true }
tau-ports  = { workspace = true, features = ["serde"] }
```

NOTE: this mirrors `tau-ir`'s current dep set (so the moved `lower/` code keeps compiling). You will trim anything genuinely unused at the end of Task 2 if clippy/cargo flags it. `serde_json` keeps std default here — this crate is std-side.

- [ ] **Step 3: Create `crates/tau-ir-lower/src/lib.rs` (placeholder)**

```rust
//! Std-side lowering for the tau workflow IR.
//!
//! Holds the `lower` pass (`tau_pkg::ProjectConfig` → `tau_ir::IrModule`)
//! and the `LowerError` type. Split out of `tau-ir` (β.7.5) so the pure
//! IR crate and `tau-runtime-core` stay no_std and wasm-buildable.
```

If the workspace `members` is an explicit list (not a `crates/*` glob), add `"crates/tau-ir-lower"` to it now.

- [ ] **Step 4: Build the empty crate**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir-lower`
Expected: PASS (empty lib).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir-lower/Cargo.toml crates/tau-ir-lower/src/lib.rs Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-ir-lower): scaffold std-side lowering crate"
```

---

### Task 2: Copy `lower/` + `error.rs` into `tau-ir-lower`, rename `IrError` → `LowerError`, repoint imports

This COPIES (does not yet delete) so `tau-ir` stays intact and the workspace stays green. Deletion is Task 4.

**Files:**
- Create: `crates/tau-ir-lower/src/error.rs` (from `tau-ir/src/error.rs`, renamed type)
- Create: `crates/tau-ir-lower/src/lower/{mod,parse,resolve,typecheck,capability_fit,mcp_build_error}.rs` (from `tau-ir/src/lower/`)
- Modify: `crates/tau-ir-lower/src/lib.rs` (declare modules + re-exports)

- [ ] **Step 1: Copy the files**

```bash
cp crates/tau-ir/src/error.rs crates/tau-ir-lower/src/error.rs
mkdir -p crates/tau-ir-lower/src/lower
cp crates/tau-ir/src/lower/*.rs crates/tau-ir-lower/src/lower/
```

- [ ] **Step 2: Wire `tau-ir-lower/src/lib.rs`**

Replace the placeholder body with:

```rust
//! Std-side lowering for the tau workflow IR.
//!
//! Holds the `lower` pass (`tau_pkg::ProjectConfig` → `tau_ir::IrModule`)
//! and the `LowerError` type. Split out of `tau-ir` (β.7.5) so the pure
//! IR crate and `tau-runtime-core` stay no_std and wasm-buildable.

extern crate alloc;

pub mod error;
pub mod lower;

pub use error::LowerError;
pub use lower::McpBuildError;
```

(Add any further re-exports the consumers need — e.g. `ResolvedServerTool`, the `lower` entry-point fn. After the consumer repoint in Task 3, the compiler will name anything missing; add it here. Inspect `tau-ir/src/lib.rs:30` and `lower/mod.rs` for the current public lowering surface and mirror it.)

- [ ] **Step 3: Rename the error type in the copied `error.rs`**

In `crates/tau-ir-lower/src/error.rs`, rename the enum `IrError` → `LowerError` (the `pub enum IrError {` line). Keep ALL variants unchanged. The `McpBuild(#[from] crate::lower::McpBuildError)` variant now resolves correctly because `crate::lower` exists in THIS crate. If `error.rs` has `use` statements pointing at `crate::ids::*` / `crate::module::*` etc. (pure modules), rewrite those to `tau_ir::...` (see Step 4's mapping).

- [ ] **Step 4: Rewrite imports in the copied `lower/` + `error.rs` files**

Apply these path rewrites in `crates/tau-ir-lower/src/error.rs` and all `crates/tau-ir-lower/src/lower/*.rs`:

| From | To |
|---|---|
| `IrError` (the type, everywhere) | `LowerError` |
| `crate::error::IrError` | `crate::error::LowerError` (covered by the line above) |
| `crate::ids` | `tau_ir::ids` |
| `crate::module` | `tau_ir::module` |
| `crate::capability` | `tau_ir::capability` |
| `crate::tool_impl` | `tau_ir::tool_impl` |
| `crate::pipeline` | `tau_ir::pipeline` |
| `crate::context` | `tau_ir::context` |
| `crate::node` | `tau_ir::node` |
| `crate::check` | `tau_ir::check` |
| `crate::subflow` | `tau_ir::subflow` |
| `crate::trigger` | `tau_ir::trigger` |
| `crate::template` | `tau_ir::template` |
| `crate::error` | `crate::error` (UNCHANGED — `LowerError` is local) |
| `crate::lower` | `crate::lower` (UNCHANGED — lower submodules are local) |

There are ~4 bare `crate::{ ... }` multi-import lines — expand each by hand per the table (some items may be pure → `tau_ir::`, `error` → stays `crate::`).

Do this with care, then let the compiler find stragglers. Suggested: targeted `sed` per row for the unambiguous `crate::<module>` → `tau_ir::<module>` rows, then a global `IrError` → `LowerError` in these files only, then hand-fix the bare `crate::{...}` lines.

- [ ] **Step 5: Build `tau-ir-lower` standalone, iterate on compile errors**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir-lower`
Expected initially: FAILS with unresolved-path errors → fix each by applying the Step-4 mapping (or adding a missing re-export to `lib.rs` Step 2). Common fixes:
- A pure type used unqualified → import it from `tau_ir` (e.g. `use tau_ir::module::IrModule;`).
- `lower/mod.rs`'s `use tau_pkg::project::ProjectConfig;` and `use tau_ports::target::TargetTriple;` — these STAY (both are deps of `tau-ir-lower`).
Repeat until it builds clean.

- [ ] **Step 6: Run the moved tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower`
Expected: PASS — the `lower/` unit tests (including the `IrError::Parse` / `McpBuildError` assertions, now `LowerError::`) moved with the code. If a test references `tau_ir::lower::` or `tau_ir::error::`, change it to `crate::` / `tau_ir_lower::`.

- [ ] **Step 7: clippy + fmt**

Run:
```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir-lower --all-targets
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ir-lower -- --check
```
Expected: clean. Remove any dep from `Cargo.toml` (Step Task1.2) that clippy/cargo reports as unused.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ir-lower/ Cargo.lock
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-ir-lower): port lower pass + LowerError from tau-ir"
```

NOTE: `tau-ir` still has its own `lower/`+`error.rs` (now duplicated). That's the transient state; Task 4 deletes them. The workspace compiles throughout.

---

### Task 3: Repoint the three lowering consumers to `tau-ir-lower`

**Files:**
- Modify: `crates/tau-cli/Cargo.toml`, `crates/tau-cli/src/cmd/{build,run,dev/session}.rs`
- Modify: `crates/tau-conformance/Cargo.toml`, `crates/tau-conformance/src/scenario.rs`
- Modify: `crates/tau-ir-conformance/Cargo.toml`, `crates/tau-ir-conformance/src/{bundle_mode,dev_mode}.rs`

- [ ] **Step 1: Add the `tau-ir-lower` dep to the three consumers**

In each of `crates/tau-cli/Cargo.toml`, `crates/tau-conformance/Cargo.toml`, `crates/tau-ir-conformance/Cargo.toml`, add under `[dependencies]` (next to the existing `tau-ir` line):

```toml
tau-ir-lower = { workspace = true }
```

And add to the workspace root `Cargo.toml` `[workspace.dependencies]` (next to the `tau-ir` alias at line ~63):

```toml
tau-ir-lower = { path = "crates/tau-ir-lower", version = "0.0.0" }
```

- [ ] **Step 2: Rewrite the import paths in consumer source**

In the six consumer source files, rewrite:
- `tau_ir::lower::X` → `tau_ir_lower::X` (e.g. `tau_ir::lower::lower_project` → `tau_ir_lower::lower_project`, `tau_ir::lower::ResolvedServerTool` → `tau_ir_lower::ResolvedServerTool`).
- `tau_ir::error::IrError` → `tau_ir_lower::LowerError` (the one site: `tau-cli/src/cmd/build.rs:356`, `pub lower_error: Option<tau_ir::error::IrError>` → `Option<tau_ir_lower::LowerError>`).
- Any `use tau_ir::lower::...;` → `use tau_ir_lower::...;`.

Find every site: `grep -rn "tau_ir::lower\|tau_ir::error\|IrError" crates/tau-cli/src crates/tau-conformance/src crates/tau-ir-conformance/src`. Comments mentioning `IrError` can stay or be updated for accuracy; code references must change.

- [ ] **Step 3: Workspace check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check --workspace`
Expected: PASS. The compiler pinpoints any missed path; fix and re-run. (`tau-ir`'s own `lower`/`error` still exist, so even a missed consumer site would still resolve against the old path — to be sure nothing silently still points at `tau_ir::lower`, run `grep -rn "tau_ir::lower\|tau_ir::error::IrError" crates/tau-cli/src crates/tau-conformance/src crates/tau-ir-conformance/src` and confirm zero matches.)

- [ ] **Step 4: Run the consumer test suites**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```
Expected: PASS (these exercise the real lowering path — the integration safety net for the move).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli crates/tau-conformance crates/tau-ir-conformance Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "refactor: repoint lowering consumers to tau-ir-lower"
```

---

### Task 4: Strip `tau-ir` to pure no_std

Now nothing outside `tau-ir` uses its `lower`/`error`. Delete them and make `tau-ir` genuinely no_std.

**Files:**
- Delete: `crates/tau-ir/src/lower/` (whole dir), `crates/tau-ir/src/error.rs`
- Modify: `crates/tau-ir/src/lib.rs`, `crates/tau-ir/src/message.rs`, `crates/tau-ir/Cargo.toml`

- [ ] **Step 1: Delete the moved modules**

```bash
git rm -r crates/tau-ir/src/lower
git rm crates/tau-ir/src/error.rs
```

- [ ] **Step 2: Edit `crates/tau-ir/src/lib.rs`**

- Remove `#[cfg(feature = "with-std-adapters")] extern crate std;` (lines ~18-19).
- Remove `pub mod error;` (line 26).
- Remove the `#[cfg(feature = "with-std-adapters")] pub mod lower;` (lines ~29-30).
- Remove `pub use error::IrError;` (line 49).
- Keep everything else (all the pure modules + re-exports, including `pub mod message;` and `pub use message::{Message, MessagePayload};`).

- [ ] **Step 3: Delete the std Message envelope adapters in `crates/tau-ir/src/message.rs`**

Remove the two `#[cfg(feature = "with-std-adapters")] impl From<...> for Message { ... }` / `impl From<Message> for tau_domain::Message { ... }` blocks (around lines 162-200, the `=== Message envelope adapters ===` section). Leave the un-gated `MessagePayload` conversions intact. After this, `grep -n "with-std-adapters" crates/tau-ir/src/message.rs` must return nothing.

- [ ] **Step 4: Edit `crates/tau-ir/Cargo.toml`**

- Delete the `tau-pkg = { workspace = true, optional = true }` line from `[dependencies]` (and its preceding explanatory comment).
- Delete the `tau-pkg = { workspace = true }` line from `[dev-dependencies]`.
- Change `serde_json = { workspace = true }` → `serde_json = { workspace = true, default-features = false, features = ["alloc"] }`.
- In `[features]`: delete `with-std-adapters = ["dep:tau-pkg"]`; change `default = ["with-std-adapters"]` → `default = []`. Keep `test-fixtures = []` only if it's referenced (`grep -rn "test-fixtures" crates/tau-ir`); otherwise leave it.

- [ ] **Step 5: Build pure `tau-ir` (host) + the kernel**

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core
```
Expected: PASS. If `tau-ir` fails because a pure module still constructs `IrError` (grep says none today), that code is lowering code — move it to `tau-ir-lower` rather than re-adding the type. If `serde_json` alloc-only breaks a `tau-ir` use (e.g. a `from_reader`), switch that call to the alloc-compatible API (`from_slice`/`from_str`).

- [ ] **Step 6: Workspace check + clippy + fmt**

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check --workspace
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir --all-targets
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ir -- --check
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir Cargo.lock
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "refactor(tau-ir): strip lower+error+std adapters → pure no_std crate"
```

---

### Task 5: The payoff — wasm builds + CI guards

**Files:**
- Modify: `.github/workflows/ci.yml` (`runtime-core-no-std` job, ~lines 349-380)

- [ ] **Step 1: RED→GREEN — `tau-ir` genuine no_std (no-std target)**

Run:
```
rustup target add wasm32-unknown-unknown
timeout 420 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-ir --no-default-features --target wasm32-unknown-unknown
```
Expected: PASS. (Was impossible before the split — `tau-ir` couldn't even compile `--no-default-features`.) If a dep still pulls std, fix it to alloc-only in `tau-ir/Cargo.toml` and re-run; if a `tau-ir` source line needs std, that's a leak to fix at the call site.

- [ ] **Step 2: RED→GREEN — `tau-runtime-core` for wasm32-wasip2 (THE goal)**

Run:
```
rustup target add wasm32-wasip2
timeout 420 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-runtime-core --no-default-features --target wasm32-wasip2
```
Expected: PASS. Confirm `tau-pkg`/`tokio`/`rustix` are absent: `cargo tree -p tau-runtime-core --no-default-features --target wasm32-wasip2 -i tau-pkg` should print `error: package ID specification ... did not match any packages` (or empty) — i.e. tau-pkg is no longer in the graph.

- [ ] **Step 3: Add both guards to CI**

In `.github/workflows/ci.yml`, in the `runtime-core-no-std` job, after the existing "no-std builds (default and no-default-features)" step, add:

```yaml
      - name: tau-ir genuine no_std (wasm32-unknown-unknown)
        run: |
          rustup target add wasm32-unknown-unknown
          cargo build -p tau-ir --no-default-features --target wasm32-unknown-unknown
      - name: tau-runtime-core builds for wasm32-wasip2
        run: |
          rustup target add wasm32-wasip2
          cargo build -p tau-runtime-core --no-default-features --target wasm32-wasip2
```

- [ ] **Step 4: Validate the workflow YAML**

Run: `timeout 30 python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"`
Expected: `ok`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "ci: guard tau-ir (wasm32-unknown-unknown) + core (wasm32-wasip2) no_std"
```

---

### Task 6: Full verification before PR

**Files:** none (verification only)

- [ ] **Step 1: Affected-crate test suites**

Run each (all must PASS):
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
```

- [ ] **Step 2: Doctests for the crates whose public API moved**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --doc
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir-lower --doc
```
Expected: PASS. (If a moved doctest in `tau-ir-lower` uses `tau_ir::lower::` / `IrError`, update it to `tau_ir_lower::` / `LowerError`.)

- [ ] **Step 3: Workspace clippy + fmt**

Run:
```
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt --all -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy --workspace --all-targets
```
Expected: PASS (CI runs `just lint` = clippy `-D warnings`).

- [ ] **Step 4: Re-run both wasm guards clean**

Run:
```
timeout 420 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir --no-default-features --target wasm32-unknown-unknown
timeout 420 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core --no-default-features --target wasm32-wasip2
```
Expected: BOTH PASS.

- [ ] **Step 5: Final no-leak grep**

Run: `grep -rn "tau_pkg\|tau-pkg" crates/tau-runtime-core/src crates/tau-runtime-core/Cargo.toml crates/tau-ir/src crates/tau-ir/Cargo.toml`
Expected: NO matches (comments included; reword any stray comment). This proves both the direct (SkillResolver port) and transitive (this split) `tau-pkg` leaks are gone from the kernel + pure IR.

- [ ] **Step 6: Request code review** (handled by the controller — superpowers:requesting-code-review before the PR).

---

## Self-Review (plan author)

**Spec coverage:**
- Crate topology / new `tau-ir-lower` ✓ (Task 1).
- Move `lower/` + `error.rs`→`LowerError` + `McpBuildError`, repoint imports ✓ (Task 2).
- Delete `with-std-adapters`, `extern crate std`, std `Message` adapters, `tau-pkg` dep; `serde_json`→alloc-only ✓ (Task 4).
- Consumer migration (3 lowering consumers; pure consumers untouched) ✓ (Task 3).
- Genuine no_std `tau-ir` guard (`wasm32-unknown-unknown`) + core `wasm32-wasip2` guard, both in CI ✓ (Task 5).
- Full verification incl. conformance ✓ (Task 6).
- Composition with SkillResolver port noted ✓ (Context + Task 6 Step 5).

**Placeholder scan:** none — every step has exact commands/paths; the one judgment area (bare `crate::{...}` lines, missing re-exports) is explicitly compiler-driven with a stated mapping table.

**Type/name consistency:** `IrError` → `LowerError` used consistently (Tasks 2,3,4). `tau_ir::lower::X` → `tau_ir_lower::X` consistent (Tasks 2,3). New crate name `tau-ir-lower` / import root `tau_ir_lower` consistent throughout. `serde_json` alloc-only only for `tau-ir` (Task 4), std kept for `tau-ir-lower` (Task 1).

**Green-commit ordering:** scaffold (1) → copy (2, duplication but compiles) → repoint consumers (3) → strip duplicate from tau-ir (4) → guards (5) → verify (6). Every commit compiles; no big-bang.
