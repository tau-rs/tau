# β.3 MCP facilitator — PR-4: lowering + lockfile v7 + tau build wiring

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship PR-4 of six in the β.3 sub-project. Integrate MCP contracts into the build pipeline: `tau-ir` gets `ToolImpl::Mcp.server_tool_name` + a richer `Caches::mcp_contract` shape; `tau-mcp::contract::resolver` adds a sync port trait + `PinnedResolver`; `tau-mcp-tokio::resolver` adds a live (async, handshake-based) resolver; `tau-pkg` parses `[tools.<name>] sampling.models` + `roots` fields and bumps the lockfile schema to v7 with per-MCP-entry persistence; `tau-cli cmd/build.rs` pre-resolves every referenced MCP server (live by default, pin-only with `--offline`) and emits a v7 lockfile.

**Architecture:** The resolve stage stays SYNC. tau-cli's `cmd/build` does the async pre-fetch (PinnedResolver for `--offline`; LiveMcpContractResolver for the default live path), builds a `BTreeMap<url, ResolvedMcpContract>`, then hands a closure to `lower_project` that reads from the map. `lower/resolve.rs` walks each `ToolImpl::Mcp`, looks up the URL, and **expands** the single entry into N entries (one per `server-tool`), each with its own `ToolId("<entry>.<server-tool-name>")` and per-tool capability intersection. The lockfile records the resolved (url, contract_hash, expanded_tools) so `tau verify --bundle` (PR-6) and runtime drift checks (PR-5) can re-validate without re-handshaking.

**Tech Stack:** Rust 2021. `tau-mcp` (existing: protocol + contract + cassette), `tau-mcp-tokio` (PR-2/PR-3: transport_stdio + transport_http + host_lifecycle), `tau-ir`, `tau-pkg`, `tau-cli`. No new external deps in PR-4.

**Branch:** `feat/beta-3-pr-4-lowering` (created off `origin/main` at `835ded4` — PR-3 just landed).

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/beta-3-pr-4-lowering`.

**Spec reference:** `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` — §2 (crate layout), §4 (IR shape — `server_tool_name`), §5 (lowering data flow with MCP expansion; build-time invariants table), §6 (lockfile v7), §10 (tau check mcp_contracts deferred to PR-6), §15 (PR-4 row).

**Locked architectural decisions consumed (brainstormed 2026-06-02 in chat; this plan IS the PR-4 design record):**
1. `McpContractResolver` port trait lives in `tau-mcp::contract::resolver` (sync, no I/O). Live impl in tau-mcp-tokio's NEW `resolver.rs` (async; does handshakes upfront and populates a cache). Pinned impl `tau_mcp::contract::resolver::PinnedResolver` reads `.tau/mcp/<entry>.contract.json`.
2. `Caches::mcp_contract` evolves: `Fn(&str) -> Option<McpContractEntry>` → `Fn(&str) -> Option<ResolvedMcpContract>` with `ResolvedMcpContract { hash: Hash256, expanded_tools: Vec<ResolvedServerTool> }`. Resolve stage stays sync; tau-cli pre-fetches.
3. PinnedContract file = raw JSON of `tau_mcp::contract::PinnedContract` (the type already exists in PR-1; PR-4 ADDS the file-reading resolver). One file per MCP entry at `.tau/mcp/<entry>.contract.json`.
4. `--offline` semantics: default = live (handshake + capture + write `.tau/mcp/<entry>.contract.json`); `--offline` = pin-only, no network, error if pin missing. `--offline-strict` (live + verify-against-pin) DEFERRED to PR-6.
5. Lockfile v7 migration: v6 silently upgrades on `tau build` (existing v4→v5, v5→v6 pattern at `lockfile.rs:587-589`). Existing v6 fixtures with no MCP entries serialize identically (empty `mcp` Vec).

---

## Files map

### Modified
| File | Change |
|---|---|
| `crates/tau-ir/src/tool_impl.rs` | `ToolImpl::Mcp` gains `server_tool_name: String`. |
| `crates/tau-ir/src/lower/mod.rs` | `McpContractEntry` type alias replaced with `ResolvedMcpContract` import; `Caches::mcp_contract` closure type updated; doc example updated. |
| `crates/tau-ir/src/lower/resolve.rs` | `Mcp` arm rewritten — looks up resolved contract, EXPANDS into N entries (one per `server-tool`), rewrites agent `tool_refs`, enforces the §5 build-time invariants. |
| `crates/tau-ir/src/lower/parse.rs` | `Parsed` struct now also exposes the per-agent `tool_refs` so resolve can rewrite. (May already — verify before editing.) |
| `crates/tau-ir/src/error.rs` | `IrError` gains `McpBuild(McpBuildError)` variant. |
| `crates/tau-mcp/src/contract/mod.rs` | `pub mod resolver;` + re-exports. |
| `crates/tau-pkg/src/project/project.rs` | `UncheckedTool` + `ToolEntry` gain `sampling: Option<SamplingConfig>` + `roots: Vec<PathBuf>`. `validate_tool` propagates. |
| `crates/tau-pkg/src/lockfile.rs` | `MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION` 6 → 7. `LockFile` gains `mcp: Vec<LockedMcpEntry>` (with `#[serde(default)]`). v6→v7 migration test. |
| `crates/tau-pkg/src/lib.rs` | Re-export new lockfile types + project types. |
| `crates/tau-mcp-tokio/src/lib.rs` | `pub mod resolver;` + re-exports. |
| `crates/tau-cli/src/cmd/build.rs` | Wires the resolver chain (pinned or live); adds `--offline` flag; emits v7 lockfile with MCP entries; writes `.tau/mcp/<entry>.contract.json` on the live path. |

### Created (NEW)
| File | Responsibility |
|---|---|
| `crates/tau-ir/src/lower/mcp_build_error.rs` | `McpBuildError` enum carrying all §5 build-time invariant errors. |
| `crates/tau-mcp/src/contract/resolver.rs` | `McpContractResolver` port trait, `ResolvedMcpContract`, `ResolvedServerTool`, `PinnedResolver` (reads file by entry name), `ResolveError`. |
| `crates/tau-mcp-tokio/src/resolver.rs` | `LiveMcpContractResolver` — async; walks tau.toml mcp entries, opens each via `host_lifecycle::open`, captures `ServerContract` via the handshake driver, computes hash, builds the cache map. |
| `crates/tau-cli/tests/cmd_build_mcp.rs` | Integration tests: `--offline` pin-only path; live path (with cassette); v6→v7 lockfile auto-upgrade; existing v6 fixtures still build green. |

### Deleted
- None. The existing `McpContractEntry` type alias in `tau-ir/src/lower/mod.rs` is replaced (effectively renamed to `ResolvedMcpContract` and moved to `tau-mcp`).

---

## Standing constraints (re-read before EVERY cargo / git command)

Same shape as PR-2/PR-3:

| Command | Shape |
|---|---|
| Build / check | `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo {check,build} -p <crate>` |
| Test (nextest) | `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo nextest run -p <crate>` |
| Clippy | `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo clippy -p <crate> --all-targets -- -D warnings` |
| Fmt check | `timeout 30 env CARGO_TARGET_DIR=target/agent-<role> cargo fmt --check -p <crate>` |
| Commits | `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` |
| Push | `git push --no-verify -u origin feat/beta-3-pr-4-lowering` |
| Auto-merge | `gh pr merge <N> --auto` BARE. (Repo IS a merge queue. `autoMergeRequest:null` + `mergeQueueEntry.state=AWAITING_CHECKS` is the normal transition.) |

`<role>` per task: `impl` for the implementer; `verify` for verifications.

PR-2/3 addenda baked in:
- DO NOT enable `features = ["test-support"]` on `tau-runtime-tokio` dev-dep — workspace feature unification trap.
- If a test needs a fixture binary path, use `cargo build --message-format=json-render-diagnostics` + parse `compiler-artifact.executable`. PR-4 should NOT need any subprocess fixture (live resolver only needs cassette-replay tests).
- Auto-merge drops silently after ANY check failure. Re-enroll via `gh pr merge <N> --auto` BARE.
- macOS recurring flakes (`chat_ephemeral_writes_no_file`, `setup_non_interactive_without_tier_errors`) — rerun + re-enroll.

---

## Phase 1 — tau-ir ToolImpl evolution + Caches signature change

### Task 1.1: Add `server_tool_name` to `ToolImpl::Mcp`

**Files:**
- Modify: `crates/tau-ir/src/tool_impl.rs`

- [ ] **Step 1: Read `crates/tau-ir/src/tool_impl.rs`** to see the current `ToolImpl::Mcp` shape.

- [ ] **Step 2: Add the field.** In the `Mcp` arm of the `ToolImpl` enum, add a fourth field:

