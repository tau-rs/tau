# Doctests Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Activate the 40 currently-`ignore`d doctests across the five tier-1 stable-surface crates (`tau-plugin-protocol`, `tau-plugin-sdk`, `tau-runtime`, `tau-domain`, `tau-pkg`), so `cargo test --doc` runs them on every PR and signature drift breaks the build.

**Architecture:** One PR per crate, in size-ascending order. Each PR: examine every `ignore` fence in the target crate, classify it (A pure-activation / B needs-hidden-fixture / C placeholder / D convert-to-no_run), apply the classification, and ensure `cargo test --doc -p <crate>` is green. A small fixture pattern using `tau_ports::fixtures::MockLlmBackend` (already a dev-dep of `tau-runtime`) unlocks the Runtime-flow examples. An inventory document tracks per-item progress.

**Tech Stack:** Rust 2021, `cargo test --doc`, `tau_ports::fixtures::MockLlmBackend`, `tempfile`, `tokio` (already in dev-deps). No new workspace dependencies expected.

---

## Spec reference

This plan implements `docs/superpowers/specs/2026-05-25-doctests-round-2-design.md`. Key constraints:

- Default fence on activation: bare ` ``` ` (executed). Use `no_run` only for category D (genuinely-can't-execute) with a one-line justification.
- Hidden setup with `# ` lines is allowed and encouraged to keep rendered examples focused.
- Forbidden: real network, env-var reads (outside `# std::env::set_var` hidden setup), filesystem writes outside `tempfile::tempdir()`, real subprocess spawns, `.unwrap()` on meaningful `Result`s.

---

## Pre-flight (do this once, before Task 1)

- [ ] **Step 0.1: Verify worktree state**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-2-spec
git status
git rev-parse --abbrev-ref HEAD
```

Expected: branch `feat/doctests-round-2-spec`, clean working tree, HEAD ahead of `origin/main` by one commit (the spec).

- [ ] **Step 0.2: Confirm baseline doctest counts**

```bash
for crate in tau-plugin-protocol tau-plugin-sdk tau-runtime tau-domain tau-pkg; do
  cnt=$(git grep -E '^\s*///\s*```ignore' -- "crates/$crate/src/" | wc -l | tr -d ' ')
  echo "$crate: $cnt"
done
```

Expected output (must match exactly — if not, the inventory in Task 1 must be re-derived):

```
tau-plugin-protocol: 3
tau-plugin-sdk: 3
tau-runtime: 3
tau-domain: 12
tau-pkg: 19
```

- [ ] **Step 0.3: Confirm the green baseline**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-plugin-protocol -p tau-plugin-sdk -p tau-runtime -p tau-domain -p tau-pkg
```