```rust
    /// MCP-contracted external server.
    Mcp {
        /// MCP server URL (e.g. `"https://mcp.weather.com"`).
        url: String,
        /// Content hash of the MCP contract (the cached schema + capability
        /// declaration the server advertises at handshake). Participates in
        /// the IR module hash so a contract drift invalidates the bundle.
        contract_hash: Hash256,
        /// The subset of capabilities this MCP server is bounded to (a
        /// subset of the contract's declared capabilities; narrowed by
        /// `tau.toml` overrides).
        capability_subset: CapabilityRequirements,
        /// The name passed on the MCP wire (server-side tool name).
        /// Differs from this `Tool` node's `ToolId` because lowering
        /// expands one author-side entry (`weather`) into N IR nodes
        /// (`weather.get_forecast`, `weather.get_current`, ...); each
        /// expanded node carries the server-side name to forward on
        /// `tools/call` requests.
        server_tool_name: String,
    },
```

- [ ] **Step 3: Update the `#[test]` mod** if any existing test constructs a `ToolImpl::Mcp` literal (none currently exist in tool_impl.rs's tests, but check). If any callers in the same crate construct a literal, update them to include `server_tool_name: String::new()` (transitional default).

- [ ] **Step 4: cargo check.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
```

Expected: clean (callers may fail to build — that's expected; they're fixed in subsequent tasks).

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-ir/src/tool_impl.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/tool_impl): ToolImpl::Mcp gains server_tool_name field"
```

### Task 1.2: Introduce `McpBuildError`

**Files:**
- Create: `crates/tau-ir/src/lower/mcp_build_error.rs`
- Modify: `crates/tau-ir/src/lower/mod.rs` (declare submodule)
- Modify: `crates/tau-ir/src/error.rs` (wire variant)

- [ ] **Step 1: Create `crates/tau-ir/src/lower/mcp_build_error.rs`:**

```rust
//! Build-time errors specific to MCP contract resolution + expansion
//! (per β.3 design doc §5 build-time invariants table).
//!
//! All variants surface through `IrError::McpBuild(...)`; the `tau check`
//! aggregator renders them with exit code 64 (validation).

use alloc::string::String;
use alloc::vec::Vec;
use thiserror::Error;

/// Per-spec §5 build-time invariant violations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum McpBuildError {
    /// Live resolver couldn't reach the MCP server (network / spawn /
    /// handshake failure). Re-raised from the resolver's typed error.
    #[error("MCP contract unreachable for entry {entry:?}: {reason}")]
    ContractUnreachable {
        /// `[tools.<entry>]` name from tau.toml.
        entry: String,
        /// Resolver-side reason (e.g. "handshake timeout after 30000ms").
        reason: String,
    },
    /// One or more server-tool capability requirements aren't covered by
    /// the author's envelope.
    #[error(
        "envelope does not cover server-tool {tool:?} capabilities for entry {entry:?}: \
         missing {missing:?}"
    )]
    EnvelopeCoversContract {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Server-side tool name.
        tool: String,
        /// Capabilities the contract declared that the envelope omits
        /// (rendered as `kind`/`host`/`path` keys for diagnostics).
        missing: Vec<String>,
    },
    /// `roots = [...]` declared in tau.toml are not all covered by the
    /// envelope's `fs.read` capabilities.
    #[error("roots {roots:?} for entry {entry:?} not covered by fs.read caps")]
    RootsExceedFsCaps {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Offending root paths.
        roots: Vec<String>,
    },
    /// The MCP server's contract requires sampling (any tool's caps
    /// include `sampling.*`) but the author left `sampling.models = []`.
    #[error("entry {entry:?} server contract requires sampling but sampling.models is empty")]
    SamplingRequiredByContract {
        /// `[tools.<entry>]` name.
        entry: String,
    },
    /// `--offline` was passed but `.tau/mcp/<entry>.contract.json` is missing.
    #[error("entry {entry:?}: --offline requested but pinned contract file is missing at {path:?}")]
    PinnedContractMissing {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Expected path on disk.
        path: String,
    },
    /// A server-tool name contains `.`, which would collide with the
    /// `<entry>.<server-tool>` ToolId convention.
    #[error("server-tool name {name:?} for entry {entry:?} contains '.', which is reserved as the ToolId separator")]
    ServerToolNameContainsDot {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Server-side tool name that contains `.`.
        name: String,
    },
}
```

- [ ] **Step 2: Declare the submodule** in `crates/tau-ir/src/lower/mod.rs`. Insert after the existing `pub mod typecheck;` line:

```rust
pub mod mcp_build_error;
```

And re-export:

```rust
pub use mcp_build_error::McpBuildError;
```

- [ ] **Step 3: Add `IrError::McpBuild` variant.** Read `crates/tau-ir/src/error.rs`. Inside `IrError`, add:

```rust
    /// MCP-specific build error (per β.3 design doc §5).
    #[error("MCP build: {0}")]
    McpBuild(#[from] crate::lower::McpBuildError),
```

- [ ] **Step 4: cargo check.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
```

Expected: clean.

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-ir/src/lower/mcp_build_error.rs crates/tau-ir/src/lower/mod.rs crates/tau-ir/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/lower): McpBuildError + IrError::McpBuild variant"
```

### Task 1.3: Update `Caches::mcp_contract` signature

**Files:**
- Modify: `crates/tau-ir/src/lower/mod.rs`

We can't define `ResolvedMcpContract` in tau-mcp YET (Phase 2 does that), so for now define it inline in tau-ir's lower/mod.rs and move it out in Phase 2. Avoid a forward-dep on tau-mcp from tau-ir — tau-ir is no_std and tau-mcp is also no_std but adding a dep cycle is risky.

**Actually** — since `ResolvedMcpContract` needs to be reachable by both tau-ir (resolve consumer) and tau-mcp/tau-mcp-tokio (producers), the cleanest home is `tau-mcp::contract::resolver`. But that creates a dep `tau-ir → tau-mcp`. Verify tau-ir's Cargo.toml — does it already depend on tau-mcp? If yes, fine. If no, we'd be adding the dep.

**Quick check pattern:** read `crates/tau-ir/Cargo.toml`. If `tau-mcp` is absent, we have two options:
- Option A: define `ResolvedMcpContract` in tau-ir (no new dep; trivial structs)
- Option B: add `tau-mcp = { workspace = true }` to tau-ir deps

Option A is cleaner (tau-ir owns its own resolver-input shape; tau-mcp's `ServerContract` is one possible source). Take option A.

- [ ] **Step 1: Read `crates/tau-ir/src/lower/mod.rs`** and `crates/tau-ir/Cargo.toml`.

- [ ] **Step 2: Replace the `McpContractEntry` type alias with the richer struct.** In `lower/mod.rs`, delete:

```rust
pub type McpContractEntry = ([u8; 32], CapabilityRequirements);
```

Insert in its place:

```rust
/// Per-server-tool slice of a resolved MCP contract.
///
/// The resolve stage expands one `ToolImpl::Mcp` author entry into N IR
/// nodes (one per server-tool). Each node uses one of these structs to
/// fill in its `capability_subset` + `server_tool_name` + (separately)
/// the agent's `tool_refs` rewrite.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedServerTool {
    /// Server-side tool name (what tau-mcp-tokio sends on `tools/call`).
    pub name: alloc::string::String,
    /// Capability requirements the server declares for this tool.
    pub caps: CapabilityRequirements,
    /// JSON schema for the tool's input (passed through to IR; opaque to
    /// the resolver).
    pub input_schema: serde_json::Value,
}

/// Resolved MCP contract for one author-side `[tools.<entry>]` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMcpContract {
    /// SHA-256 of the canonical contract — participates in the IR
    /// module hash so contract drift invalidates the bundle.
    pub hash: [u8; 32],
    /// All server-side tools the contract advertises (one ToolImpl::Mcp
    /// IR node will be emitted per entry).
    pub expanded_tools: alloc::vec::Vec<ResolvedServerTool>,
    /// Whether the contract advertises `sampling/*` capabilities. Used
    /// by resolve to enforce the `SamplingRequiredByContract` invariant.
    pub requires_sampling: bool,
}
```

- [ ] **Step 3: Update the `Caches` struct.** Replace the existing `mcp_contract` field:

```rust
    /// Resolves an MCP URL to its fully-expanded contract (per
    /// β.3 design doc §5). Returns `None` only if `tau build` did not
    /// pre-fetch this URL — the resolver typically errors instead.
    pub mcp_contract: &'a dyn Fn(&str) -> Option<ResolvedMcpContract>,
```

- [ ] **Step 4: Update the lower doc example** in `mod.rs` (lines around 53-58) so `caches.mcp_contract` matches the new shape:

```rust
/// let caches = Caches {
///     native_tool: &|_| None,
///     mcp_contract: &|_| None,
///     skill: &|_| None,
/// };
```

(Should already be `None`-returning so no change there, but verify the doc test still compiles after the type swap.)

- [ ] **Step 5: cargo check + doctests.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir
```

Expected: clean (one doctest passes). Other callers will fail to compile — that's expected (they're fixed in subsequent tasks).

- [ ] **Step 6: Commit.**

```sh
git add crates/tau-ir/src/lower/mod.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/lower): Caches.mcp_contract uses ResolvedMcpContract (expanded per-server-tool)"
```

### Task 1.4: Rewrite `lower/resolve.rs` — MCP expansion

**Files:**
- Modify: `crates/tau-ir/src/lower/resolve.rs`
- Modify: `crates/tau-ir/src/lower/parse.rs` (only if `Parsed` doesn't expose what we need — read first)

The current resolve walks each tool and patches `contract_hash` + `capability_subset` in-place. The new resolve must **expand** one Mcp tool into many. That means:
- Iterate over `parsed.workflow.tools` to find Mcp entries.
- For each, look up the cache, build N new `Tool` nodes (one per `expanded_tools`).
- Remove the original entry from `tools`; insert the N new ones.
- Walk each agent's `tool_refs` and rewrite occurrences of the old entry name to the N expanded ids.
- Enforce build-time invariants (envelope ⊇ caps, roots ⊆ fs.read, sampling-not-empty-if-required, server-tool-name-no-dot).

**Verify before writing**: `crates/tau-ir/src/lower/parse.rs` exports `Parsed` — read it to confirm `parsed.workflow.tools` is a `BTreeMap<ToolId, Tool>` and `parsed.workflow.agents[i].tool_refs` is a `Vec<ToolId>`. Adapt the code below to match exact field names.

- [ ] **Step 1: Read `parse.rs`** to confirm `Parsed` shape.

- [ ] **Step 2: Rewrite `resolve.rs`:**

```rust
//! Second lowering stage: fill content hashes from caller-supplied caches.
//! For MCP tools, also EXPANDS one author entry into N IR nodes
//! (one per server-tool). See β.3 design doc §5.

use alloc::string::String;
use alloc::vec::Vec;

use crate::capability::CapabilityRequirements;
use crate::error::IrError;
use crate::ids::ToolId;
use crate::lower::{Caches, McpBuildError, ResolvedMcpContract, ResolvedServerTool};
use crate::tool_impl::ToolImpl;
use crate::workflow::Tool;

use super::parse::Parsed;

/// Run the resolve stage on a `Parsed` value.
pub(super) fn resolve(mut parsed: Parsed, caches: &Caches<'_>) -> Result<Parsed, IrError> {
    // First pass: resolve native tool content_hashes (unchanged from PR-3).
    // Collect MCP entries that need expansion.
    let mut mcp_entries: Vec<(ToolId, String)> = Vec::new(); // (old id, url)
    for (id, tool) in parsed.workflow.tools.iter_mut() {
        match &mut tool.impl_ {
            ToolImpl::Native { fn_ref, content_hash } => {
                if let Some(h) = (caches.native_tool)(&fn_ref.name) {
                    *content_hash = h;
                }
            }
            ToolImpl::Mcp { url, .. } => {
                mcp_entries.push((id.clone(), url.clone()));
            }
            ToolImpl::Subflow { .. } | ToolImpl::Step { .. } => {}
        }
    }

    // Second pass: per MCP entry, expand.
    for (old_id, url) in mcp_entries {
        let resolved = (caches.mcp_contract)(&url).ok_or_else(|| {
            IrError::McpBuild(McpBuildError::ContractUnreachable {
                entry: old_id.0.clone(),
                reason: alloc::format!("no cache entry for url {url:?}"),
            })
        })?;
        expand_mcp_entry(&mut parsed, &old_id, &url, &resolved)?;
    }

    Ok(parsed)
}

fn expand_mcp_entry(
    parsed: &mut Parsed,
    old_id: &ToolId,
    url: &str,
    resolved: &ResolvedMcpContract,
) -> Result<(), IrError> {
    // Capture the envelope (the original tool's capability_subset) before
    // we remove it.
    let original_tool = parsed
        .workflow
        .tools
        .get(old_id)
        .cloned()
        .expect("old_id was just collected from this map");
    let envelope = match &original_tool.impl_ {
        ToolImpl::Mcp { capability_subset, .. } => capability_subset.clone(),
        _ => unreachable!("expand_mcp_entry called on non-Mcp tool"),
    };

    // Enforce SamplingRequiredByContract.
    // (sampling.models from tau.toml is parsed into Tool::sampling_models —
    //  Phase 3 adds that field; for this task, READ Tool to see whether
    //  it's there yet. If not, defer this check by leaving a // TODO
    //  comment AND a unit test marked #[ignore] until Phase 3 lands.)
    if resolved.requires_sampling {
        // TODO(PR-4-Phase-3): once Tool::sampling_models is wired from
        // tau-pkg, check that the slice is non-empty here.
        // For now, the check is dormant.
    }

    // Enforce no dot in server-tool names + caps subset + emit new entries.
    let mut new_entries: Vec<(ToolId, Tool)> = Vec::new();
    for st in &resolved.expanded_tools {
        if st.name.contains('.') {
            return Err(IrError::McpBuild(McpBuildError::ServerToolNameContainsDot {
                entry: old_id.0.clone(),
                name: st.name.clone(),
            }));
        }
        let intersection = intersect_caps(&envelope, &st.caps);
        let missing = caps_missing_from_envelope(&envelope, &st.caps);
        if !missing.is_empty() {
            return Err(IrError::McpBuild(McpBuildError::EnvelopeCoversContract {
                entry: old_id.0.clone(),
                tool: st.name.clone(),
                missing,
            }));
        }
        let new_id = ToolId(alloc::format!("{}.{}", old_id.0, st.name));
        let new_tool = Tool {
            spec: original_tool.spec.clone(),
            impl_: ToolImpl::Mcp {
                url: url.into(),
                contract_hash: resolved.hash,
                capability_subset: intersection,
                server_tool_name: st.name.clone(),
            },
        };
        new_entries.push((new_id, new_tool));
    }

    // Remove old, insert new.
    parsed.workflow.tools.remove(old_id);
    for (id, tool) in new_entries.iter() {
        parsed.workflow.tools.insert(id.clone(), tool.clone());
    }

    // Rewrite agent tool_refs.
    let new_ids: Vec<ToolId> = new_entries.iter().map(|(id, _)| id.clone()).collect();
    for agent in parsed.workflow.agents.values_mut() {
        let mut rewritten: Vec<ToolId> = Vec::with_capacity(agent.tool_refs.len());
        for r in agent.tool_refs.iter() {
            if r == old_id {
                rewritten.extend(new_ids.iter().cloned());
            } else {
                rewritten.push(r.clone());
            }
        }
        agent.tool_refs = rewritten;
    }

    Ok(())
}

fn intersect_caps(
    envelope: &CapabilityRequirements,
    contract: &CapabilityRequirements,
) -> CapabilityRequirements {
    // v0 intersection: contract caps WIN if they're a strict subset of
    // envelope; envelope caps that the contract doesn't reference are
    // dropped from the resolved entry. Real impl can be a literal
    // set-intersection on the capability hash sets — for v0, just clone
    // the contract caps (resolve enforces envelope ⊇ contract above).
    contract.clone()
}

fn caps_missing_from_envelope(
    envelope: &CapabilityRequirements,
    contract: &CapabilityRequirements,
) -> Vec<String> {
    // v0: enumerate contract.required_shapes and check each is in
    // envelope.required_shapes. Adapt to the real CapabilityRequirements
    // shape after reading the type.
    //
    // **The implementer must read crates/tau-ir/src/capability.rs and
    // adapt this function** — CapabilityRequirements may be a
    // `BTreeSet<CapabilityShape>` or a `Vec<CapabilityRequirement>`;
    // the missing-set is contract - envelope by some equality.
    let _ = (envelope, contract);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentId, ToolId};
    use alloc::string::ToString;

    fn rsc_minimal(name: &str) -> ResolvedServerTool {
        ResolvedServerTool {
            name: name.into(),
            caps: CapabilityRequirements::default(),
            input_schema: serde_json::Value::Null,
        }
    }

    #[test]
    fn dot_in_server_tool_name_rejected() {
        // Construct a Parsed with one MCP entry and a cache that returns
        // a server-tool named "bad.name". expect_err with
        // McpBuildError::ServerToolNameContainsDot.
        // Detailed harness deferred — exact `Parsed` constructor shape
        // depends on the existing parse.rs API, which the implementer
        // should crib from PR-3's existing tests in tau-ir/src/lower/.
    }
}
```

**IMPORTANT for the implementer:** the `caps_missing_from_envelope` and `intersect_caps` functions are STUBS in the snippet above. Before committing, the implementer MUST:
1. Read `crates/tau-ir/src/capability.rs` to see the real `CapabilityRequirements` shape.
2. Implement set-difference (contract \ envelope) by some structural equality.
3. Implement intersection (the resolved entry holds the narrower of envelope/contract).

The shape is small; following the existing `capability_fit::check` patterns in the same crate is the simplest reference.

**Similarly:** the `dot_in_server_tool_name_rejected` test body is a sketch — the implementer should write a real test that constructs a minimal `Parsed`, calls `resolve`, and asserts on the error. Crib from PR-1/2/3 tests in the same crate.

- [ ] **Step 3: cargo check + unit tests.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
```

Expected: clean. Existing tau-ir tests should pass (the change is purely additive to existing semantics — Native/Subflow/Step paths unchanged). The new `dot_in_server_tool_name_rejected` test should pass once the implementer writes a real body.

- [ ] **Step 4: Commit.**

```sh
git add crates/tau-ir/src/lower/resolve.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/lower): MCP expansion stage in resolve — one entry → N server-tools with cap intersection + invariants"
```

---

## Phase 2 — tau-mcp::contract::resolver port trait + PinnedResolver

### Task 2.1: `resolver.rs` — port trait + types + PinnedResolver

**Files:**
- Create: `crates/tau-mcp/src/contract/resolver.rs`
- Modify: `crates/tau-mcp/src/contract/mod.rs`

The resolver lives in tau-mcp because the input type (`ServerContract`) does too, and the output type (`ResolvedMcpContract`) is structurally identical to tau-ir's. The trait is sync; live (async) impls live in tau-mcp-tokio.

- [ ] **Step 1: Create `crates/tau-mcp/src/contract/resolver.rs`:**

```rust
//! `McpContractResolver` port trait + impls.
//!
//! The resolver feeds tau-ir's `Caches::mcp_contract` closure. Two impls
//! ship in v0:
//!
//! - [`PinnedResolver`] — sync; reads `.tau/mcp/<entry>.contract.json`
//!   per `tau build --offline`.
//! - `tau_mcp_tokio::resolver::LiveMcpContractResolver` — async; performs
//!   the MCP handshake via `host_lifecycle::open` + `tools/list`.
//!
//! Both impls populate a sync cache (`BTreeMap<url, ResolvedMcpContract>`)
//! that the lowering stage reads through.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::contract::canonical::canonical_hash;
use crate::contract::pinned::PinnedContract;
use crate::contract::server_contract::{ContractTool, ServerContract};
use crate::error::McpError;
use crate::McpError as _; // reach McpError for cassette feature

/// Per-server-tool slice of a resolved MCP contract. Mirror of
/// `tau_ir::lower::ResolvedServerTool` — the resolver returns this type
/// and tau-cli converts to tau-ir's shape (trivial field-by-field map).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedServerTool {
    /// Server-side tool name.
    pub name: String,
    /// Capability shape names the contract declares for this tool
    /// (rendered as `"kind=…,host=…"` strings v0; future may carry
    /// structured caps once tau-mcp grows a Capability type).
    pub caps: Vec<String>,
    /// JSON schema for the tool's input (opaque pass-through).
    pub input_schema: serde_json::Value,
}

/// Resolved MCP contract for one tau.toml `[tools.<entry>]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedMcpContract {
    /// SHA-256 of the canonical contract.
    pub hash: [u8; 32],
    /// All server-side tools the contract advertises.
    pub expanded_tools: Vec<ResolvedServerTool>,
    /// True iff any expanded tool's caps include `sampling.*`.
    pub requires_sampling: bool,
}

/// Resolver errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ResolveError {
    /// Pinned file not found.
    #[error("pinned contract file not found at {path:?}")]
    PinnedFileMissing {
        /// Path the resolver tried to read.
        path: String,
    },
    /// Pinned file present but unreadable.
    #[error("pinned contract file unreadable at {path:?}: {reason}")]
    PinnedFileUnreadable {
        /// Path attempted.
        path: String,
        /// I/O error message.
        reason: String,
    },
    /// Pinned file present but content didn't parse.
    #[error("pinned contract file parse failure at {path:?}: {reason}")]
    PinnedFileParse {
        /// Path attempted.
        path: String,
        /// serde / hashing error.
        reason: String,
    },
    /// Contract hash from the pinned file failed self-verification.
    #[error("pinned contract self-hash mismatch at {path:?}")]
    PinnedFileSelfHashMismatch {
        /// Path attempted.
        path: String,
    },
}

impl From<ResolveError> for McpError {
    fn from(e: ResolveError) -> Self {
        McpError::Protocol(alloc::format!("resolver: {e}"))
    }
}

/// The resolver port trait.
///
/// Implementors fetch a contract for one tau.toml `[tools.<entry>]`
/// (identified by `entry` name + `url`). v0 callers pre-fetch all
/// entries before lowering; the trait does not need to be cache-aware.
pub trait McpContractResolver {
    /// Resolve `(entry, url)` to a `ResolvedMcpContract`.
    ///
    /// Note: this method is **sync**. The live resolver in
    /// tau-mcp-tokio handles its own async-to-sync bridging (the
    /// async pre-fetch loop runs before lower; the cache is sync).
    fn resolve(&self, entry: &str, url: &str) -> Result<ResolvedMcpContract, ResolveError>;
}

/// Pinned-file resolver. Reads `<base>/<entry>.contract.json`.
///
/// `base` is typically `<project_root>/.tau/mcp/`.
#[cfg(feature = "with-std-adapters")]
pub struct PinnedResolver {
    base: std::path::PathBuf,
}

#[cfg(feature = "with-std-adapters")]
impl PinnedResolver {
    /// Construct with the directory holding `<entry>.contract.json` files.
    pub fn new(base: impl Into<std::path::PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Build the on-disk path for one entry.
    pub fn pinned_path(&self, entry: &str) -> std::path::PathBuf {
        self.base.join(alloc::format!("{entry}.contract.json"))
    }
}

#[cfg(feature = "with-std-adapters")]
impl McpContractResolver for PinnedResolver {
    fn resolve(&self, entry: &str, url: &str) -> Result<ResolvedMcpContract, ResolveError> {
        let _ = url; // pinned file is keyed by entry name; URL is the cache key
        let path = self.pinned_path(entry);
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ResolveError::PinnedFileMissing {
                    path: path.display().to_string(),
                }
            } else {
                ResolveError::PinnedFileUnreadable {
                    path: path.display().to_string(),
                    reason: alloc::format!("{e}"),
                }
            }
        })?;
        let pinned: PinnedContract = serde_json::from_slice(&bytes).map_err(|e| {
            ResolveError::PinnedFileParse {
                path: path.display().to_string(),
                reason: alloc::format!("{e}"),
            }
        })?;
        pinned.verify_self_hash().map_err(|_| {
            ResolveError::PinnedFileSelfHashMismatch {
                path: path.display().to_string(),
            }
        })?;
        let hash = pinned.decoded_hash().map_err(|e| {
            ResolveError::PinnedFileParse {
                path: path.display().to_string(),
                reason: alloc::format!("decoded_hash: {e}"),
            }
        })?;
        Ok(resolved_from_server_contract(hash, &pinned.contract))
    }
}

/// Public conversion: `ServerContract` (from a live handshake OR a
/// pinned file) + `hash` → `ResolvedMcpContract`. Used by both
/// `PinnedResolver` and `tau_mcp_tokio::resolver::LiveMcpContractResolver`.
pub fn resolved_from_server_contract(
    hash: [u8; 32],
    contract: &ServerContract,
) -> ResolvedMcpContract {
    let mut requires_sampling = false;
    let mut expanded_tools = Vec::with_capacity(contract.tools.len());
    for t in &contract.tools {
        let caps = caps_from_contract_tool(t);
        if caps.iter().any(|c| c.starts_with("sampling.")) {
            requires_sampling = true;
        }
        expanded_tools.push(ResolvedServerTool {
            name: t.name.clone(),
            caps,
            input_schema: t.input_schema.clone(),
        });
    }
    ResolvedMcpContract {
        hash,
        expanded_tools,
        requires_sampling,
    }
}

fn caps_from_contract_tool(t: &ContractTool) -> Vec<String> {
    // v0: the MCP wire doesn't yet carry per-tool cap declarations. The
    // ContractTool struct may or may not have a caps field — read
    // crates/tau-mcp/src/contract/server_contract.rs first. If present,
    // map each cap to a `"kind=…[,host=…[,path=…]]"` string here. If
    // absent, return Vec::new() (the per-tool caps are governed entirely
    // by the author's envelope at this stage).
    let _ = t;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_contract() -> ServerContract {
        // Construct via the existing test helper or by hand; cribbing
        // from crates/tau-mcp/src/contract/server_contract.rs tests.
        // Implementer fills this in after reading the file.
        unimplemented!("crib from server_contract.rs tests")
    }

    #[test]
    fn resolved_from_server_contract_round_trip() {
        // Build a small ServerContract with two tools, pass through
        // resolved_from_server_contract, assert hash + expanded_tools
        // length + names match.
        let _ = dummy_contract;
    }
}
```

**Implementer notes:**
- `caps_from_contract_tool` is a stub; v0 may legitimately return `Vec::new()` if `ContractTool` doesn't yet declare per-tool caps. The cap subset check then lives entirely on the envelope side (resolve.rs's `caps_missing_from_envelope`). Leave it as `Vec::new()` for v0.
- The two unit tests at the bottom should be fleshed out: read `server_contract.rs` tests for the helper pattern, then assert that `resolved_from_server_contract` produces a contract with N expanded_tools matching the input.

- [ ] **Step 2: Wire into `crates/tau-mcp/src/contract/mod.rs`:**

```rust
//! existing doc...

pub mod canonical;
pub mod pinned;
pub mod resolver;
pub mod server_contract;

pub use canonical::canonical_hash;
pub use pinned::PinnedContract;
pub use resolver::{
    resolved_from_server_contract, McpContractResolver, ResolveError, ResolvedMcpContract,
    ResolvedServerTool,
};
#[cfg(feature = "with-std-adapters")]
pub use resolver::PinnedResolver;
pub use server_contract::{ContractTool, ServerContract};
```

Adjust the `pub use` block to match the existing mod.rs's exports — preserve any existing re-exports.

- [ ] **Step 3: cargo check + tests.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp --features with-std-adapters
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --features with-std-adapters
```

Expected: clean.

- [ ] **Step 4: Commit.**

```sh
git add crates/tau-mcp/src/contract/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp/contract): McpContractResolver port trait + PinnedResolver (gated on with-std-adapters)"
```

---

## Phase 3 — tau-pkg sampling.models + roots fields + ToolBody URL parsing

### Task 3.1: Add `sampling` + `roots` fields to `UncheckedTool` + `ToolEntry`

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs`

- [ ] **Step 1: Read `crates/tau-pkg/src/project/project.rs`** (search for `pub struct UncheckedTool` and `pub struct ToolEntry`).

- [ ] **Step 2: Add a `SamplingConfig` struct** at module scope:

```rust
/// Author-declared sampling allowlist for an MCP-contracted tool.
///
/// `[tools.<name>] sampling.models = [...]` — set of LLM model ids the
/// MCP server is allowed to invoke via `sampling/createMessage`. Empty
/// or missing means sampling is refused.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingConfig {
    /// Allowlisted LLM model ids. Empty = sampling refused.
    #[serde(default)]
    pub models: Vec<String>,
}
```

- [ ] **Step 3: Extend `UncheckedTool`** with two new fields (preserve `#[serde(deny_unknown_fields)]`):

```rust
    /// Sampling allowlist (β.3 — empty/missing = sampling refused).
    #[serde(default)]
    pub sampling: Option<SamplingConfig>,
    /// Roots advertised to the MCP server via `roots/list` (β.3 —
    /// must be subset of `fs.read` caps; checked at lowering time).
    #[serde(default)]
    pub roots: Vec<std::path::PathBuf>,
```

- [ ] **Step 4: Extend `ToolEntry`** with the same two fields.

- [ ] **Step 5: Update `validate_tool`** at line ~697 to propagate the fields:

```rust
fn validate_tool(name: String, raw: UncheckedTool) -> Result<ToolEntry, ProjectConfigError> {
    // existing validation...
    Ok(ToolEntry {
        name,
        body: raw.body,
        description: raw.description,
        input_schema: raw.input_schema,
        capabilities: raw.capabilities,
        sampling: raw.sampling,           // NEW
        roots: raw.roots,                 // NEW
    })
}
```

- [ ] **Step 6: Write a unit test** that parses a `[tools.weather]` block with `sampling.models = ["claude-haiku-4-5"]` and `roots = ["/tmp/mcp"]` and asserts both fields round-trip:

```rust
#[test]
fn unchecked_tool_parses_sampling_and_roots() {
    let toml = r#"
        [body]
        mcp = "https://mcp.example.com"
        description = "weather"
        capabilities = []

        [body.sampling]
        models = ["claude-haiku-4-5"]

        [body]
        roots = ["/tmp/mcp"]
    "#;
    // The exact TOML shape depends on `#[serde(flatten)]` semantics on
    // `body`. The implementer should crib the test from existing
    // UncheckedTool tests in this file and adapt — TOML's flatten +
    // nested-table behavior is awkward and the existing tests are the
    // authority. The key assertion: roundtrip preserves both fields.
}
```

The exact TOML shape with `#[serde(flatten)] body: ToolBody` is awkward; the implementer should crib the existing UncheckedTool test in this file (search for `UncheckedTool` + `#[test]`).

- [ ] **Step 7: cargo check + test.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(/.*sampling.*/)'
```

Expected: new test passes. Existing tests still pass.

- [ ] **Step 8: Commit.**

```sh
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-pkg/project): UncheckedTool + ToolEntry gain sampling.models + roots fields"
```

### Task 3.2: ToolBody URL discriminator validation

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs`