Expected: all doctests pass (the 40 ignored items are still ignored and don't count as failures). If any current doctest fails on baseline `main` + this spec commit, stop and investigate before proceeding.

---

## Task 1: Phase 1 — Build the inventory

**Files:**
- Create: `docs/superpowers/inventories/2026-05-25-ignored-doctests.md`

This task produces the canonical per-item classification that Tasks 2–6 execute against. No source code is modified.

- [ ] **Step 1.1: Generate the raw fence list**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-2-spec
git grep -n '```ignore' -- 'crates/tau-plugin-protocol/src/' 'crates/tau-plugin-sdk/src/' 'crates/tau-runtime/src/' 'crates/tau-domain/src/' 'crates/tau-pkg/src/' > /tmp/ignored-fences.txt
wc -l /tmp/ignored-fences.txt
```

Expected: 40 lines.

- [ ] **Step 1.2: Create the inventory file with header + table skeleton**

Write `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` with this exact content (the row body is filled in step 1.3):

```markdown
# Ignored doctests inventory — round 2

**Source:** `git grep '```ignore' -- crates/{tau-plugin-protocol,tau-plugin-sdk,tau-runtime,tau-domain,tau-pkg}/src/` on 2026-05-25.
**Spec:** `docs/superpowers/specs/2026-05-25-doctests-round-2-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-25-doctests-round-2.md`.

## Categories

- **A — pure activation:** body is correct; flip `ignore` → ` ``` `.
- **B — needs hidden setup:** body is correct but references types/values that need `# ` hidden preamble (e.g. a `MockLlmBackend`, a `tempfile::tempdir()` directory).
- **C — placeholder:** body is `/* ... */` or stale-reference (e.g. "deferred to Task 7"). Rewrite or delete.
- **D — `no_run`:** activation would require forbidden side-effects (network, real sandbox, real subprocess). Convert `ignore` → `no_run`, add a one-line justification above the fence.

## Items

| # | Crate | File:line | Item | Category | Strategy |
|---|---|---|---|---|---|
| 1 | tau-plugin-protocol | error.rs:13 | `ProtocolError` | A | flip to ` ``` ` |
| 2 | tau-plugin-protocol | error.rs:83 | `RpcErrorEnvelope` | A | flip to ` ``` ` |
| 3 | tau-plugin-protocol | frame.rs:27 | `Frame` enum (Notification example) | A* | flip to ` ``` ` AND replace `params: vec![]` with a valid MessagePack array — see Task 2 step 2.3 |
| 4 | tau-plugin-sdk | configure.rs:76 | `Configure` trait | A | flip to ` ``` ` |
| 5 | tau-plugin-sdk | runners/llm_backend.rs:122 | `run_llm_backend_with_config` | B | add hidden `# struct MyPlugin; impl LlmBackend for MyPlugin { ... } impl Configure for MyPlugin { ... }` setup |
| 6 | tau-plugin-sdk | runners/tool.rs:125 | `run_tool_with_config` | B | as #5 but with `Tool` instead of `LlmBackend` |
| 7 | tau-runtime | builder.rs:405 | `Runtime::run_streaming` | B | needs full Runtime construction via `Runtime::builder()` + `MockLlmBackend` + AgentDefinition + PackageManifest — see Task 4 step 4.3 |
| 8 | tau-runtime | builder.rs:464 | `Runtime::run_streaming_with_history` | B | same shape as #7 |
| 9 | tau-runtime | error.rs:58 | `BuildError` | C | body says "construction deferred to Task 7"; replace with a real `Runtime::builder().build()` call asserting `BuildError::NoLlmBackend` |
| 10 | tau-domain | message.rs:74 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 11 | tau-domain | package/capability.rs:20 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 12 | tau-domain | package/capability.rs:70 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 13 | tau-domain | package/capability.rs:104 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 14 | tau-domain | package/capability.rs:129 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 15 | tau-domain | package/capability.rs:149 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 16 | tau-domain | package/capability.rs:175 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 17 | tau-domain | package/manifest.rs:17 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 18 | tau-domain | package/manifest.rs:45 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 19 | tau-domain | package/manifest.rs:507 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 20 | tau-domain | package/plugin.rs:96 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 21 | tau-domain | package/plugin.rs:153 | TBD-by-Task-5 | ? | classify in step 5.1 |
| 22 | tau-pkg | install.rs:152 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 23 | tau-pkg | install.rs:769 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 24 | tau-pkg | lockfile.rs:135 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 25 | tau-pkg | lockfile.rs:192 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 26 | tau-pkg | lockfile.rs:317 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 27 | tau-pkg | lockfile.rs:538 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 28 | tau-pkg | lockfile.rs:587 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 29 | tau-pkg | lockfile.rs:608 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 30 | tau-pkg | lockfile.rs:632 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 31 | tau-pkg | manifest.rs:41 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 32 | tau-pkg | registry.rs:25 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 33 | tau-pkg | registry.rs:46 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 34 | tau-pkg | scope.rs:262 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 35 | tau-pkg | scope.rs:296 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 36 | tau-pkg | scope.rs:326 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 37 | tau-pkg | scope.rs:400 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 38 | tau-pkg | tree_hash.rs:86 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 39 | tau-pkg | update.rs:28 | TBD-by-Task-6 | ? | classify in step 6.1 |
| 40 | tau-pkg | update.rs:94 | TBD-by-Task-6 | ? | classify in step 6.1 |

## Status log

(Updated by Tasks 2–6 as each row is activated. `status` ∈ {pending, activated, no_run, deleted}.)
```

- [ ] **Step 1.3: Commit the inventory skeleton**

```bash
git add docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "$(cat <<'EOF'
docs(plan): inventory skeleton for doctests round 2

Lists all 40 currently-ignored doctests across the five tier-1 crates,
with categorization for the items already inspected during plan
authoring (rows 1-9). Domain + pkg rows (10-40) are filled in during
their respective per-crate tasks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit lands cleanly. No PR yet — the inventory rides along with Task 2's PR (or the inventory updates ride with each per-crate PR).

---

## Task 2: PR-A — tau-plugin-protocol (3 items)

**Files:**
- Modify: `crates/tau-plugin-protocol/src/error.rs` (lines 13, 83)
- Modify: `crates/tau-plugin-protocol/src/frame.rs` (line 27)
- Modify: `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` (rows 1–3 status)

- [ ] **Step 2.1: Activate `ProtocolError` doctest (row 1, category A)**

Open `crates/tau-plugin-protocol/src/error.rs`. The current fence (lines 13–17):

```rust
/// ```ignore
/// use tau_plugin_protocol::ProtocolError;
/// let err = ProtocolError::FrameTooLarge { len: 1, max: 0 };
/// assert!(format!("{err}").contains("frame too large"));
/// ```
```

Change ` ```ignore ` (line 13) to ` ``` `. The body is unchanged. Result:

```rust
/// ```
/// use tau_plugin_protocol::ProtocolError;
/// let err = ProtocolError::FrameTooLarge { len: 1, max: 0 };
/// assert!(format!("{err}").contains("frame too large"));
/// ```
```

- [ ] **Step 2.2: Activate `RpcErrorEnvelope` doctest (row 2, category A)**

Same file, lines 83–91. Change ` ```ignore ` to ` ``` `:

```rust
/// ```
/// use tau_plugin_protocol::{RpcErrorEnvelope, METHOD_NOT_FOUND};
/// let env = RpcErrorEnvelope {
///     code: METHOD_NOT_FOUND,
///     message: "method not found".into(),
///     data: None,
/// };
/// assert_eq!(env.code, -32601);
/// ```
```

- [ ] **Step 2.3: Activate `Frame` doctest (row 3, category A* — body fix needed)**

Open `crates/tau-plugin-protocol/src/frame.rs`. Current body (lines 27–36):

```rust
/// ```ignore
/// use tau_plugin_protocol::Frame;
/// let frame = Frame::Notification {
///     method: "stream.chunk".into(),
///     params: vec![],
/// };
/// let bytes = frame.clone().encode().unwrap();
/// let decoded = Frame::decode(&bytes).unwrap();
/// assert_eq!(frame, decoded);
/// ```
```

This currently fails if activated, because `params: vec![]` on a `Notification` triggers `ProtocolError::EmptyFrameSlot { slot: "params" }` per `error.rs:60-65`. Replace with a valid one-byte empty-MessagePack-array payload (`0x90`):

```rust
/// ```
/// use tau_plugin_protocol::Frame;
/// let frame = Frame::Notification {
///     method: "stream.chunk".into(),
///     params: vec![0x90], // empty MessagePack array
/// };
/// let bytes = frame.clone().encode().unwrap();
/// let decoded = Frame::decode(&bytes).unwrap();
/// assert_eq!(frame, decoded);
/// ```
```

- [ ] **Step 2.4: Run the doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-plugin-protocol
```

Expected: all doctests in `tau-plugin-protocol` pass, including the three just activated. Total runs goes up by 3.

- [ ] **Step 2.5: Update inventory status**

In `docs/superpowers/inventories/2026-05-25-ignored-doctests.md`, append to the "Status log" section:

```markdown
- 2026-05-25 — rows 1, 2, 3 → activated (PR-A).
```

- [ ] **Step 2.6: Commit**

```bash
git add crates/tau-plugin-protocol/src/error.rs \
        crates/tau-plugin-protocol/src/frame.rs \
        docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "$(cat <<'EOF'
test(plugin-protocol): activate three ignored doctest examples

Round 2 of "doctests in /// comments". Flips `ignore` to executed on
ProtocolError, RpcErrorEnvelope, and Frame doctests. Fixes the Frame
example body to use [0x90] (empty MessagePack array) instead of vec![],
which would have hit EmptyFrameSlot if executed as-was.

Refs: docs/superpowers/specs/2026-05-25-doctests-round-2-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.7: Push and open PR-A**

```bash
scripts/agent-push.sh
gh pr create --title "test(plugin-protocol): activate three ignored doctest examples" --body "$(cat <<'EOF'
## Summary
- Activate `ProtocolError`, `RpcErrorEnvelope`, and `Frame` doctests (rows 1-3 of inventory).
- Fix `Frame` example body to use a valid MessagePack empty-array payload.

Round 2 of "doctests in /// comments" — see [spec](../docs/superpowers/specs/2026-05-25-doctests-round-2-design.md).

## Test plan
- [x] `cargo test --doc -p tau-plugin-protocol` green locally.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Task 3: PR-B — tau-plugin-sdk (3 items)

**Files:**
- Modify: `crates/tau-plugin-sdk/src/configure.rs` (line 76)
- Modify: `crates/tau-plugin-sdk/src/runners/llm_backend.rs` (line 122)
- Modify: `crates/tau-plugin-sdk/src/runners/tool.rs` (line 125)
- Modify: `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` (rows 4–6 status)

- [ ] **Step 3.1: Activate `Configure` doctest (row 4, category A)**

`crates/tau-plugin-sdk/src/configure.rs` lines 76–98 already contain a complete, self-contained example (uses only `serde::Deserialize` + `tau_plugin_sdk::{ConfigError, Configure}` + a hand-rolled `MyConfig`/`MyPlugin`). Flip ` ```ignore ` (line 76) to ` ``` `:

```rust
/// ```
/// use serde::Deserialize;
/// use tau_plugin_sdk::{ConfigError, Configure};
///
/// #[derive(Deserialize)]
/// struct MyConfig {
///     api_key: String,
/// }
///
/// struct MyPlugin {
///     api_key: String,
/// }
///
/// impl Configure for MyPlugin {
///     type Config = MyConfig;
///     fn from_config(config: Self::Config) -> Result<Self, ConfigError> {
///         if config.api_key.is_empty() {
///             return Err(ConfigError::MissingField("api_key"));
///         }
///         Ok(MyPlugin { api_key: config.api_key })
///     }
/// }
/// ```
```

- [ ] **Step 3.2: Convert `run_llm_backend_with_config` to `no_run` with hidden fixture (row 5, category B)**

Open `crates/tau-plugin-sdk/src/runners/llm_backend.rs`. The current body (lines 122–132) calls `run_llm_backend_with_config::<MyPlugin>(...)` but `MyPlugin` is undefined and the body would attempt to read from stdin in a tokio runtime — too much for a doctest.

Convert `ignore` → `no_run` and add hidden setup defining a minimal `MyPlugin` that implements both `LlmBackend` and `Configure`. Replace lines 122–132 with:

```rust
/// ```no_run
/// # use async_trait::async_trait;
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{LlmBackend, CompletionRequest, CompletionResponse, LlmError};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { api_key: String }
/// # struct MyPlugin { _api_key: String }
/// # impl Configure for MyPlugin {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyPlugin { _api_key: c.api_key })
/// #     }
/// # }
/// # #[async_trait]
/// # impl LlmBackend for MyPlugin {
/// #     fn name(&self) -> &str { "my-plugin" }
/// #     async fn complete(&self, _: CompletionRequest) -> Result<CompletionResponse, LlmError> {
/// #         unimplemented!()
/// #     }
/// # }
/// // In plugin main.rs:
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_llm_backend_with_config::<MyPlugin>(
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
```

**Note** the trait method signatures (`name`, `complete`) must match `tau_ports::LlmBackend` exactly. If `cargo test --doc` reports a signature mismatch in step 3.5, open `crates/tau-ports/src/llm_backend.rs` (or wherever `LlmBackend` is defined — `git grep 'pub trait LlmBackend' crates/tau-ports/`) and copy the current trait signature into the hidden block.

- [ ] **Step 3.3: Convert `run_tool_with_config` to `no_run` with hidden fixture (row 6, category B)**

Open `crates/tau-plugin-sdk/src/runners/tool.rs`. Same pattern as step 3.2, but for `Tool`. Replace lines 125–135:

```rust
/// ```no_run
/// # use async_trait::async_trait;
/// # use tau_plugin_sdk::{Configure, ConfigError};
/// # use tau_ports::{Tool, ToolCallRequest, ToolCallResponse, ToolError};
/// # use serde::Deserialize;
/// # #[derive(Deserialize)] struct MyConfig { base_url: String }
/// # struct MyTool { _base_url: String }
/// # impl Configure for MyTool {
/// #     type Config = MyConfig;
/// #     fn from_config(c: MyConfig) -> Result<Self, ConfigError> {
/// #         Ok(MyTool { _base_url: c.base_url })
/// #     }
/// # }
/// # #[async_trait]
/// # impl Tool for MyTool {
/// #     fn name(&self) -> &str { "my-tool" }
/// #     async fn call(&self, _: ToolCallRequest) -> Result<ToolCallResponse, ToolError> {
/// #         unimplemented!()
/// #     }
/// # }
/// // In plugin main.rs:
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     tau_plugin_sdk::run_tool_with_config::<MyTool>(
///         env!("CARGO_PKG_NAME"),
///         env!("CARGO_PKG_VERSION"),
///     ).await?;
///     Ok(())
/// }
/// ```
```

Same signature-mismatch caveat as step 3.2 applies — `git grep 'pub trait Tool' crates/tau-ports/` to verify.

- [ ] **Step 3.4: Add `async-trait` to `tau-plugin-sdk` dev-deps if missing**

```bash
grep -E 'async-trait|async_trait' crates/tau-plugin-sdk/Cargo.toml
```

If no match, add to `[dev-dependencies]` in `crates/tau-plugin-sdk/Cargo.toml`:

```toml
async-trait = { workspace = true }
```

(If `async-trait` is not a workspace dep either, pin it: `async-trait = "0.1"`.)

- [ ] **Step 3.5: Run the doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-plugin-sdk
```

Expected: pass. If row 5 or 6 fails with trait signature mismatch, follow the note in 3.2.

- [ ] **Step 3.6: Update inventory + commit + PR-B**

Append to inventory status log:
```markdown
- 2026-05-25 — rows 4 → activated, 5+6 → no_run with hidden fixture (PR-B).
```

```bash
git add crates/tau-plugin-sdk/ docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(plugin-sdk): activate three ignored doctest examples

Round 2. Configure trait example flips to executed (self-contained body).
run_llm_backend_with_config and run_tool_with_config convert to no_run
with hidden trait-impl preambles — full execution would require a real
stdin/stdout dispatch loop.

Refs: docs/superpowers/specs/2026-05-25-doctests-round-2-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
scripts/agent-push.sh
gh pr create --title "test(plugin-sdk): activate three ignored doctest examples" --body "Round 2 — see [spec](../docs/superpowers/specs/2026-05-25-doctests-round-2-design.md). Configure flips to executed; run_*_with_config convert to no_run with hidden fixture.

## Test plan
- [x] cargo test --doc -p tau-plugin-sdk green locally.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Task 4: PR-C — tau-runtime (3 items + fixture pattern)

**Files:**
- Modify: `crates/tau-runtime/src/builder.rs` (lines 405, 464)
- Modify: `crates/tau-runtime/src/error.rs` (line 58)
- Modify: `crates/tau-runtime/Cargo.toml` (if missing dev-deps)
- Modify: `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` (rows 7–9 status)

This task establishes the Runtime-flow fixture pattern (`MockLlmBackend` from `tau_ports::fixtures`). `tau-runtime` already has `tau-ports = { workspace = true, features = ["test-fixtures", "serde"] }` as a dev-dep (verified — see `crates/tau-runtime/Cargo.toml`), so no new deps needed.

- [ ] **Step 4.1: Find the canonical Runtime construction pattern**

Read `crates/tau-runtime/tests/run_completed.rs` lines 1–50 and `crates/tau-runtime/tests/tracing_emission.rs` lines 1–110. Note the shape of:
- `MockLlmBackend::new("gpt-4").with_response(resp)`
- `Runtime::builder().with_llm_backend(...).build()?`
- An `AgentDefinition` literal
- A `PackageManifest` literal or constructor

The hidden setup in steps 4.2–4.4 mirrors this exactly.

- [ ] **Step 4.2: Activate `BuildError` doctest (row 9, category C)**

Open `crates/tau-runtime/src/error.rs`. Current body (lines 58–61) is a placeholder:

```rust
/// ```ignore
/// // `BuildError` is `#[non_exhaustive]`; constructed by `build()`.
/// // Construction example deferred to Task 7 when the builder lands.
/// ```
```

The builder shipped. Replace with a real example asserting `BuildError::NoLlmBackend`:

```rust
/// ```
/// use tau_runtime::{Runtime, BuildError};
/// let err = Runtime::builder().build().unwrap_err();
/// assert!(matches!(err, BuildError::NoLlmBackend));
/// ```
```

(Note: `unwrap_err()` is acceptable here because the example explicitly documents the error path — see spec §6, the `meaningful Result` rule applies to success-path examples.)

- [ ] **Step 4.3: Activate `Runtime::run_streaming` doctest (row 7, category B)**

Open `crates/tau-runtime/src/builder.rs`. Current body (lines 405–414) uses `/* ... */` for Runtime construction. Replace with a hidden-fixture version that actually executes:

```rust
    /// ```
    /// # tokio_test::block_on(async {
    /// # use tau_runtime::{Runtime, RunOptions};
    /// # use tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage};
    /// # use tau_domain::{AgentDefinition, PackageManifest, Message};
    /// # use futures_util::StreamExt;
    /// # let resp = make_completion_response("hello", make_token_usage(1, 1));
    /// # let llm = MockLlmBackend::new("gpt-4").with_response(resp);
    /// # let runtime = Runtime::builder().with_llm_backend(llm).build().unwrap();
    /// # let agent_def: AgentDefinition = AgentDefinition::default();
    /// # let manifest: PackageManifest = PackageManifest::default();
    /// # let msg: Message = Message::user("hi");
    /// # let opts: RunOptions = Default::default();
    /// let mut stream = runtime.run_streaming(agent_def, manifest, msg, opts).await.unwrap();
    /// while let Some(_event) = stream.next().await {
    ///     // handle event
    /// }
    /// # });
    /// ```
```

**Compatibility check before writing this code:** verify each of these exists with the spelled-out shape:
- `AgentDefinition::default()` — `git grep 'impl Default for AgentDefinition' crates/tau-domain/`
- `PackageManifest::default()` — `git grep 'impl Default for PackageManifest' crates/tau-domain/`
- `Message::user(impl Into<String>)` — `git grep 'pub fn user' crates/tau-domain/src/message.rs`

If any are missing, fall back to the construction shape used in `tests/run_completed.rs`. The hidden block can be longer — what matters is that the visible (non-`#`) lines stay focused on `run_streaming` itself.

- [ ] **Step 4.4: Activate `Runtime::run_streaming_with_history` doctest (row 8, category B)**

Same file, lines 464–475. Same hidden-fixture preamble as step 4.3, but the visible call is `run_streaming_with_history(agent_def, manifest, history, msg, opts)` and the preamble adds `# let history: Vec<Message> = Vec::new();`. Body:

```rust
    /// ```
    /// # tokio_test::block_on(async {
    /// # use tau_runtime::{Runtime, RunOptions};
    /// # use tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage};
    /// # use tau_domain::{AgentDefinition, PackageManifest, Message};
    /// # use futures_util::StreamExt;
    /// # let resp = make_completion_response("hello", make_token_usage(1, 1));
    /// # let llm = MockLlmBackend::new("gpt-4").with_response(resp);
    /// # let runtime = Runtime::builder().with_llm_backend(llm).build().unwrap();
    /// # let agent_def: AgentDefinition = AgentDefinition::default();
    /// # let manifest: PackageManifest = PackageManifest::default();
    /// # let history: Vec<Message> = Vec::new();
    /// # let msg: Message = Message::user("hi");
    /// # let opts: RunOptions = Default::default();
    /// let mut stream = runtime
    ///     .run_streaming_with_history(agent_def, manifest, history, msg, opts)
    ///     .await
    ///     .unwrap();
    /// while let Some(_event) = stream.next().await {
    ///     // handle event
    /// }
    /// # });
    /// ```
```

- [ ] **Step 4.5: Add `tokio-test` and `futures-util` to `tau-runtime` dev-deps if missing for doctests**

```bash
grep -E 'tokio-test|futures-util' crates/tau-runtime/Cargo.toml
```

`futures-util` is already present (see `Cargo.toml` `dev-dependencies` block). If `tokio-test` is missing, add to `[dev-dependencies]`:

```toml
tokio-test = "0.4"
```

**Note:** dev-deps are available to doctests in the same crate by default — no `[dependencies.tokio-test]` needed.

- [ ] **Step 4.6: Run the doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-runtime
```

Expected: all `tau-runtime` doctests pass. If compilation fails on a missing `Default` impl or method, fix the hidden preamble using the pattern from `tests/run_completed.rs`.

- [ ] **Step 4.7: Update inventory + commit + PR-C**

Append to inventory status log:
```markdown
- 2026-05-25 — rows 7, 8, 9 → activated (PR-C, established Runtime-flow fixture pattern via `tau_ports::fixtures::MockLlmBackend`).
```

```bash
git add crates/tau-runtime/ docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(runtime): activate three ignored doctest examples (round 2)

Activates Runtime::run_streaming, Runtime::run_streaming_with_history,
and BuildError doctests using tau_ports::fixtures::MockLlmBackend
(already a dev-dep). Establishes the Runtime-flow hidden-fixture
pattern subsequent rounds can reuse.

Refs: docs/superpowers/specs/2026-05-25-doctests-round-2-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
scripts/agent-push.sh
gh pr create --title "test(runtime): activate three ignored doctest examples (round 2)" --body "Round 2 — see [spec](../docs/superpowers/specs/2026-05-25-doctests-round-2-design.md). Establishes the Runtime-flow hidden-fixture pattern for downstream rounds.

## Test plan
- [x] cargo test --doc -p tau-runtime green locally.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Task 5: PR-D — tau-domain (12 items)

**Files:**
- Modify: 5 files in `crates/tau-domain/src/` (per the inventory)
- Modify: `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` (rows 10–21 status)

This task classifies and activates 12 items not pre-read during plan authoring. Each row gets the same procedure.

- [ ] **Step 5.1: Classify each row**

For each of inventory rows 10–21:

1. Open the file at the listed line.
2. Read 10 lines before and 30 lines after the ` ```ignore ` fence to see the example body and the item it documents.
3. Classify A / B / C / D using spec §4 rules.
4. Update the row's `Category` and `Strategy` columns in the inventory.

Then commit the classified inventory as a separate commit (helps reviewers see the categorization decisions independent of the code changes):

```bash
git add docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(inventory): classify tau-domain doctest rows 10-21"
```

- [ ] **Step 5.2: Activate category-A rows**

For each row classified A in step 5.1: open the file, change ` ```ignore ` to ` ``` `. Don't run tests yet — batch.

- [ ] **Step 5.3: Activate category-B rows**

For each row classified B: add hidden `# ` preamble lines for whatever the body references. Because `tau-domain` is pure data (no async, no Runtime), the typical preamble is just `# use tau_domain::SomeType;` plus a `# let x: SomeType = …;` line.

- [ ] **Step 5.4: Resolve category-C rows**

For each row classified C: either write a real example body (preferred) or delete the entire `# Example` section (including the `///` lines that introduce it). Document the choice in the inventory status log.

- [ ] **Step 5.5: Convert category-D rows**

For each row classified D: change `ignore` to `no_run` and add a `///` comment above the fence with the one-line justification. Example:

```rust
/// # Example
///
/// (`no_run` because it requires a real `serde_yaml` parse on a file path —
/// kept hermetic by not executing.)
///
/// ```no_run
/// ...
/// ```
```

For round-2 D is expected to be rare or zero in `tau-domain` since the surface is data-only.

- [ ] **Step 5.6: Run the doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-domain
```

Expected: pass. If any executed row fails (likely `Default` not implemented for some type, or a renamed method), debug per-row using the same fixture patterns established in Task 4, or downgrade that specific row to category D with justification (update inventory).

- [ ] **Step 5.7: Commit + PR-D**

```bash
git add crates/tau-domain/ docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(domain): activate twelve ignored doctest examples (round 2)

Per row classification in docs/superpowers/inventories/2026-05-25-ignored-doctests.md.

Refs: docs/superpowers/specs/2026-05-25-doctests-round-2-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
scripts/agent-push.sh
gh pr create --title "test(domain): activate twelve ignored doctest examples (round 2)" --body "Round 2 — see [spec](../docs/superpowers/specs/2026-05-25-doctests-round-2-design.md) and [inventory](../docs/superpowers/inventories/2026-05-25-ignored-doctests.md).

## Test plan
- [x] cargo test --doc -p tau-domain green locally.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Task 6: PR-E — tau-pkg (19 items)

**Files:**
- Modify: 7 files in `crates/tau-pkg/src/` (per the inventory)
- Modify: `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` (rows 22–40 status)

Identical procedure to Task 5, but for `tau-pkg` (19 items, rows 22–40).

- [ ] **Step 6.1: Classify rows 22–40** — same procedure as 5.1.

- [ ] **Step 6.2: Activate category-A rows** — same as 5.2.

- [ ] **Step 6.3: Activate category-B rows** — same as 5.3. `tau-pkg` is more likely to need `tempfile::tempdir()` setup since `install.rs`, `lockfile.rs`, `scope.rs` deal with filesystem state. Use:

```rust
/// ```
/// # let dir = tempfile::tempdir().unwrap();
/// # let path = dir.path();
/// use tau_pkg::{...};
/// // actual example using `path`
/// ```
```

Verify `tempfile` is in `crates/tau-pkg/Cargo.toml` `[dev-dependencies]`:

```bash
grep tempfile crates/tau-pkg/Cargo.toml
```

If absent, add `tempfile = { workspace = true }` to `[dev-dependencies]`.

- [ ] **Step 6.4: Resolve category-C rows** — same as 5.4.

- [ ] **Step 6.5: Convert category-D rows** — same as 5.5. Expect more D rows here than in `tau-domain` (filesystem-touching examples may not be trivially executable). Each D row needs a justification line.

- [ ] **Step 6.6: Run the doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-pkg
```

- [ ] **Step 6.7: Commit + PR-E** — same shape as 5.7 with `-p tau-pkg`.

---

## Task 7: Verification + cleanup

**Files:** none (verification only).

- [ ] **Step 7.1: Confirm zero `ignore` fences remain in the targeted crates**

```bash
git grep '```ignore' -- 'crates/tau-plugin-protocol/src/' 'crates/tau-plugin-sdk/src/' 'crates/tau-runtime/src/' 'crates/tau-domain/src/' 'crates/tau-pkg/src/'
```

Expected: 0 matches. If any remain, they must be either:
- Activated (` ``` `) — move them to step 7.4's gap list to fix in a follow-up PR.
- Documented as category D — they should have already been converted to `no_run`. If they're still `ignore`, that's a missed conversion.

- [ ] **Step 7.2: Confirm all doctests pass workspace-wide**

```bash
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo test --doc -p tau-plugin-protocol -p tau-plugin-sdk -p tau-runtime -p tau-domain -p tau-pkg
```

Expected: pass.

- [ ] **Step 7.3: Confirm clippy is still clean on the changed crates**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-doctests cargo clippy -p tau-plugin-protocol -p tau-plugin-sdk -p tau-runtime -p tau-domain -p tau-pkg --all-targets -- -D warnings
```

Expected: pass.

- [ ] **Step 7.4: Final inventory status pass**

Open `docs/superpowers/inventories/2026-05-25-ignored-doctests.md`. Every row's status column must be one of: `activated`, `no_run`, `deleted`. No `pending` or `?` remain.

If any row is still incomplete, that row is a gap — file an issue or follow-up PR.

- [ ] **Step 7.5: Close the loop**

Update `ROADMAP.md` (or wherever doctest-coverage is tracked) with a one-line entry: "Round 2 of doctests-in-comments shipped: all 40 ignored doctests across the five tier-1 crates are now executed or documented `no_run`."

```bash
# Locate the row to update:
grep -n "doctest\|round 1\|tier-1" ROADMAP.md | head
```

(If no obvious row exists, skip — the spec + inventory commits suffice as the durable record.)

- [ ] **Step 7.6: Final commit**

```bash
git add ROADMAP.md docs/superpowers/inventories/2026-05-25-ignored-doctests.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs: close out doctests round 2

All 40 originally-ignored doctests across the five tier-1 crates are
now executed or documented no_run. Round 3 (bare-item coverage) and
round 4 (tier-2 crates) remain as future work — see spec §10.

Refs: docs/superpowers/specs/2026-05-25-doctests-round-2-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
scripts/agent-push.sh
```

---

## Notes for the executor

- **PR cadence:** wait for each PR to merge before opening the next, so that the inventory file always reflects merged state. The branch-protection workflow requires PRs to be up-to-date with `main` — use `gh pr update-branch <PR#>` if main moves under you.
- **Pushing:** ALWAYS use `scripts/agent-push.sh` (or `git push --no-verify` if the change is docs-only). NEVER bare `git push` from agent runtime — see CLAUDE.md "AGENT PUSH RULES".
- **Cargo invocations:** ALWAYS prefixed with `timeout` + `CARGO_INCREMENTAL=0` + `CARGO_TARGET_DIR=target/agent-doctests` + `-p <crate>`. See CLAUDE.md "CARGO RULES".
- **Lefthook may corrupt git identity** during pre-push integration tests — every commit in this plan uses `-c user.name=… -c user.email=…` + `--no-verify` as the safe pattern (CLAUDE.md).
- **If a step reveals the spec is wrong** (e.g. a category-A row is actually category-D): update the inventory + spec in a small follow-up commit. The spec is a draft; round 2 is the first time these classifications meet real code.