The existing `ToolBody::Mcp(String)` accepts ANY string. PR-4 should reject URLs that don't match a known scheme at parse time so build errors surface earlier. Reuse tau-mcp-tokio's `host_lifecycle::url::parse_url` — but that's an async-aware dep tau-pkg shouldn't take. Instead, parse here with a small whitelist: `stdio:`, `http://`, `https://`.

- [ ] **Step 1: Add a validate-on-extract helper** in `validate_tool`:

```rust
// Inside validate_tool, after pulling body out:
if let ToolBody::Mcp(url) = &raw.body {
    if !is_supported_mcp_url(url) {
        return Err(ProjectConfigError::UnsupportedMcpUrl {
            tool: name.clone(),
            url: url.clone(),
        });
    }
}

fn is_supported_mcp_url(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("stdio:") || url.starts_with("http://") || url.starts_with("https://")
}
```

- [ ] **Step 2: Add the error variant** to `ProjectConfigError` in the same file (or wherever the enum lives — `grep -n "pub enum ProjectConfigError"`):

```rust
    /// `[tools.<name>] mcp = "..."` URL has an unsupported scheme.
    #[error("tool {tool:?}: unsupported MCP URL scheme: {url:?}")]
    UnsupportedMcpUrl {
        /// Tool name.
        tool: String,
        /// Offending URL.
        url: String,
    },
```

- [ ] **Step 3: Test.**

```rust
#[test]
fn mcp_url_with_unsupported_scheme_rejected() {
    let toml = r#"
        [tools.bad]
        mcp = "ws://example.com"
    "#;
    let err = ProjectConfig::parse_str(toml).expect_err("should reject");
    assert!(matches!(err, ProjectConfigError::UnsupportedMcpUrl { .. }));
}
```

- [ ] **Step 4: Run.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(/.*unsupported_mcp.*/)'
```

Expected: pass.

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-pkg/project): validate MCP URL scheme at parse (stdio/http/https only)"
```

---

## Phase 4 — tau-pkg lockfile v6 → v7

### Task 4.1: Bump `MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION` + add `mcp` field

**Files:**
- Modify: `crates/tau-pkg/src/lockfile.rs`

- [ ] **Step 1: Read `crates/tau-pkg/src/lockfile.rs`** to confirm the existing v6 shape + migration logic at line ~587.

- [ ] **Step 2: Bump the constant** at line 63:

```rust
pub const MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION: u32 = 7;
```

- [ ] **Step 3: Define new types.** Insert after `LockedSkill` (around line 414):

```rust
/// Per-MCP-entry lockfile record (β.3 PR-4).
///
/// Records what `tau build` resolved for one `[tools.<entry>]` MCP
/// server so subsequent `tau verify --bundle` and the runtime drift
/// check can re-validate without re-handshaking.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedMcpEntry {
    /// Author-side `[tools.<entry>]` name from tau.toml.
    pub entry: String,
    /// MCP server URL the entry was resolved against.
    pub url: String,
    /// Hex-encoded SHA-256 of the canonical resolved contract.
    pub contract_hash: String,
    /// Optional path to the pinned-contract file (relative to project
    /// root). Present when `tau build` wrote the pin or when `--offline`
    /// was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_contract: Option<String>,
    /// Server-side tools the contract expanded into.
    #[serde(default)]
    pub expanded_tools: Vec<LockedMcpExpandedTool>,
}

/// One expanded server-tool's lockfile record.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedMcpExpandedTool {
    /// Server-side tool name.
    pub name: String,
    /// Capability shape names (v0 string form `"kind=…[,host=…]"`).
    #[serde(default)]
    pub caps: Vec<String>,
    /// Hex-encoded SHA-256 of the tool's `input_schema` (deterministic
    /// canonical JSON hash). Used for the runtime drift check.
    pub schema_hash: String,
}
```

- [ ] **Step 4: Add the field to `LockFile`** at line 82 — insert after `packages`:

```rust
    /// Per-MCP-entry resolved records (β.3 PR-4; lockfile schema v7).
    /// Empty `Vec` on lockfiles that have no `[tools.<name>] mcp = …`
    /// entries; v6 lockfiles read with no `mcp` key get `Vec::new()`
    /// via `#[serde(default)]`.
    #[serde(default, rename = "mcp")]
    pub mcp_entries: Vec<LockedMcpEntry>,
```

- [ ] **Step 5: Update `Default::default`** at line 475:

```rust
impl Default for LockFile {
    fn default() -> Self {
        Self {
            schema_version: MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION,
            generated_by_tau_version: env!("CARGO_PKG_VERSION").to_owned(),
            generated_at: SystemTime::now(),
            packages: Vec::new(),
            mcp_entries: Vec::new(),
        }
    }
}
```

- [ ] **Step 6: Update the doc comment block** at line 83 to mention v6→v7:

```rust
    /// v6→v7: `LockFile` gains `mcp_entries: Vec<LockedMcpEntry>` defaulted
    /// via `#[serde(default)]` (β.3 PR-4).
```

(Append to the existing migration narrative.)

- [ ] **Step 7: Update the auto-upgrade write path** at lines 587-589 if it explicitly mentions versions — usually a simple bump.

- [ ] **Step 8: Update existing doctests** that show `schema_version = 6` to `schema_version = 7`. There are several — `grep -n "schema_version = 6\b" crates/tau-pkg/src/lockfile.rs` to find them.

The implementer should NOT rewrite every occurrence — some appear in narrative comments showing migration paths. Only update:
- doc examples that show the CURRENT version (`assert_eq!(lf.schema_version, 6)` → `7`)
- doctest TOML strings that demonstrate the current shape (`schema_version = 6` → `7`)

Preserve narrative references to historical versions (v1-v6) verbatim.

- [ ] **Step 9: Test.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(/lockfile.*/)'
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg
```

Expected: all existing lockfile tests pass (v6 lockfiles silently upgrade). Doctests pass.

- [ ] **Step 10: Commit.**

```sh
git add crates/tau-pkg/src/lockfile.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-pkg/lockfile): schema v6→v7 with LockedMcpEntry + LockedMcpExpandedTool"
```

### Task 4.2: v6 → v7 migration test

**Files:**
- Modify: `crates/tau-pkg/src/lockfile.rs` (extend `mod tests`)

- [ ] **Step 1: Add a unit test.**

```rust
#[test]
fn v6_lockfile_silently_upgrades_to_v7_with_empty_mcp_entries() {
    let v6_toml = r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "example"
active_version = "1.0.0"
source = "https://example.com/x.git"
"#;
    let lf = LockFile::from_toml_str(v6_toml).expect("v6 parses");
    assert_eq!(lf.schema_version, MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION);
    assert!(lf.mcp_entries.is_empty(), "v6 lockfile auto-upgrades with empty mcp_entries");
}

#[test]
fn v7_lockfile_with_mcp_entries_round_trips() {
    let v7_toml = r#"schema_version = 7
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "example"
active_version = "1.0.0"
source = "https://example.com/x.git"

[[mcp]]
entry = "weather"
url = "stdio:npx --yes weather"
contract_hash = "9f2e000000000000000000000000000000000000000000000000000000000000"
pinned_contract = ".tau/mcp/weather.contract.json"

[[mcp.expanded_tools]]
name = "get_forecast"
caps = ["kind=net.http,host=api.weather.com"]
schema_hash = "0a1b000000000000000000000000000000000000000000000000000000000000"
"#;
    let lf = LockFile::from_toml_str(v7_toml).expect("v7 parses");
    assert_eq!(lf.schema_version, 7);
    assert_eq!(lf.mcp_entries.len(), 1);
    let entry = &lf.mcp_entries[0];
    assert_eq!(entry.entry, "weather");
    assert_eq!(entry.expanded_tools.len(), 1);
    assert_eq!(entry.expanded_tools[0].name, "get_forecast");
}
```

- [ ] **Step 2: Run.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg -E 'test(/.*v(6|7).*/)'
```

Expected: both new tests pass.

- [ ] **Step 3: Commit.**

```sh
git add crates/tau-pkg/src/lockfile.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-pkg/lockfile): v6→v7 silent upgrade + v7 mcp_entries round-trip"
```

---

## Phase 5 — tau-mcp-tokio LiveMcpContractResolver

### Task 5.1: `resolver.rs` — async live resolver

**Files:**
- Create: `crates/tau-mcp-tokio/src/resolver.rs`
- Modify: `crates/tau-mcp-tokio/src/lib.rs`

The live resolver runs once, before lowering, in tau-cli's `cmd/build`. It walks every `[tools.<entry>] mcp = "..."` entry, opens each via `host_lifecycle::open`, captures the `ServerContract` from the handshake driver, and returns a `BTreeMap<url, ResolvedMcpContract>` plus the `PinnedContract` instances so tau-cli can write `.tau/mcp/<entry>.contract.json`.

- [ ] **Step 1: Create `crates/tau-mcp-tokio/src/resolver.rs`:**

```rust
//! Live MCP contract resolver — async, performs handshakes upfront and
//! populates a sync cache that tau-ir's lowering stage reads through.

use std::collections::BTreeMap;
use std::sync::Arc;

use tau_mcp::contract::canonical::canonical_hash;
use tau_mcp::contract::pinned::PinnedContract;
use tau_mcp::contract::resolver::{resolved_from_server_contract, ResolvedMcpContract};
use tau_mcp::contract::server_contract::ServerContract;
use tau_mcp::McpError;
use tau_ports::CapabilityPlan;
use thiserror::Error;
use tracing::{info, instrument};

use crate::host_lifecycle::{open, McpClientOptions};
use crate::process_gate_passthrough::passthrough_gate;

/// Errors from the live resolver.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LiveResolverError {
    /// `host_lifecycle::open` failed.
    #[error("entry {entry:?} url {url:?}: open failed: {reason}")]
    OpenFailed {
        /// `[tools.<entry>]` name.
        entry: String,
        /// MCP URL.
        url: String,
        /// `LifecycleError` rendered.
        reason: String,
    },
    /// Canonical hash failed.
    #[error("entry {entry:?}: canonical_hash failed: {0}")]
    Hash(McpError),
}

/// Resolved contract + pin payload (caller writes pin to disk).
pub struct LiveResolved {
    /// Tau-ir-shaped resolved contract (cache key in tau-cli is the URL).
    pub resolved: ResolvedMcpContract,
    /// The pinned-contract payload (caller writes to `.tau/mcp/<entry>.contract.json`).
    pub pinned: PinnedContract,
}

/// One author-side `[tools.<entry>] mcp = "..."` to dial.
pub struct McpEntryInput {
    /// `[tools.<entry>]` name.
    pub entry: String,
    /// MCP URL.
    pub url: String,
    /// Capability plan from author's `capabilities` field.
    pub plan: CapabilityPlan,
}

/// Resolve all `entries` concurrently (one handshake per URL).
///
/// Returns a map keyed by URL so tau-ir's `Caches::mcp_contract` closure
/// can be `|url| map.get(url).cloned()`. ALSO returns per-entry pinned
/// contracts so tau-cli can write `.tau/mcp/<entry>.contract.json`.
#[instrument(skip(entries))]
pub async fn resolve_all(
    entries: Vec<McpEntryInput>,
) -> Result<BTreeMap<String, LiveResolved>, LiveResolverError> {
    let mut out = BTreeMap::new();
    for input in entries {
        info!(entry = %input.entry, url = %input.url, "live MCP resolve");
        let client = open(
            &input.url,
            &input.plan,
            passthrough_gate(),
            McpClientOptions::default(),
        )
        .await
        .map_err(|e| LiveResolverError::OpenFailed {
            entry: input.entry.clone(),
            url: input.url.clone(),
            reason: format!("{e}"),
        })?;
        let contract: &ServerContract = client.contract();
        let hash = canonical_hash(contract).map_err(LiveResolverError::Hash)?;
        let pinned = PinnedContract::from_parts(input.url.clone(), contract.clone())
            .map_err(LiveResolverError::Hash)?;
        let resolved = resolved_from_server_contract(hash, contract);
        out.insert(
            input.url.clone(),
            LiveResolved { resolved, pinned },
        );
    }
    Ok(out)
}

/// Internal: get a passthrough gate. Lives behind an extra layer because
/// the existing passthrough type is in `process_gate::passthrough` and we
/// don't want a circular dep with tau-runtime-tokio just for this. v0
/// imports directly; PR-5 introduces a properly-typed gate selection.
mod process_gate_passthrough_inner {
    use std::sync::Arc;
    use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;
    use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;
    pub fn passthrough_gate() -> Arc<dyn DynProcessCapabilityGate> {
        Arc::new(PassthroughSandbox::new())
    }
}

// Re-export under the name used in the public fn signature.
mod process_gate_passthrough {
    pub use super::process_gate_passthrough_inner::passthrough_gate;
}
```

**Implementer notes:**
- The `process_gate_passthrough` nested module dance exists to avoid name pollution. Simplify to a single private fn if the implementer prefers.
- `PinnedContract::from_parts(url, contract)` returns `Result<Self, McpError>` per PR-1's `pinned.rs`; the error variant `LiveResolverError::Hash(McpError)` is reused for both canonical_hash and PinnedContract::from_parts errors (both are McpError under the hood).

- [ ] **Step 2: Wire into lib.rs.** Add to `crates/tau-mcp-tokio/src/lib.rs`:

```rust
pub mod resolver;

pub use resolver::{resolve_all, LiveResolved, LiveResolverError, McpEntryInput};
```

- [ ] **Step 3: Cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Commit.**

```sh
git add crates/tau-mcp-tokio/src/resolver.rs crates/tau-mcp-tokio/src/lib.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-mcp-tokio/resolver): LiveMcpContractResolver pre-fetches contracts via host_lifecycle::open"
```

---

## Phase 6 — tau-cli/cmd/build wiring + --offline flag

### Task 6.1: Wire pinned + live resolvers in `cmd/build.rs`

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs`

The current `build.rs` (PR-3 baseline) has a stub `mcp_contract: &|_url| None` at line 130. PR-4 replaces it with a real cache populated by either the pinned or live resolver.

- [ ] **Step 1: Read `crates/tau-cli/src/cmd/build.rs`** to see the existing structure (args struct, run function, Caches construction).

- [ ] **Step 2: Extend `BuildArgs`** with the `--offline` flag:

```rust
    /// Use pinned `.tau/mcp/<entry>.contract.json` files for MCP
    /// contracts; never reach out to live MCP servers. Build errors
    /// out if any pinned file is missing.
    #[arg(long)]
    pub offline: bool,
```

- [ ] **Step 3: Walk tau.toml for MCP entries.** Before constructing `Caches`, collect every `[tools.<entry>] mcp = "..."` and build a `Vec<McpEntryInput>` (or `PinnedResolver` calls for `--offline`):

```rust
// (sketch — fold into the real cmd/build/run shape)
let mcp_entries: Vec<(String, String, CapabilityPlan)> = config
    .tools
    .iter()
    .filter_map(|(name, t)| match &t.body {
        ToolBody::Mcp(url) => Some((
            name.clone(),
            url.clone(),
            plan_from_caps(&t.capabilities),
        )),
        _ => None,
    })
    .collect();

let mcp_cache: BTreeMap<String, ResolvedMcpContract> = if args.offline {
    // PinnedResolver path.
    let base = project_root.join(".tau").join("mcp");
    let resolver = PinnedResolver::new(&base);
    let mut cache = BTreeMap::new();
    for (entry, url, _plan) in &mcp_entries {
        let resolved = resolver.resolve(entry, url).map_err(|e| {
            anyhow!("MCP pin resolve failed for entry {entry:?}: {e}")
        })?;
        cache.insert(url.clone(), resolved.into_ir_shape());
    }
    cache
} else {
    // Live path.
    let inputs: Vec<McpEntryInput> = mcp_entries
        .iter()
        .map(|(entry, url, plan)| McpEntryInput {
            entry: entry.clone(),
            url: url.clone(),
            plan: plan.clone(),
        })
        .collect();
    let live = tau_mcp_tokio::resolver::resolve_all(inputs).await?;
    // Write pinned files for next-time --offline.
    let pin_base = project_root.join(".tau").join("mcp");
    std::fs::create_dir_all(&pin_base)?;
    for (entry, _url, _plan) in &mcp_entries {
        if let Some(lr) = live.get(_url) {
            let path = pin_base.join(format!("{entry}.contract.json"));
            let bytes = serde_json::to_vec_pretty(&lr.pinned)?;
            std::fs::write(&path, bytes)?;
        }
    }
    live.into_iter()
        .map(|(url, lr)| (url, lr.resolved.into_ir_shape()))
        .collect()
};

// Pass the cache to Caches.
let caches = Caches {
    native_tool: &|name| native_registry.lookup(name),
    mcp_contract: &|url| mcp_cache.get(url).cloned(),
    skill: &|_| None,
};
```

**The `into_ir_shape()` method** is the conversion from `tau_mcp::contract::resolver::ResolvedMcpContract` to `tau_ir::lower::ResolvedMcpContract`. They have the SAME shape but live in different crates — the implementer should add a small `impl From<tau_mcp::ResolvedMcpContract> for tau_ir::lower::ResolvedMcpContract` (or a helper fn `into_ir_shape()` on the tau-mcp side) so the conversion is one line. The simplest place is a free function in tau-cli/src/cmd/build_helpers.rs or just inline in build.rs.

- [ ] **Step 4: Emit v7 lockfile with mcp entries.** After lowering succeeds, populate `lockfile.mcp_entries`:

```rust
for (entry, url, _plan) in &mcp_entries {
    if let Some(resolved) = mcp_cache.get(url) {
        lockfile.mcp_entries.push(LockedMcpEntry {
            entry: entry.clone(),
            url: url.clone(),
            contract_hash: hex::encode(resolved.hash),
            pinned_contract: Some(format!(".tau/mcp/{entry}.contract.json")),
            expanded_tools: resolved
                .expanded_tools
                .iter()
                .map(|st| LockedMcpExpandedTool {
                    name: st.name.clone(),
                    caps: st.caps.clone(),
                    schema_hash: hex::encode(schema_hash(&st.input_schema)),
                })
                .collect(),
        });
    }
}
```

Where `schema_hash` is a small helper that canonicalizes + SHA-256s the input_schema JSON. Crib from `tau_mcp::contract::canonical::canonical_hash`'s pattern.

- [ ] **Step 5: cargo check + clippy.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit.**

```sh
git add crates/tau-cli/src/cmd/build.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-cli/cmd/build): wire pinned/live MCP resolver + --offline flag + emit v7 lockfile MCP entries"
```

---

## Phase 7 — Migration test sweep + new e2e fixture

### Task 7.1: Verify all existing fixtures still build green

**Files:**
- Modify (test only): `crates/tau-cli/tests/cmd_build_mcp.rs` (NEW)

- [ ] **Step 1: Survey existing fixtures.** Run:

```sh
find crates -name "tau.toml" -not -path "*/target/*"
find crates -path "*/.tau/*Tau.lock*" -not -path "*/target/*"
```

Expected: every conformance fixture in `tau-ir-conformance/cases/`, every `tau-cli/tests/fixtures/*/tau.toml`, plus any e2e fixtures under `tau-pkg/tests/`.

- [ ] **Step 2: Add a sweep test** in `crates/tau-cli/tests/cmd_build_mcp.rs`:

```rust
//! PR-4 migration sweep: every existing fixture's `tau.toml` still
//! lowers and builds green after the lockfile v6→v7 + Caches signature
//! change.

#[test]
fn all_fixtures_build_with_v7_lockfile() {
    let fixtures = collect_tau_tomls();
    assert!(
        !fixtures.is_empty(),
        "fixture sweep didn't find any tau.toml — did the path glob break?"
    );
    for path in fixtures {
        // Build via the same pipeline as `tau build` (sans the live MCP
        // handshake — fixtures without MCP entries should succeed
        // unchanged; fixtures with MCP entries should be skipped here
        // and tested separately via cassette below).
        let result = lower_fixture(&path);
        match result {
            Ok(_) => continue,
            Err(e) if e.is_mcp_unreachable() => {
                // MCP fixture — skip in sweep; covered by cassette test.
                continue;
            }
            Err(e) => panic!("fixture {} failed to build: {e}", path.display()),
        }
    }
}

fn collect_tau_tomls() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut out = Vec::new();
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    for entry in walkdir::WalkDir::new(workspace.join("crates"))
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_name() == "tau.toml" && !entry.path().to_string_lossy().contains("/target/") {
            out.push(entry.into_path());
        }
    }
    out
}

fn lower_fixture(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    // Read tau.toml, run lower_project with stub caches, assert ok.
    // For MCP fixtures, return a synthetic "MCP unreachable" error that
    // is_mcp_unreachable() recognizes.
    todo!("crib from tau-cli's existing fixture-build helpers")
}
```

**This test is a sketch.** The implementer should:
- Use existing fixture-iteration helpers in `crates/tau-cli/tests/common/` if present (PR-1/2/3 may have established them).
- Add `walkdir` as a dev-dep on tau-cli (`walkdir = "2"` in workspace.dependencies if absent).
- Adapt `lower_fixture` to call the real lowering pipeline (parse → resolve with stub caches → typecheck → capability_fit).

- [ ] **Step 3: Run the sweep.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_build_mcp
```

Expected: passes. If any fixture fails, the implementer should look at WHY — typically a fixture using the old `McpContractEntry` shape needs no change (Caches' closure type swap is transparent at the fixture level).

- [ ] **Step 4: Commit.**

```sh
git add crates/tau-cli/tests/cmd_build_mcp.rs Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-cli/cmd/build): sweep test — all existing fixtures still build with v7 lockfile + new Caches shape"
```

### Task 7.2: New e2e fixture — MCP build via cassette

**Files:**
- Create: `crates/tau-cli/tests/fixtures/mcp_weather/tau.toml`
- Create: `crates/tau-cli/tests/fixtures/mcp_weather/.tau/mcp/weather.contract.json` (committed; will be used as the pin)
- Modify (test): `crates/tau-cli/tests/cmd_build_mcp.rs` (add second test)

The new e2e fixture exercises the `--offline` build path against a committed pinned-contract file. This avoids needing a live MCP server in CI.

- [ ] **Step 1: Create the fixture's `tau.toml`:**

```toml
[project]
name = "mcp_weather"

[tools.weather]
mcp = "https://mcp.example.com/weather"
capabilities = [
    { kind = "net.http", host = "api.weather.com" },
]
```

- [ ] **Step 2: Create the pinned-contract JSON** (`.tau/mcp/weather.contract.json`):

```json
{
  "url": "https://mcp.example.com/weather",
  "contract": {
    "server_info": {
      "name": "mock-weather",
      "version": "0.0.0"
    },
    "tools": [
      {
        "name": "get_forecast",
        "description": "Get weather forecast",
        "input_schema": {
          "type": "object",
          "properties": {"lat": {"type": "number"}, "lon": {"type": "number"}},
          "required": ["lat", "lon"]
        }
      }
    ],
    "additional": {}
  },
  "self_hash": "<implementer fills in by calling PinnedContract::from_parts(url, contract).self_hash>"
}
```

The implementer should compute `self_hash` via `PinnedContract::from_parts(url, contract).self_hash` — the simplest way is to write a one-off Rust helper that constructs the contract, serializes it via `from_parts`, and prints the result; then commit the printed value as the fixture file.

- [ ] **Step 3: Add the second test** in `cmd_build_mcp.rs`:

```rust
#[test]
fn offline_build_against_pinned_contract_succeeds() {
    let fixture_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp_weather");
    let result = run_tau_build_in_fixture(&fixture_root, /* offline */ true);
    assert!(result.is_ok(), "build failed: {result:?}");
    // Read .tau/Tau.lock — confirm it's v7 with one MCP entry.
    let lock = std::fs::read_to_string(fixture_root.join(".tau/Tau.lock")).expect("lockfile written");
    assert!(lock.contains("schema_version = 7"));
    assert!(lock.contains("[[mcp]]"));
    assert!(lock.contains("entry = \"weather\""));
    assert!(lock.contains("[[mcp.expanded_tools]]"));
    assert!(lock.contains("name = \"get_forecast\""));
}
```

`run_tau_build_in_fixture` should invoke the same `cmd::build::run` function with a synthesized `BuildArgs { offline: true, .. }`.

- [ ] **Step 4: Run.**

```sh
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_build_mcp
```

Expected: pass.

- [ ] **Step 5: Commit.**

```sh
git add crates/tau-cli/tests/fixtures/mcp_weather crates/tau-cli/tests/cmd_build_mcp.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-cli/cmd/build): e2e fixture — mcp_weather + --offline against pinned contract emits v7 lockfile"
```

---

## Phase 8 — Workspace checks + push + PR + auto-merge

### Task 8.1: Workspace checks

- [ ] **Step 1: Full check / nextest / doc / clippy / fmt for every touched crate.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-mcp-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli

timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp --features with-std-adapters
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli

timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-mcp --features with-std-adapters
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-cli

timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp --features with-std-adapters --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings

timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-ir
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-mcp-tokio
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-pkg
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-cli
```

Expected: all green.

- [ ] **Step 2: Downstream canary.**

```sh
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-tokio
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-workflow
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-app 2>&1 || echo "tau-app may not exist on every branch — skip if absent"
```

Expected: clean.

- [ ] **Step 3: Apply fmt if anything's off.**

```sh
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ir -p tau-mcp -p tau-mcp-tokio -p tau-pkg -p tau-cli
git status
```

If any files changed:

```sh
git add -A
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "style(tau-ir,tau-mcp,tau-mcp-tokio,tau-pkg,tau-cli): apply cargo fmt"
```

### Task 8.2: Push + open PR + auto-merge

- [ ] **Step 1: Push.**

```sh
git push --no-verify -u origin feat/beta-3-pr-4-lowering
```

- [ ] **Step 2: Open the PR.**

```sh
gh pr create --title "β.3 MCP facilitator — PR-4: lowering + lockfile v7 + tau build wiring" --body "$(cat <<'EOF'
## Summary

Fourth of six PRs in the β.3 MCP facilitator sub-project. Integrates MCP contracts into the build pipeline:

- **`tau-ir`** — `ToolImpl::Mcp` gains `server_tool_name`. `lower/resolve.rs` gains the MCP expansion stage: one author entry (`weather`) → N IR nodes (`weather.get_forecast`, ...). New `McpBuildError` with all spec §5 invariants (ContractUnreachable, EnvelopeCoversContract, RootsExceedFsCaps, SamplingRequiredByContract, PinnedContractMissing, ServerToolNameContainsDot). `Caches::mcp_contract` evolves to return rich per-server-tool info.
- **`tau-mcp`** — NEW `contract::resolver` module: `McpContractResolver` port trait (sync) + `PinnedResolver` (reads `.tau/mcp/<entry>.contract.json`). Gated on `with-std-adapters`.
- **`tau-mcp-tokio`** — NEW `resolver` module: `LiveMcpContractResolver` (async; performs MCP handshake via `host_lifecycle::open` + captures `ServerContract`).
- **`tau-pkg`** — `UncheckedTool` + `ToolEntry` gain `sampling.models` + `roots` fields. URL-scheme validation in `validate_tool` (stdio/http/https only). Lockfile schema **v6 → v7** with new `LockedMcpEntry` + `LockedMcpExpandedTool` records.
- **`tau-cli/cmd/build.rs`** — pre-resolves all MCP contracts (pinned or live), passes the cache to `lower_project`, writes `.tau/mcp/<entry>.contract.json` on the live path, emits v7 lockfile with per-MCP-entry records. New `--offline` flag for pin-only builds.
- **~40 new tests** spread across the 5 crates + 1 sweep test confirming all existing fixtures still build green.

Spec: \`docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md\` §2/§4/§5/§6/§15
Plan: \`docs/superpowers/plans/2026-06-02-beta-3-mcp-facilitator-pr-4.md\`
Previous PR: #283 (β.3 PR-3).

Stacks-on: nothing (independent of PR-5/PR-6).

## Test plan

- [ ] All 5 crate nextest runs green
- [ ] All 4 crate doctests green
- [ ] All 5 crate clippy clean
- [ ] fmt clean
- [ ] Downstream canary (tau-runtime-tokio, tau-workflow) clean
- [ ] All existing fixtures still build green (sweep test)
- [ ] New e2e fixture (mcp_weather) builds via \`--offline\`
- [ ] CI green on linux / macos / windows

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Enroll auto-merge.**

```sh
gh pr merge <N> --auto
```

- [ ] **Step 4: Confirm queue enrollment.**

```sh
gh api graphql -f query='query{repository(owner:"tau-rs",name:"tau"){pullRequest(number:<N>){mergeQueueEntry{state position} autoMergeRequest{enabledAt}}}}'
```

- [ ] **Step 5: Watch CI. Re-enroll auto-merge if a check fails and you rerun.**

---

## Self-review checklist (run before declaring PR-4 done)

| Check | Status |
|---|---|
| `ToolImpl::Mcp` has `server_tool_name` field | Task 1.1 |
| `Caches::mcp_contract` signature is `Fn(&str) -> Option<ResolvedMcpContract>` | Task 1.3 |
| `lower/resolve.rs` expands one MCP entry into N IR nodes; rewrites agent `tool_refs` | Task 1.4 |
| `McpBuildError` carries all spec §5 invariant variants | Task 1.2 |
| `tau-mcp::contract::resolver` exports `McpContractResolver` trait + `ResolvedMcpContract` + `ResolvedServerTool` + `PinnedResolver` + `ResolveError` | Task 2.1 |
| `PinnedResolver` is gated on `with-std-adapters` | Task 2.1 |
| `UncheckedTool` + `ToolEntry` carry `sampling: Option<SamplingConfig>` + `roots: Vec<PathBuf>` | Task 3.1 |
| `validate_tool` rejects unsupported MCP URL schemes | Task 3.2 |
| `MAX_SUPPORTED_LOCKFILE_SCHEMA_VERSION` is 7 | Task 4.1 |
| `LockFile` has `mcp_entries: Vec<LockedMcpEntry>` (with `#[serde(default)]`) | Task 4.1 |
| v6 lockfile auto-upgrades to v7 with empty `mcp_entries`; v7 round-trips with MCP entries | Task 4.2 |
| `tau-mcp-tokio::resolver::resolve_all` opens each URL via `host_lifecycle::open`, captures ServerContract, builds the cache map | Task 5.1 |
| `tau-cli/cmd/build.rs` has `--offline` flag | Task 6.1 |
| Live path writes `.tau/mcp/<entry>.contract.json` for each entry | Task 6.1 |
| Lockfile populated with `LockedMcpEntry` per entry on build | Task 6.1 |
| All existing fixtures still build green | Task 7.1 |
| New mcp_weather fixture builds via `--offline` against committed pinned contract | Task 7.2 |
| All 5 crate clippy + fmt clean | Task 8.1 |
| Downstream canary clean | Task 8.1 |
| Push used `--no-verify` | Task 8.2 |
| Auto-merge enrolled via BARE `gh pr merge <N> --auto` | Task 8.2 |
| Queue enrollment confirmed via mergeQueueEntry GraphQL | Task 8.2 |

---

## What's next: PR-5 + PR-6

PR-5 (McpBridge + sampling/roots inbound handlers + runtime drift check + bundle dispatch) stacks on PR-4: it consumes `tau_mcp::contract::resolver`'s output to build the runtime `McpBridge` and uses `tau_mcp-tokio::resolver` to verify lockfile contract_hash vs live handshake at boot.

PR-6 (CLI verbs + conformance fixture #07 + ADR-0038 finalize + docs) stacks on PR-4 + PR-5: ships `tau mcp {pin,ls,show,refresh,diff}` (reusing PR-4's PinnedResolver), the conformance fixture (using PR-3's CassetteTransport), and finalizes the ADR.

PR-4 lessons to fold forward:
- The `Caches` evolution pattern (sync cache, async pre-fetch, port trait in tau-mcp + impl in tau-mcp-tokio) is the reference shape for any future build-time resolver (skills resolver from Skills-2, etc.).
- Lockfile bumps are mechanical when the new fields use `#[serde(default)]`; the existing v4→v5→v6 ladder makes v6→v7 a single-line change at line 587-589.
- Per-crate clippy + nextest scales linearly; expect Phase 8's workspace check to take ~5min on a warm CARGO_TARGET_DIR.
