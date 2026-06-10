# β.3 PR-6 — MCP CLI verbs + conformance + ADR-0038 finalize + docs

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the β.3 MCP facilitator sub-project by shipping the user-facing CLI (`tau mcp {pin, ls, show, refresh, diff}`), the cassette URL scheme, a `tau check mcp_contracts` aggregator phase, conformance fixture #07, the finalized ADR-0038, and two mdBook pages.

**Architecture:** Five new `tau mcp` verbs share a `cmd/mcp/{mod,pin,ls,show,refresh,diff}.rs` module modelled on the existing `cmd/skill/` layout (Skills-3 PR #66). Pin/refresh produce `.tau/mcp/<name>.contract.json` files via the existing `PinnedContract::from_parts` API; show/ls/diff read them. The cassette URL scheme adds `McpUrl::Cassette { path }` + `cassette_dial::dial()` so `host_lifecycle::open()` can dispatch `cassette:<path>` URLs through the `CassetteTransport` shipped in PR-3. Conformance fixture #07 exercises the full cassette-replay path under both DevMode and BundleMode via the β.2 conformance harness.

**Tech Stack:** Rust 1.84+, tokio, anyhow, serde / serde_json, clap. Existing crates touched: `tau-mcp-tokio` (URL + dial), `tau-mcp` (PinnedContract — already done), `tau-cli` (5 verbs + check phase), `tau-ir-conformance` (fixture #07), `tau-pkg` (lockfile readers — already done by PR-4). Docs: mdBook + Diátaxis.

**Branch:** `feat/beta-3-pr-6-mcp-cli`
**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/beta-3-pr-6-mcp-cli` (off `origin/main` at `dff9570`)
**Spec:** `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` — §10 (CLI surface), §11 (cassette format — shipped PR-3), §12 (testing strategy), §15 row "PR-6".
**ADR (placeholder shipped in PR-1, finalized here):** `docs/decisions/ADR-0038-mcp-facilitator.md`

---

## Locked design decisions

These decisions were approved on 2026-06-10 in the brainstorm/design discussion. **This plan IS the PR-6 design record** — no separate spec edit needed beyond ADR-0038 finalize in Phase 6.

| # | Decision | Rationale |
|---|---|---|
| 1 | `tau mcp pin <name> [--from URL]` accepts an optional URL override. When omitted, the URL comes from `[tools.<name>] mcp = "..."` in `tau.toml`. The `--from URL` may carry any supported scheme (stdio/http/https/cassette) — the same `parse_url()` path handles all four. | Lets the user pin a cassette during local dev without editing `tau.toml`. |
| 2 | New `cassette:<path>` URL scheme — `McpUrl::Cassette { path: PathBuf }` variant in `parse_url`, plus `cassette_dial::dial()` so `open()` can dispatch through `tau_mcp::cassette::transport::CassetteTransport` (shipped PR-3). Path may be relative to project root or absolute; whitespace trimmed; empty path rejected. | Was deferred from PR-5. Wiring it in PR-6 makes fixture #07 (and any user-recorded cassette) directly addressable via `[tools.<name>] mcp = "cassette:..."`. |
| 3 | `tau mcp show <name>` supports `--human` (default), `--json`, and `--sarif`. SARIF is a single-rule, zero-results, vacuously-valid SARIF 2.1.0 document, consistent with `tau check`'s output options shipped in PR #161. | Pre-flight tooling wants machine-readable status from every tau verb. |
| 4 | ADR-0038 was a placeholder shipped in PR-1; PR-6 finalizes it with the as-shipped reality (5 transports — stdio, http, https, cassette — counted as 3 user-visible URL families; 3 scanner configs; lockfile v7; conformance fixture #07; full CLI surface). | Closes the β.3 doc trail. |
| 5 | Conformance fixture #07 lives at `crates/tau-ir-conformance/fixtures/07_mcp_weather/`. Cassette-replay weather scenario. Cross-mode (DevMode + BundleMode) via the β.2 conformance harness. | Locks the end-to-end pipeline (URL → CassetteTransport → handshake → tool call → IR step result) in a single executable test. |
| 6 | Two mdBook pages: `docs/how-to/mcp-servers.md` (Diátaxis "how-to" — add a server, pin + refresh) and `docs/reference/tau-mcp.md` (Diátaxis "reference" — every verb's flags + JSON schema). Both added to `docs/SUMMARY.md`. | Ships the docs gap left open since PR-1. |

---

## Files map

### Create
| Path | Purpose |
|---|---|
| `crates/tau-mcp-tokio/src/host_lifecycle/cassette_dial.rs` | `dial(path, options) -> impl McpTransport` over `CassetteTransport` |
| `crates/tau-cli/src/cmd/mcp/mod.rs` | dispatch + shared `OutputFormat` enum + JSON/SARIF render helpers |
| `crates/tau-cli/src/cmd/mcp/pin.rs` | `tau mcp pin <name> [--from URL]` |
| `crates/tau-cli/src/cmd/mcp/ls.rs` | `tau mcp ls` |
| `crates/tau-cli/src/cmd/mcp/show.rs` | `tau mcp show <name> [--json\|--sarif]` |
| `crates/tau-cli/src/cmd/mcp/refresh.rs` | `tau mcp refresh <name>` |
| `crates/tau-cli/src/cmd/mcp/diff.rs` | `tau mcp diff <name>` |
| `crates/tau-cli/src/cmd/check/categories/mcp_contracts.rs` | new check aggregator phase |
| `crates/tau-ir-conformance/fixtures/07_mcp_weather/workflow.toml` | fixture workflow |
| `crates/tau-ir-conformance/fixtures/07_mcp_weather/weather_cassette.jsonl` | cassette payload |
| `crates/tau-ir-conformance/fixtures/07_mcp_weather/expected_report.json` | expected output |
| `crates/tau-ir-conformance/fixtures/07_mcp_weather/.tau/mcp/weather.contract.json` | pinned contract |
| `docs/decisions/ADR-0038-mcp-facilitator.md` | OVERWRITE the placeholder with finalized content |
| `docs/how-to/mcp-servers.md` | Diátaxis how-to |
| `docs/reference/tau-mcp.md` | Diátaxis reference |

### Modify
| Path | Lines / purpose |
|---|---|
| `crates/tau-mcp-tokio/src/host_lifecycle/url.rs` | add `Cassette { path: PathBuf }` variant + parse_url arm + 4 tests |
| `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs` | re-export `cassette_dial` |
| `crates/tau-mcp-tokio/src/host_lifecycle/open.rs` | add Cassette match arm calling `cassette_dial::dial` |
| `crates/tau-cli/src/cli.rs` | add `Mcp(McpSubcommand)` variant + `McpSubcommand` enum + flag structs |
| `crates/tau-cli/src/cmd/mod.rs` | declare `pub mod mcp;` + route from `Commands::Mcp` |
| `crates/tau-cli/src/main.rs` | dispatch arm `Commands::Mcp(sub) => mcp::dispatch(sub, &mut output).await` |
| `crates/tau-cli/src/cmd/check/mod.rs` | add `McpContracts` to `CheckCategory` listing |
| `crates/tau-cli/src/cmd/check/categories/mod.rs` | declare `pub mod mcp_contracts;` |
| `crates/tau-cli/src/cmd/check/runner.rs` | route `CheckCategory::McpContracts` → `mcp_contracts::run` |
| `crates/tau-ir-conformance/tests/cross_mode.rs` (or whichever file enumerates fixtures) | add `07_mcp_weather` to the fixture list |
| `docs/SUMMARY.md` | add lines for the 2 new mdBook pages |

---

## Standing constraints (CLAUDE.md — NON-NEGOTIABLE)

- **Cargo:** `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <subcmd> -p <crate>`. Never bare `cargo`. Never `--workspace`. Always `-p`.
- **Commits:** `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` (lefthook test-native corrupts git identity).
- **Push:** `git push --no-verify -u origin feat/beta-3-pr-6-mcp-cli` (avoid silent-kill).
- **Auto-merge:** `gh pr merge <N> --auto` BARE (repo is on the merge queue — `--squash`/`--delete-branch` are rejected by the queue runner).
- **Worktree:** Operate inside `/Users/titouanlebocq/code/tau-worktrees/beta-3-pr-6-mcp-cli` only. Never `cd` to the original repo root.

### Lessons from PR-2/3/4/5/CI redesign — DO / DON'T

1. **DON'T** add `features = ["test-support"]` to any `tau-runtime-tokio` dev-dep. Workspace feature unification activates it everywhere and breaks `plugin_host_ipc_llm.rs`.
2. **DO** use `Option::is_some_and(...)` over `map_or(false, ...)` — CI's stable rustc surfaces `clippy::unnecessary_map_or` that local rustc may miss.
3. **DO** add explicit `::new()` constructors for any `#[non_exhaustive]` types you construct in test code.
4. **DO** rerun and re-enrol auto-merge on macOS infra flakes (`chat_ephemeral_writes_no_file`, `echo-tool` fixture race, `child_crash_mid_call_surfaces_transport_error`).
5. **DON'T** add `[[profile.ci.overrides]]` to `.config/nextest.toml` with `package(__placeholder__)` filters — nextest validates every override at parse time, even no-match ones. Just don't add the override.
6. **DON'T** add new branch-protection-required CI checks. Only `ci-summary` is the gate. PR-6 adds zero new workflow files.
7. **DO** model the cassette dial path on PR-3's `tests/cassette_transport.rs` test — `CassetteTransport` has a recv-loop quirk where it consumes responses linearly, so the dial helper must hand the transport into `McpClient::new` without driving its own handshake-driver-on-a-side-channel.

---

## Phase 1 — `tau-mcp-tokio` cassette URL scheme + dial helper

**Goal:** `parse_url("cassette:./foo.jsonl")` returns `McpUrl::Cassette { path: PathBuf::from("./foo.jsonl") }`, and `open("cassette:...", plan, gate, options)` returns a live `McpClient` reading from the cassette.

### Task 1.1 — Add `McpUrl::Cassette` variant + `parse_url` arm + 4 tests

**Files:**
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/url.rs`

- [ ] **Step 1: Read the current file** — start by reading `crates/tau-mcp-tokio/src/host_lifecycle/url.rs` to ground the diff. The current `McpUrl` enum has 3 variants (`Stdio`, `Http`, `Https`) and 8 tests.

- [ ] **Step 2: Write the failing tests at the bottom of `mod tests`**

```rust
#[test]
fn cassette_relative_path_parses() {
    let url = parse_url("cassette:./fixtures/weather.jsonl").expect("parse");
    match url {
        McpUrl::Cassette { path } => {
            assert_eq!(path, std::path::PathBuf::from("./fixtures/weather.jsonl"));
        }
        other => panic!("expected Cassette, got {other:?}"),
    }
}

#[test]
fn cassette_absolute_path_parses() {
    let url = parse_url("cassette:/tmp/x.jsonl").expect("parse");
    match url {
        McpUrl::Cassette { path } => {
            assert_eq!(path, std::path::PathBuf::from("/tmp/x.jsonl"));
        }
        other => panic!("expected Cassette, got {other:?}"),
    }
}

#[test]
fn cassette_empty_path_rejected() {
    let err = parse_url("cassette:").expect_err("should reject");
    match err {
        UrlParseError::EmptyCassettePath => {}
        other => panic!("expected EmptyCassettePath, got {other:?}"),
    }
    assert!(matches!(
        parse_url("cassette:   "),
        Err(UrlParseError::EmptyCassettePath)
    ));
}

#[test]
fn cassette_path_trimmed() {
    let url = parse_url("cassette:   ./x.jsonl   ").expect("parse");
    match url {
        McpUrl::Cassette { path } => {
            assert_eq!(path, std::path::PathBuf::from("./x.jsonl"));
        }
        other => panic!("expected Cassette, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run the tests to confirm failure**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-mcp-tokio --lib host_lifecycle::url -- --nocapture
```
Expected: 4 fails. `Cassette` variant not found; `EmptyCassettePath` variant not found.

- [ ] **Step 4: Add the variant + parse_url arm + new error variant**

In `crates/tau-mcp-tokio/src/host_lifecycle/url.rs`, add to the enum and parser. The Cassette arm goes BEFORE the http arm (matched by `strip_prefix("cassette:")`).

```rust
// Add at top of file with the other imports:
use std::path::PathBuf;

// In enum McpUrl, add as a new variant after Https:
    /// Recorded MCP traffic replayed from a JSONL cassette.
    /// Path is left as-given (relative to project root or absolute);
    /// resolution to filesystem happens in `cassette_dial::dial`.
    Cassette {
        /// Path to the cassette file.
        path: PathBuf,
    },

// In parse_url, add this arm BEFORE the http(s) arm:
    if let Some(rest) = s.strip_prefix("cassette:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(UrlParseError::EmptyCassettePath);
        }
        return Ok(McpUrl::Cassette {
            path: PathBuf::from(rest),
        });
    }
```

Then in `crates/tau-mcp-tokio/src/host_lifecycle/error.rs`, add the new `UrlParseError` variant. (READ the file first to see the existing variants — pattern after `EmptyStdioCommand`.)

```rust
/// `cassette:` URL had no path after the prefix (e.g. `cassette:`).
#[error("cassette URL has empty path")]
EmptyCassettePath,
```

- [ ] **Step 5: Run the tests to confirm pass**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-mcp-tokio --lib host_lifecycle::url
```
Expected: 12 pass (8 existing + 4 new).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-mcp-tokio/src/host_lifecycle/url.rs \
        crates/tau-mcp-tokio/src/host_lifecycle/error.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-mcp-tokio): McpUrl::Cassette variant + parse_url arm"
```

### Task 1.2 — `cassette_dial.rs` + `open()` dispatch arm + 2 dial tests

**Files:**
- Create: `crates/tau-mcp-tokio/src/host_lifecycle/cassette_dial.rs`
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/mod.rs` (add `pub mod cassette_dial;`)
- Modify: `crates/tau-mcp-tokio/src/host_lifecycle/open.rs` (add `Cassette` match arm)

- [ ] **Step 1: Read PR-3's cassette transport test** — `crates/tau-mcp-tokio/tests/cassette_transport.rs` shows the working pattern for how `CassetteTransport` is constructed and handed to `McpClient`. Mirror that shape in `cassette_dial::dial`.

- [ ] **Step 2: Read the `CassetteTransport` API** — `crates/tau-mcp/src/cassette/transport.rs`. Note whether it constructs from a file path or from a `Read` impl. (The current API takes a parsed cassette + plays it back; the dial helper is responsible for the file I/O.)

- [ ] **Step 3: Write the failing test** in `crates/tau-mcp-tokio/tests/cassette_dial.rs` (new integration test file).

```rust
//! Integration tests for `host_lifecycle::cassette_dial::dial`.

use std::sync::Arc;
use tau_mcp_tokio::host_lifecycle::client::McpClientOptions;
use tau_mcp_tokio::host_lifecycle::open::open;
use tau_ports::CapabilityPlan;
use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;

fn empty_gate() -> Arc<dyn DynProcessCapabilityGate> {
    // Use whatever permissive-gate constructor exists in test-support.
    // If none is exported, construct an ad-hoc one (see PR-2's tests for
    // the established pattern — DO NOT enable features=["test-support"]
    // on tau-runtime-tokio; see lesson #1).
    todo!("read tau-runtime-tokio for the permissive gate test helper")
}

#[tokio::test]
async fn open_cassette_replays_handshake() {
    let cassette = "tests/fixtures/weather_minimal_cassette.jsonl";
    let plan = CapabilityPlan::default();
    let gate = empty_gate();
    let client = open(
        &format!("cassette:{cassette}"),
        &plan,
        gate,
        McpClientOptions::default(),
    )
    .await
    .expect("open cassette");
    assert!(!client.contract().tools.is_empty(),
        "handshake should yield at least one tool");
}

#[tokio::test]
async fn cassette_missing_file_errors() {
    let plan = CapabilityPlan::default();
    let gate = empty_gate();
    let err = open(
        "cassette:/nonexistent/file.jsonl",
        &plan,
        gate,
        McpClientOptions::default(),
    )
    .await
    .expect_err("should fail");
    // The error chain should mention the path so the user can act on it.
    let msg = format!("{err}");
    assert!(msg.contains("nonexistent") || msg.contains("file.jsonl"),
        "expected path in error, got: {msg}");
}
```

Then put a minimal cassette at `crates/tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl` — copy PR-3's `cassette_vector.jsonl` test fixture and trim to just `initialize` request/response.

- [ ] **Step 4: Run the tests to confirm failure**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-mcp-tokio --test cassette_dial -- --nocapture
```
Expected: fails — `cassette_dial` module not found.

- [ ] **Step 5: Implement `cassette_dial.rs`**

```rust
//! Dial a cassette as if it were a live MCP server.
//!
//! Reads a JSONL cassette from disk, constructs a `CassetteTransport`,
//! and wraps it as a `dyn McpTransport` so `host_lifecycle::open()` can
//! drive the handshake uniformly with stdio + HTTP paths.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tau_mcp::cassette::transport::CassetteTransport;
use tracing::{info, instrument};

use crate::host_lifecycle::error::LifecycleError;

/// Options for cassette dial. v0 is empty; reserved for future
/// (e.g. clock-replay scaling, partial-cassette tolerance).
#[derive(Debug, Default, Clone)]
pub struct CassetteDialOptions {}

/// Dial a cassette file and return a transport ready for handshake.
///
/// The path is interpreted relative to the caller's CWD if relative,
/// or used as-is if absolute. The cassette is fully loaded into memory
/// at dial time (cassettes are small).
#[instrument(name = "cassette_dial", skip(_options), fields(path = %path.display()))]
pub fn dial(
    path: &Path,
    _options: CassetteDialOptions,
) -> Result<Arc<CassetteTransport>, LifecycleError> {
    info!("dialing cassette");
    let bytes = std::fs::read(path).map_err(|e| LifecycleError::Io {
        path: PathBuf::from(path),
        source: e,
    })?;
    let transport = CassetteTransport::from_jsonl_bytes(&bytes)
        .map_err(|e| LifecycleError::CassetteParse { source: e })?;
    Ok(Arc::new(transport))
}
```

Add the two new `LifecycleError` variants in `crates/tau-mcp-tokio/src/host_lifecycle/error.rs` (READ the file first — pattern after the existing variants):

```rust
#[error("cassette IO error at {path}")]
Io {
    path: std::path::PathBuf,
    #[source]
    source: std::io::Error,
},
#[error("cassette parse error: {source}")]
CassetteParse {
    #[source]
    source: tau_mcp::cassette::CassetteError,
},
```

(If the `tau-mcp` error type isn't named `CassetteError`, READ `crates/tau-mcp/src/cassette/mod.rs` for the actual name and adjust.)

- [ ] **Step 6: Wire `cassette_dial` into `host_lifecycle/mod.rs`**

Add `pub mod cassette_dial;` next to the other `pub mod` declarations.

- [ ] **Step 7: Add the dispatch arm in `open.rs`**

```rust
// In the match in open():
        McpUrl::Cassette { path } => open_cassette(&path, options).await,

// Add a new fn alongside open_stdio + open_http:
async fn open_cassette(
    path: &std::path::Path,
    options: McpClientOptions,
) -> Result<McpClient, LifecycleError> {
    info!(cassette_path = %path.display(), "dialing cassette MCP server");
    let transport = cassette_dial::dial(path, cassette_dial::CassetteDialOptions::default())?;
    let contract = drive_handshake(&*transport, &options.handshake).await?;
    info!(
        server_name = %contract.server_info.name,
        tools_count = contract.tools.len(),
        "MCP handshake complete (cassette)"
    );
    Ok(McpClient::new(transport, contract, options))
}
```

Add `use crate::host_lifecycle::cassette_dial;` at the top.

**NOTE on `Arc<CassetteTransport>` vs the `Box<dyn McpTransport>` shape `open_stdio` produces:** `McpClient::new` is generic over `T: McpTransport`, so `Arc<CassetteTransport>` works as long as `Arc<CassetteTransport>` implements `McpTransport`. If it doesn't (only `&CassetteTransport` does), wrap with `Box::new(CassetteTransport::from_jsonl_bytes(...))` instead and return `Box<CassetteTransport>` from `dial`. READ `crates/tau-mcp-tokio/src/host_lifecycle/client.rs` for the trait bound on `McpClient::new` before finalizing.

- [ ] **Step 8: Run the dial tests + the full lib test to confirm no regression**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-mcp-tokio
```
Expected: all previous tests still pass + 2 new dial tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/tau-mcp-tokio/src/host_lifecycle/cassette_dial.rs \
        crates/tau-mcp-tokio/src/host_lifecycle/mod.rs \
        crates/tau-mcp-tokio/src/host_lifecycle/open.rs \
        crates/tau-mcp-tokio/src/host_lifecycle/error.rs \
        crates/tau-mcp-tokio/tests/cassette_dial.rs \
        crates/tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-mcp-tokio): cassette_dial + open() dispatch arm"
```

---

## Phase 2 — `tau-cli` `cmd/mcp/` module scaffold + `pin` + `ls`

**Goal:** `tau mcp pin <name> [--from URL]` writes `.tau/mcp/<name>.contract.json`. `tau mcp ls` enumerates pinned contracts. Shared output format scaffolding sits in `cmd/mcp/mod.rs`.

### Task 2.1 — Scaffold `cmd/mcp/mod.rs` + CLI wiring + dispatch test

**Files:**
- Create: `crates/tau-cli/src/cmd/mcp/mod.rs`
- Modify: `crates/tau-cli/src/cli.rs`
- Modify: `crates/tau-cli/src/cmd/mod.rs`
- Modify: `crates/tau-cli/src/main.rs`

- [ ] **Step 1: Read the established pattern** — `crates/tau-cli/src/cmd/skill/mod.rs` shows the dispatch shape. The `tau skill <sub>` verbs use a `SkillSubcommand` enum in `cli.rs` and a `dispatch(sub, &mut output)` fn here. Mirror it.

- [ ] **Step 2: Write a failing CLI integration test** at `crates/tau-cli/tests/mcp_dispatch.rs`:

```rust
//! Smoke test: `tau mcp --help` and the 5 sub-verbs are dispatchable.

use assert_cmd::Command;

#[test]
fn mcp_help_lists_five_verbs() {
    let output = Command::cargo_bin("tau")
        .expect("binary")
        .args(["mcp", "--help"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for verb in ["pin", "ls", "show", "refresh", "diff"] {
        assert!(stdout.contains(verb), "expected `{verb}` in: {stdout}");
    }
}
```

(Use `assert_cmd` — already a dev-dep on tau-cli per PR-2/PR-3 history. If not, ADD it as a `dev-dependencies` line in `crates/tau-cli/Cargo.toml`.)

- [ ] **Step 3: Run** to confirm fail

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-cli --test mcp_dispatch
```
Expected: `mcp` subcommand not found.

- [ ] **Step 4: Add the CLI enum in `cli.rs`** — READ `crates/tau-cli/src/cli.rs` first to find the `Commands` enum + see how `Skill(SkillArgs)` is declared.

```rust
// In the Commands enum, add:
    /// Manage Model Context Protocol (MCP) server contracts.
    #[command(subcommand)]
    Mcp(McpSubcommand),

// At the bottom of the file (or in a new section), add:
#[derive(Debug, clap::Subcommand)]
pub enum McpSubcommand {
    /// Probe a server and write its contract to `.tau/mcp/<name>.contract.json`.
    Pin(McpPinArgs),
    /// List pinned MCP contracts in the current project.
    Ls(McpLsArgs),
    /// Show a pinned contract (human / JSON / SARIF).
    Show(McpShowArgs),
    /// Re-probe a server and overwrite the existing pin file.
    Refresh(McpRefreshArgs),
    /// Diff a pinned contract against a live probe (read-only).
    Diff(McpDiffArgs),
}

#[derive(Debug, clap::Args)]
pub struct McpPinArgs {
    /// Tool name (must match a `[tools.<name>]` block in tau.toml).
    pub name: String,
    /// Override the URL (defaults to `[tools.<name>] mcp = "..."`).
    /// Accepts stdio:, http://, https://, or cassette: URLs.
    #[arg(long, value_name = "URL")]
    pub from: Option<String>,
    /// Emit machine-readable JSON instead of human output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct McpLsArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct McpShowArgs {
    /// Tool name.
    pub name: String,
    /// Emit machine-readable JSON.
    #[arg(long, conflicts_with = "sarif")]
    pub json: bool,
    /// Emit SARIF 2.1.0 (single-rule, zero-results vacuous doc).
    #[arg(long, conflicts_with = "json")]
    pub sarif: bool,
}

#[derive(Debug, clap::Args)]
pub struct McpRefreshArgs {
    /// Tool name.
    pub name: String,
    /// Emit machine-readable JSON describing the diff.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct McpDiffArgs {
    /// Tool name.
    pub name: String,
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}
```

- [ ] **Step 5: Create `cmd/mcp/mod.rs`** with the dispatch fn + the shared output helpers.

```rust
//! `tau mcp <subcommand>` — manage MCP server contracts.
//!
//! See spec at `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
//! §10 (CLI surface) and ADR-0038.

pub mod diff;
pub mod ls;
pub mod pin;
pub mod refresh;
pub mod show;

use crate::cli::McpSubcommand;
use crate::output::Output;

/// One-shot output format selector. Used by `show`; `pin`/`refresh`
/// have their own bool flags but funnel through the same renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
    Sarif,
}

impl OutputFormat {
    /// Build from `--json` / `--sarif` flag pair. Validated as
    /// mutually exclusive at the clap layer.
    pub fn from_flags(json: bool, sarif: bool) -> Self {
        match (json, sarif) {
            (true, false) => Self::Json,
            (false, true) => Self::Sarif,
            _ => Self::Human,
        }
    }
}

/// Render an arbitrary serializable value as a SARIF 2.1.0 document.
/// Single tool ("tau-mcp"), single rule (the verb name), zero results.
/// Consistent with `tau check --sarif` from PR #161.
pub fn render_sarif(rule_id: &str, embedded_payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "tau-mcp",
                    "informationUri": "https://github.com/LEBOCQTitouan/tau",
                    "rules": [{ "id": rule_id }],
                }
            },
            "results": [],
            "properties": { "embedded": embedded_payload },
        }],
    })
}

/// Route `tau mcp <subcommand>` to its impl module.
pub async fn dispatch(sub: McpSubcommand, output: &mut Output) -> anyhow::Result<()> {
    match sub {
        McpSubcommand::Pin(args) => pin::run(args, output).await,
        McpSubcommand::Ls(args) => ls::run(args, output).await,
        McpSubcommand::Show(args) => show::run(args, output).await,
        McpSubcommand::Refresh(args) => refresh::run(args, output).await,
        McpSubcommand::Diff(args) => diff::run(args, output).await,
    }
}
```

- [ ] **Step 6: Add stub module files** (one-liner each so the build links). For each of `pin.rs`, `ls.rs`, `show.rs`, `refresh.rs`, `diff.rs`, write:

```rust
//! Stub — implemented in Task 2.2 / 2.3 / 3.x.
use crate::cli::*;
use crate::output::Output;

pub async fn run(_args: McpPinArgs, _output: &mut Output) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented")
}
```

(Substitute the right Args type per file: `McpLsArgs`, `McpShowArgs`, `McpRefreshArgs`, `McpDiffArgs`.)

- [ ] **Step 7: Wire `cmd/mod.rs`** — add `pub mod mcp;`.

- [ ] **Step 8: Dispatch from `main.rs`** — add to the `match cli.command` block:

```rust
Commands::Mcp(sub) => cmd::mcp::dispatch(sub, &mut output).await?,
```

- [ ] **Step 9: Run the test to confirm pass**

```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo test -p tau-cli --test mcp_dispatch
```
Expected: `mcp_help_lists_five_verbs` passes.

- [ ] **Step 10: Commit**

```bash
git add crates/tau-cli/src/cli.rs \
        crates/tau-cli/src/main.rs \
        crates/tau-cli/src/cmd/mod.rs \
        crates/tau-cli/src/cmd/mcp/ \
        crates/tau-cli/tests/mcp_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): mcp subcommand scaffold + 5 verb stubs"
```

### Task 2.2 — Implement `pin.rs` + 2 tests

**Files:**
- Modify: `crates/tau-cli/src/cmd/mcp/pin.rs`
- Modify: `crates/tau-cli/tests/mcp_dispatch.rs` (add `pin` tests)

- [ ] **Step 1: Add failing tests**

```rust
// In tests/mcp_dispatch.rs, add:

use assert_fs::prelude::*;
use predicates::str::contains;

#[tokio::test]
async fn pin_writes_contract_file_for_cassette_tool() {
    // Use a tiny in-test project with a cassette tool.
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    // Copy the minimal weather cassette into the project.
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read fixture"))
        .expect("write fixture");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "pin-test"
version = "0.0.1"

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
"#).expect("write tau.toml");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert()
        .success();

    let pinned = tmp.child(".tau/mcp/weather.contract.json");
    pinned.assert(predicates::path::is_file());
    let content = std::fs::read_to_string(pinned.path()).expect("read");
    assert!(content.contains("\"schema_version\":1"), "got: {content}");
    assert!(content.contains("\"url\":\"cassette:"), "got: {content}");
    assert!(content.contains("\"contract_hash_hex\":\""), "got: {content}");
}

#[tokio::test]
async fn pin_with_from_override_uses_override_url() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read"))
        .expect("write");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "pin-test"
version = "0.0.1"

[tools.weather]
mcp = "stdio:nonexistent-binary"
"#).expect("write tau.toml");

    let override_url = "cassette:./fixtures/weather.jsonl";
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "pin", "weather", "--from", override_url])
        .assert()
        .success();
    let content = std::fs::read_to_string(tmp.child(".tau/mcp/weather.contract.json").path())
        .expect("read");
    assert!(content.contains(override_url), "got: {content}");
}
```

(Add `assert_fs`, `predicates` to dev-dependencies if missing — they're standard in the workspace.)

- [ ] **Step 2: Run** to confirm fail. Expected: tests fail, exits with non-zero + the bail message.

- [ ] **Step 3: Implement `pin.rs`**

```rust
//! `tau mcp pin <name> [--from URL]` — probe a server and write its
//! contract to `.tau/mcp/<name>.contract.json`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tau_mcp::contract::pinned::PinnedContract;
use tau_mcp_tokio::host_lifecycle::client::McpClientOptions;
use tau_mcp_tokio::host_lifecycle::open::open;
use tau_pkg::config::load_project_config;
use tau_ports::CapabilityPlan;

use crate::cli::McpPinArgs;
use crate::output::Output;

pub async fn run(args: McpPinArgs, output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let project = load_project_config(&project_root)
        .with_context(|| format!("load tau.toml at {}", project_root.display()))?;

    // Resolve the URL: --from override wins, else the tau.toml tool block.
    let url = if let Some(override_url) = args.from {
        override_url
    } else {
        project
            .tool(&args.name)
            .ok_or_else(|| anyhow!("no [tools.{}] block in tau.toml", &args.name))?
            .mcp
            .clone()
            .ok_or_else(|| anyhow!("tool `{}` has no `mcp = \"...\"` field", &args.name))?
    };

    // Drive a fresh probe.
    let gate = build_permissive_gate(&project).await?;
    let client = open(&url, &CapabilityPlan::default(), gate, McpClientOptions::default())
        .await
        .with_context(|| format!("open MCP server at {url}"))?;
    let contract = client.contract().clone();
    let pinned = PinnedContract::from_parts(url.clone(), contract)
        .map_err(|e| anyhow!("build pinned contract: {e}"))?;

    // Persist.
    let pin_dir = project_root.join(".tau/mcp");
    std::fs::create_dir_all(&pin_dir)
        .with_context(|| format!("create {}", pin_dir.display()))?;
    let pin_path = pin_dir.join(format!("{}.contract.json", &args.name));
    let bytes = serde_json::to_vec_pretty(&pinned).context("serialize pinned contract")?;
    std::fs::write(&pin_path, &bytes)
        .with_context(|| format!("write {}", pin_path.display()))?;

    if args.json {
        let payload = serde_json::json!({
            "ok": true,
            "name": args.name,
            "path": pin_path,
            "url": url,
            "contract_hash_hex": pinned.contract_hash_hex,
            "tools_count": pinned.contract.tools.len(),
        });
        output.println(&serde_json::to_string_pretty(&payload)?);
    } else {
        output.println(&format!(
            "pinned `{}` from {} → {} ({} tools, hash {})",
            args.name,
            url,
            pin_path.display(),
            pinned.contract.tools.len(),
            &pinned.contract_hash_hex[..16],
        ));
    }
    Ok(())
}

async fn build_permissive_gate(
    _project: &tau_pkg::config::ProjectConfig,
) -> Result<Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate>> {
    // PR-6 v0: use the runtime-tokio default gate construction path that
    // tau run uses. Read `crates/tau-cli/src/cmd/run.rs` for the
    // established helper, OR construct ad-hoc from `tau-sandbox-*::permissive`.
    todo!("read tau-cli/src/cmd/run.rs for the gate builder helper")
}
```

(The `todo!()` is a placeholder for the implementer to fill in by reading `crates/tau-cli/src/cmd/run.rs` — the gate-build path already exists; PR-6 should reuse it, not invent a new one.)

The `load_project_config` + `project.tool(name)` API may not exist exactly as shown — READ `crates/tau-pkg/src/config/mod.rs` (or wherever `ProjectConfig` lives) to find the actual accessors. PR-4 / PR-5 added MCP-related helpers; mirror those.

- [ ] **Step 4: Run** to confirm pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/mcp/pin.rs \
        crates/tau-cli/tests/mcp_dispatch.rs \
        crates/tau-cli/Cargo.toml
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): mcp pin verb writes .tau/mcp/<name>.contract.json"
```

### Task 2.3 — Implement `ls.rs` + 2 tests

**Files:**
- Modify: `crates/tau-cli/src/cmd/mcp/ls.rs`
- Modify: `crates/tau-cli/tests/mcp_dispatch.rs`

- [ ] **Step 1: Add failing tests**

```rust
#[tokio::test]
async fn ls_empty_project_returns_zero_pins() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "ls-test"
version = "0.0.1"
"#).expect("write");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "ls", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"pins\":[]"));
}

#[tokio::test]
async fn ls_lists_existing_pin_files() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("tau.toml").write_str(r#"
[project]
name = "ls-test"
version = "0.0.1"
"#).expect("write");
    // Write a hand-crafted minimal pin file.
    let pin = serde_json::json!({
        "schema_version": 1,
        "url": "stdio:echo",
        "contract_hash_hex": "00".repeat(32),
        "contract": {
            "protocol_version": "2025-03-26",
            "server_info": {"name": "weather", "version": "1.0"},
            "tools": [],
        }
    });
    tmp.child(".tau/mcp/weather.contract.json")
        .write_str(&serde_json::to_string(&pin).unwrap())
        .expect("write");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "ls"])
        .assert()
        .success()
        .stdout(predicates::str::contains("weather"));
}
```

- [ ] **Step 2: Run** to confirm fail.

- [ ] **Step 3: Implement `ls.rs`**

```rust
//! `tau mcp ls` — list pinned MCP contracts in the current project.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tau_mcp::contract::pinned::PinnedContract;

use crate::cli::McpLsArgs;
use crate::output::Output;

#[derive(serde::Serialize)]
struct PinSummary {
    name: String,
    url: String,
    server_name: String,
    tools_count: usize,
    contract_hash_hex: String,
    path: PathBuf,
}

pub async fn run(args: McpLsArgs, output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let pin_dir = project_root.join(".tau/mcp");
    let mut pins = Vec::new();
    if pin_dir.is_dir() {
        for entry in std::fs::read_dir(&pin_dir)
            .with_context(|| format!("read {}", pin_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_suffix(".contract.json"))
                .map(String::from);
            let Some(name) = name else { continue; };
            let bytes = std::fs::read(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let pinned: PinnedContract = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", path.display()))?;
            pins.push(PinSummary {
                name,
                url: pinned.url.clone(),
                server_name: pinned.contract.server_info.name.clone(),
                tools_count: pinned.contract.tools.len(),
                contract_hash_hex: pinned.contract_hash_hex.clone(),
                path,
            });
        }
        pins.sort_by(|a, b| a.name.cmp(&b.name));
    }

    if args.json {
        let payload = serde_json::json!({ "pins": pins });
        output.println(&serde_json::to_string_pretty(&payload)?);
    } else if pins.is_empty() {
        output.println("no pinned MCP contracts (run `tau mcp pin <name>`)");
    } else {
        for p in &pins {
            output.println(&format!(
                "{:24} {:8} {} → {} ({} tools, hash {})",
                p.name,
                "MCP",
                p.url,
                p.server_name,
                p.tools_count,
                &p.contract_hash_hex[..16],
            ));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run** to confirm pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/mcp/ls.rs \
        crates/tau-cli/tests/mcp_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): mcp ls verb enumerates pinned contracts"
```

---

## Phase 3 — `show`, `refresh`, `diff`

### Task 3.1 — `show.rs` + 2 tests (human + json + sarif)

**Files:**
- Modify: `crates/tau-cli/src/cmd/mcp/show.rs`
- Modify: `crates/tau-cli/tests/mcp_dispatch.rs`

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn show_json_emits_full_contract() {
    let tmp = setup_project_with_pin();  // helper extracted from earlier tests
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "show", "weather", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"server_info\""))
        .stdout(predicates::str::contains("\"tools\""));
}

#[tokio::test]
async fn show_sarif_emits_valid_sarif_document() {
    let tmp = setup_project_with_pin();
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    let output = cmd.current_dir(tmp.path())
        .args(["mcp", "show", "weather", "--sarif"])
        .output().expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(parsed["version"], "2.1.0");
    assert_eq!(parsed["runs"][0]["tool"]["driver"]["name"], "tau-mcp");
    assert_eq!(parsed["runs"][0]["results"].as_array().unwrap().len(), 0);
}
```

(Extract `setup_project_with_pin()` into a `mod common { ... }` block at top of `mcp_dispatch.rs` to DRY the test fixture setup.)

- [ ] **Step 2: Run** to confirm fail.

- [ ] **Step 3: Implement `show.rs`**

```rust
//! `tau mcp show <name>` — show a pinned MCP contract.

use anyhow::{Context, Result};
use tau_mcp::contract::pinned::PinnedContract;

use crate::cli::McpShowArgs;
use crate::cmd::mcp::{render_sarif, OutputFormat};
use crate::output::Output;

pub async fn run(args: McpShowArgs, output: &mut Output) -> Result<()> {
    let project_root = std::env::current_dir().context("get cwd")?;
    let pin_path = project_root
        .join(".tau/mcp")
        .join(format!("{}.contract.json", &args.name));
    let bytes = std::fs::read(&pin_path)
        .with_context(|| format!("no pin file at {}", pin_path.display()))?;
    let pinned: PinnedContract = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", pin_path.display()))?;

    match OutputFormat::from_flags(args.json, args.sarif) {
        OutputFormat::Json => {
            output.println(&serde_json::to_string_pretty(&pinned)?);
        }
        OutputFormat::Sarif => {
            let payload = serde_json::to_value(&pinned)?;
            let sarif = render_sarif("tau-mcp/show", payload);
            output.println(&serde_json::to_string_pretty(&sarif)?);
        }
        OutputFormat::Human => {
            output.println(&format!("name:       {}", &args.name));
            output.println(&format!("url:        {}", &pinned.url));
            output.println(&format!(
                "server:     {} v{}",
                pinned.contract.server_info.name, pinned.contract.server_info.version
            ));
            output.println(&format!("hash:       {}", &pinned.contract_hash_hex));
            output.println(&format!("tools:      {}", pinned.contract.tools.len()));
            for t in &pinned.contract.tools {
                output.println(&format!("  - {}", t.name));
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run** to confirm pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/mcp/show.rs \
        crates/tau-cli/tests/mcp_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): mcp show verb (human/json/sarif renderers)"
```

### Task 3.2 — `refresh.rs` + 1 test

**Files:**
- Modify: `crates/tau-cli/src/cmd/mcp/refresh.rs`
- Modify: `crates/tau-cli/tests/mcp_dispatch.rs`

`refresh` is structurally `pin` again, but emits a diff against the prior pin in the human renderer. The implementer should factor the common probe-and-pin path into a private helper inside `mcp/mod.rs` (e.g. `pub(super) async fn probe_and_pin(name, override_url, project_root) -> Result<(PinnedContract, Option<PinnedContract>)>` returning new + previous), then both `pin.rs` and `refresh.rs` call it.

- [ ] **Step 1: Failing test**

```rust
#[tokio::test]
async fn refresh_overwrites_pin_file_and_reports_diff() {
    let tmp = setup_project_with_cassette_tool();  // helper for tau.toml + fixture
    // Initial pin.
    assert_cmd::Command::cargo_bin("tau").expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert().success();
    let first = std::fs::read_to_string(tmp.child(".tau/mcp/weather.contract.json").path()).unwrap();

    // Refresh against the same cassette — produces the same contract.
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "refresh", "weather", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"changed\":false"));
    let second = std::fs::read_to_string(tmp.child(".tau/mcp/weather.contract.json").path()).unwrap();
    assert_eq!(first, second);
}
```

- [ ] **Step 2: Run** to confirm fail.

- [ ] **Step 3: Implement `refresh.rs`** + the shared `probe_and_pin` helper in `mod.rs`. Implementer drives this — main shape is "load previous if any, probe via open(), build new PinnedContract, write, print diff" using `pinned.contract_hash_hex` equality for the changed/unchanged signal.

- [ ] **Step 4: Run** + **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/mcp/refresh.rs \
        crates/tau-cli/src/cmd/mcp/mod.rs \
        crates/tau-cli/src/cmd/mcp/pin.rs \
        crates/tau-cli/tests/mcp_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): mcp refresh verb + shared probe-and-pin helper"
```

### Task 3.3 — `diff.rs` + 2 tests (no-drift exits 0; drift exits 64)

**Files:**
- Modify: `crates/tau-cli/src/cmd/mcp/diff.rs`
- Modify: `crates/tau-cli/tests/mcp_dispatch.rs`

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn diff_unchanged_exits_zero() {
    let tmp = setup_project_with_cassette_tool();
    assert_cmd::Command::cargo_bin("tau").expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert().success();
    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "diff", "weather"])
        .assert()
        .success();  // exit 0
}

#[tokio::test]
async fn diff_drift_exits_64() {
    let tmp = setup_project_with_cassette_tool();
    assert_cmd::Command::cargo_bin("tau").expect("bin")
        .current_dir(tmp.path())
        .args(["mcp", "pin", "weather"])
        .assert().success();
    // Tamper with the pin file — bump the contract version field.
    let pin_path = tmp.child(".tau/mcp/weather.contract.json");
    let mut pinned: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(pin_path.path()).unwrap()).unwrap();
    pinned["contract"]["server_info"]["version"] = serde_json::json!("99.0");
    // Recompute hash to keep the pin file internally consistent; we want
    // diff to detect drift between the pin (now claiming v99.0) and the
    // live cassette (still v1.0).
    std::fs::write(pin_path.path(), serde_json::to_string(&pinned).unwrap()).unwrap();
    // Reset the hash so verify_self_hash passes — diff failure should be
    // about cassette-vs-pin, not pin-self-corruption.
    // (The implementer may simplify by skipping verify_self_hash in diff
    // and only comparing the actual contract bodies.)

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("bin");
    cmd.current_dir(tmp.path())
        .args(["mcp", "diff", "weather"])
        .assert()
        .code(64);
}
```

- [ ] **Step 2: Run** to confirm fail.

- [ ] **Step 3: Implement `diff.rs`** — load the pin, probe live, compare the two `contract_hash_hex` values. Equal → exit 0. Unequal → print field-level diff (use a simple "tool count old/new, server version old/new, hash old/new" line; fancy JSON-diff is YAGNI) and exit 64 via `std::process::exit(64)`.

- [ ] **Step 4: Run** + **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/mcp/diff.rs \
        crates/tau-cli/tests/mcp_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): mcp diff verb (exit 0/64 on contract drift)"
```

---

## Phase 4 — `tau check mcp_contracts` phase

**Goal:** New aggregator phase that walks `Tau.lock`'s `mcp_entries` (lockfile v7, shipped PR-4) and verifies every entry with a `pinned_contract: Some(<path>)` has a pin file that hashes to the recorded `contract_hash`. Mismatch → typed `CheckFinding` with both hashes + path.

### Task 4.1 — Add `CheckCategory::McpContracts` + dispatch + 2 tests

**Files:**
- Modify: `crates/tau-cli/src/cmd/check/result.rs` (add variant)
- Modify: `crates/tau-cli/src/cmd/check/categories/mod.rs` (declare module)
- Create: `crates/tau-cli/src/cmd/check/categories/mcp_contracts.rs`
- Modify: `crates/tau-cli/src/cmd/check/runner.rs` (route)
- Modify: `crates/tau-cli/src/cmd/check/mod.rs` (add to "all categories" listing)
- Modify: an existing check integration test file (or create `crates/tau-cli/tests/check_mcp.rs`)

- [ ] **Step 1: Read the established phase pattern** — `crates/tau-cli/src/cmd/check/categories/packages.rs` is a good template. It takes a `&CheckCtx`, returns `Vec<CheckFinding>`, uses `Severity` + `FindingLocation`.

- [ ] **Step 2: Add the enum variant** — READ `crates/tau-cli/src/cmd/check/result.rs` for the `CheckCategory` enum + add `McpContracts` next to the existing variants (Config, Lockfile, Packages, Sandbox, Plugins, Skills). Update any `match` exhaustiveness elsewhere — let the compiler tell you where.

- [ ] **Step 3: Failing integration test** in `crates/tau-cli/tests/check_mcp.rs`:

```rust
//! Integration test: tau check mcp_contracts.

use assert_fs::prelude::*;

#[test]
fn check_mcp_contracts_passes_when_pin_matches_lockfile() {
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_consistent_project(&tmp);  // helper: tau.toml + Tau.lock + pin file all aligned
    assert_cmd::Command::cargo_bin("tau").unwrap()
        .current_dir(tmp.path())
        .args(["check", "mcp-contracts"])
        .assert()
        .success();
}

#[test]
fn check_mcp_contracts_fails_when_pin_drifted() {
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_consistent_project(&tmp);
    // Tamper with the pin file (modify contract, keep hash) so pin's
    // self-hash diverges from lockfile's contract_hash.
    tamper_pin_contract(&tmp);
    assert_cmd::Command::cargo_bin("tau").unwrap()
        .current_dir(tmp.path())
        .args(["check", "mcp-contracts"])
        .assert()
        .code(2);  // CheckStatus::Fixable per result.rs convention
}
```

The `setup_consistent_project` helper must build a v7 lockfile with at least one `LockedMcpEntry` with a `pinned_contract: Some("./.tau/mcp/weather.contract.json")` field. READ `crates/tau-pkg/src/lockfile/mod.rs` to find the writer API.

- [ ] **Step 4: Run** to confirm fail.

- [ ] **Step 5: Implement `mcp_contracts.rs`**

```rust
//! `tau check mcp_contracts` — verify pinned MCP contracts match the
//! lockfile.
//!
//! Walks `Tau.lock`'s `mcp` entries; for each entry with
//! `pinned_contract: Some(path)`, reads the pin via
//! `serde_json::from_slice::<PinnedContract>` and compares
//! `pinned.decoded_hash()` to `entry.contract_hash`. Mismatch surfaces
//! as a Fixable `CheckFinding` with both hashes + the file path.

use std::path::PathBuf;

use tau_mcp::contract::pinned::PinnedContract;

use crate::cmd::check::result::{CheckCategory, CheckFinding, FindingLocation, Severity};
use crate::cmd::check::runner::CheckCtx;

pub async fn run(ctx: &CheckCtx) -> Vec<CheckFinding> {
    let Some(lockfile) = ctx.lockfile.as_ref() else {
        return vec![]; // no lockfile = handled by the `lockfile` category
    };
    let Some(mcp_entries) = lockfile_mcp_entries(lockfile) else {
        return vec![]; // older lockfile schema = nothing to check here
    };

    let mut findings = Vec::new();
    for entry in mcp_entries {
        let Some(pin_rel) = entry_pinned_contract(entry) else { continue; };
        let pin_path = ctx.project_root.join(&pin_rel);
        let bytes = match std::fs::read(&pin_path) {
            Ok(b) => b,
            Err(e) => {
                findings.push(CheckFinding {
                    category: CheckCategory::McpContracts,
                    severity: Severity::Error,
                    message: format!("missing pin file `{}`: {e}", pin_path.display()),
                    location: Some(FindingLocation::File { path: pin_path.clone() }),
                    code: Some("mcp.contract.missing".into()),
                });
                continue;
            }
        };
        let pinned: PinnedContract = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                findings.push(CheckFinding {
                    category: CheckCategory::McpContracts,
                    severity: Severity::Error,
                    message: format!("malformed pin file `{}`: {e}", pin_path.display()),
                    location: Some(FindingLocation::File { path: pin_path.clone() }),
                    code: Some("mcp.contract.malformed".into()),
                });
                continue;
            }
        };
        // First: pin self-integrity.
        if let Err(e) = pinned.verify_self_hash() {
            findings.push(CheckFinding {
                category: CheckCategory::McpContracts,
                severity: Severity::Error,
                message: format!("pin self-hash drift for `{}`: {e}", entry_name(entry)),
                location: Some(FindingLocation::File { path: pin_path.clone() }),
                code: Some("mcp.contract.self_drift".into()),
            });
            continue;
        }
        // Second: pin vs lockfile.
        if pinned.contract_hash_hex != entry_hash_hex(entry) {
            findings.push(CheckFinding {
                category: CheckCategory::McpContracts,
                severity: Severity::Error,
                message: format!(
                    "pin hash drift for `{}`: lockfile says `{}`, pin file says `{}` at {}",
                    entry_name(entry),
                    entry_hash_hex(entry),
                    pinned.contract_hash_hex,
                    pin_path.display(),
                ),
                location: Some(FindingLocation::File { path: pin_path.clone() }),
                code: Some("mcp.contract.lockfile_drift".into()),
            });
        }
    }
    findings
}

// Implementer: READ crates/tau-pkg/src/lockfile/* to determine the
// actual accessor names. The three helpers below are placeholders that
// must adapt to the as-shipped API.
fn lockfile_mcp_entries<'a>(_lockfile: &'a tau_pkg::lockfile::Lockfile) -> Option<&'a [tau_pkg::lockfile::LockedMcpEntry]> {
    todo!("READ tau_pkg::lockfile for the mcp-entries accessor")
}
fn entry_pinned_contract(_entry: &tau_pkg::lockfile::LockedMcpEntry) -> Option<PathBuf> {
    todo!("READ tau_pkg::lockfile::LockedMcpEntry for the pinned_contract field")
}
fn entry_name(_entry: &tau_pkg::lockfile::LockedMcpEntry) -> &str {
    todo!("READ tau_pkg::lockfile::LockedMcpEntry for the name field")
}
fn entry_hash_hex(_entry: &tau_pkg::lockfile::LockedMcpEntry) -> &str {
    todo!("READ tau_pkg::lockfile::LockedMcpEntry for the contract_hash field")
}
```

The `todo!()` blocks at the bottom MUST be filled in by reading the actual lockfile types shipped in PR-4. The implementer should remove the placeholders entirely once they've inlined the field accesses, since these are trivial getters.

- [ ] **Step 6: Wire `runner.rs`** — add `CheckCategory::McpContracts => mcp_contracts::run(ctx).await` to the dispatch match.

- [ ] **Step 7: Add to `mod.rs`'s `all_categories()`** list (so bare `tau check` runs it).

- [ ] **Step 8: Run** the integration tests to confirm pass.

- [ ] **Step 9: Commit**

```bash
git add crates/tau-cli/src/cmd/check/ \
        crates/tau-cli/tests/check_mcp.rs
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "feat(tau-cli): tau check mcp_contracts phase"
```

---

## Phase 5 — Conformance fixture #07 (cassette-replay weather)

**Goal:** A workflow that calls a cassette-backed MCP server's `get_forecast` tool and produces a deterministic IR run report. Executes under both DevMode and BundleMode via the β.2 conformance harness.

### Task 5.1 — Build fixture #07 directory + 2 cross-mode tests

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/07_mcp_weather/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/07_mcp_weather/weather_cassette.jsonl`
- Create: `crates/tau-ir-conformance/fixtures/07_mcp_weather/.tau/mcp/weather.contract.json`
- Create: `crates/tau-ir-conformance/fixtures/07_mcp_weather/expected_report.json`
- Modify: `crates/tau-ir-conformance/tests/cross_mode.rs` (or whatever file enumerates fixtures) — add `07_mcp_weather`.

- [ ] **Step 1: Read fixture 06** — `crates/tau-ir-conformance/fixtures/06_multi_turn_history/` for the layout convention (workflow.toml + mock_llm.jsonl + expected_report.json) and `crates/tau-ir-conformance/tests/*.rs` for the test enumeration pattern.

- [ ] **Step 2: Read PR-3's cassette format spec** — `crates/tau-mcp/src/cassette/` modules for the JSONL schema (each line is a CassetteMessage with direction + payload + optional metadata).

- [ ] **Step 3: Write `workflow.toml`** — minimal workflow with one agent calling one MCP tool:

```toml
[project]
name = "weather-demo"
version = "0.0.1"

[tools.weather]
mcp = "cassette:./weather_cassette.jsonl"

[agents.forecaster.prompt]
system = "You are a forecaster."

[agents.forecaster.tools]
weather = "*"

[workflows.main]
steps = [
  { agent = "forecaster", prompt = "What is the forecast for Paris?" }
]
```

(Exact agent/workflow TOML shape comes from existing fixtures — adapt as needed. The implementer reads fixture 06 for the right key names.)

- [ ] **Step 4: Write `weather_cassette.jsonl`** — handshake + one `tools/call` response. ~6-8 JSONL lines. Implementer copies the cassette skeleton from `crates/tau-mcp/tests/golden/` (PR-3's test cassettes) and edits for a weather tool with name `get_forecast`, input schema, and a canned output `{"forecast": "sunny, 22°C"}`.

- [ ] **Step 5: Write the pin file `.tau/mcp/weather.contract.json`** that matches the cassette's `initialize` response. Easiest path: leave it empty initially; the conformance test's first run regenerates it via `PinnedContract::from_parts(...)`, then commit the generated file. Implementer writes a one-shot script if needed (or just runs `tau mcp pin weather` inside the fixture dir and copies the result).

- [ ] **Step 6: Write `expected_report.json`** — same shape as fixture 06's expected_report.json. The IR run report should contain the agent's emitted message + the MCP tool call + the cassette's tool response. Implementer captures actual output from a first run and pins it as the expected.

- [ ] **Step 7: Add fixture to the test enumerator** — READ `crates/tau-ir-conformance/tests/*.rs` to find the spot that iterates `fixtures/*` and add `"07_mcp_weather"` to the list (or, if iteration is automatic via fs read_dir, no edit needed).

- [ ] **Step 8: Add a handler in `conformance.rs` (if needed)** — the existing β.2 harness should handle MCP tool calls if they go through the regular IR dispatch path. If fixture 07 fails because no cassette dial is wired into the conformance test runner, ADD an arm in the harness that initializes any `cassette:` URLs upfront using the new `host_lifecycle::open()` path.

- [ ] **Step 9: Run** the conformance tests:

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-ir-conformance
```
Expected: 07_mcp_weather passes in both DevMode and BundleMode.

- [ ] **Step 10: Commit**

```bash
git add crates/tau-ir-conformance/fixtures/07_mcp_weather/ \
        crates/tau-ir-conformance/tests/
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "test(tau-ir-conformance): fixture 07 cassette-replay weather (DevMode+BundleMode)"
```

---

## Phase 6 — ADR-0038 finalize + 2 mdBook pages + SUMMARY.md

**Goal:** Replace the placeholder ADR with the as-shipped reality; add the two Diátaxis pages; list both in SUMMARY.md.

### Task 6.1 — Finalize ADR-0038

**File:** `docs/decisions/ADR-0038-mcp-facilitator.md` (OVERWRITE)

- [ ] **Step 1: Read the placeholder** to know what's there. Then OVERWRITE with a finalized ADR using this skeleton:

```markdown
# ADR-0038: MCP Facilitator (β.3)

**Status:** Accepted
**Date:** 2026-06-10
**Supersedes:** (none — finalizes ADR-0038 placeholder from PR-1)

## Context

[2-3 paragraphs: tau's plugin protocol predated MCP; the philosophy
pivot 2026-05-29 named MCP as the canonical multi-vendor tool contract;
β.3 introduces an MCP-client facilitator inside tau-runtime-tokio so
existing IR programs can reference MCP servers via `[tools.<name>]
mcp = "..."` without changing the IR.]

## Decision

Adopt MCP as a first-class tool contract on equal footing with the
existing bespoke plugin protocol. Specifically:

1. **Transport surface (3 schemes, 4 dial code paths):**
   - `stdio:<argv>` — subprocess MCP server (PR-2)
   - `http://...` / `https://...` — Streamable HTTP MCP server (PR-3)
   - `cassette:<path>` — recorded MCP traffic replayed from JSONL (PR-3 + PR-6)

2. **Pinned contracts:** Each referenced server has its `ServerContract`
   captured at install time as `.tau/mcp/<name>.contract.json` (schema
   v1 — `PinnedContract` struct in `tau-mcp::contract::pinned`).
   `contract_hash_hex` is the canonical hash of the contract body.

3. **Lockfile v7:** Adds `mcp_entries: Vec<LockedMcpEntry>` capturing
   `name`, `url`, `contract_hash`, optional `pinned_contract: Option<PathBuf>`.

4. **CLI surface:** `tau mcp {pin, ls, show, refresh, diff}` (PR-6) for
   inspection + provenance management. `tau check mcp_contracts` (PR-6)
   gates that pinned files match the lockfile.

5. **Conformance:** Fixture #07 exercises a cassette-replay weather scenario
   under both DevMode and BundleMode (PR-6) — locks the runtime → handshake
   → tool-call pipeline.

6. **Drift defence-in-depth:** Three independent checks must agree —
   `PinnedContract::verify_self_hash` (pin internal integrity),
   `tau check mcp_contracts` (pin vs lockfile), runtime drift check
   (live server vs pin). Any mismatch fails closed.

## Consequences

Positive:
- Users can adopt arbitrary MCP servers without bespoke plugin code.
- Cassettes make MCP tools fully testable + reproducible without a
  network or subprocess at test time.
- The plugin protocol is now slated for replacement-by-MCP per the
  philosophy pivot 2026-05-29 — β.3 is the foundation for that work.

Negative:
- Three drift checks must be kept in lockstep across `tau-mcp`,
  `tau-pkg`, and `tau-cli`. The conformance fixture is the canary.
- Each pinned contract is committed to the repo (small JSON files,
  ~5-50KB each).

## Alternatives considered

- **Streamable HTTP only, no stdio:** rejected — most MCP servers in the
  ecosystem (filesystem, github, etc.) ship as stdio-only.
- **No cassettes, mock MCP servers per test:** rejected — duplicates
  test infrastructure across crates and gives weaker guarantees about
  protocol compliance than replaying real traffic.
- **Inline contracts in lockfile (no separate .tau/mcp/ files):**
  rejected — contracts can be 50KB+ each; would bloat the lockfile.

## References

- PR #281 (β.3 PR-1 — scaffolds)
- PR #282 (β.3 PR-2 — stdio transport + host lifecycle)
- PR #283 (β.3 PR-3 — HTTP transport + cassette)  [verify PR # from git log]
- PR #284 (β.3 PR-4 — lowering + lockfile v7 + tau build wiring)
- PR #285 (β.3 PR-5 — bridge wiring + dev/bundle dispatch)
- PR #<this PR> (β.3 PR-6 — CLI + conformance + this ADR finalize)
- Spec: `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md`
- Philosophy pivot: `docs/explanation/tau-philosophy.md`
```

(Implementer fills in the PR numbers from `git log --oneline main..HEAD` and the placeholder bracketed phrases.)

- [ ] **Step 2: Commit** (combined with Task 6.2/6.3 at end of phase).

### Task 6.2 — `docs/how-to/mcp-servers.md`

**File:** `docs/how-to/mcp-servers.md` (CREATE)

- [ ] **Step 1: Write the how-to** following Diátaxis conventions (task-oriented, step-by-step, references existing how-tos like `docs/how-to/quarantine-flaky-tests.md` for tone). Cover:
  1. Add a server to `tau.toml`:
     ```toml
     [tools.weather]
     mcp = "stdio:npx --yes @some/weather-mcp"
     ```
  2. Pin its contract: `tau mcp pin weather`.
  3. Use the tool in an agent: `[agents.X.tools] weather = "*"`.
  4. Refresh when the server changes: `tau mcp refresh weather`.
  5. Detect drift in CI: `tau check mcp_contracts` (or `tau check`).
  6. Local-only cassette workflow:
     ```toml
     [tools.weather]
     mcp = "cassette:./fixtures/weather.jsonl"
     ```
  7. Link to the reference page for full flag docs.

### Task 6.3 — `docs/reference/tau-mcp.md`

**File:** `docs/reference/tau-mcp.md` (CREATE)

- [ ] **Step 1: Write the reference** — Diátaxis "reference" tone (precise, exhaustive). Cover every verb with synopsis + flags + exit codes + JSON output schema:
  - `tau mcp pin <name> [--from URL] [--json]`
  - `tau mcp ls [--json]`
  - `tau mcp show <name> [--json | --sarif]`
  - `tau mcp refresh <name> [--json]`
  - `tau mcp diff <name> [--json]` (exit 0 unchanged, 64 drift)
  - The pinned contract file format (point at `PinnedContract` rustdoc).
  - The lockfile v7 `mcp_entries` shape (point at `LockedMcpEntry` rustdoc).
  - The `tau check mcp_contracts` phase (output codes).

### Task 6.4 — Add both pages to `docs/SUMMARY.md`

**File:** `docs/SUMMARY.md` (MODIFY)

- [ ] **Step 1: Read** the current SUMMARY.md to find the right sections (how-to + reference subtrees).

- [ ] **Step 2: Add lines** (exact path syntax depends on the file — match the surrounding entries):

```markdown
    - [Add an MCP server](how-to/mcp-servers.md)
...
    - [tau mcp](reference/tau-mcp.md)
```

- [ ] **Step 3: Build the book locally** (per CLAUDE.md DOCS RULES):

```
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build
```
Expected: only `[INFO]` lines + a `docs/book/` tree. Linkcheck clean.

- [ ] **Step 4: Clean the build output**

```
rm -rf docs/book
```

- [ ] **Step 5: Commit all of Phase 6 together**

```bash
git add docs/decisions/ADR-0038-mcp-facilitator.md \
        docs/how-to/mcp-servers.md \
        docs/reference/tau-mcp.md \
        docs/SUMMARY.md
git -c user.name="Test User" -c user.email="test@example.com" \
  commit --no-verify -m "docs(adr): finalize ADR-0038; add MCP how-to + reference pages"
```

---

## Phase 7 — Workspace checks + push + PR + auto-merge

### Task 7.1 — Local validation across all touched crates

- [ ] **Step 1: fmt**

```
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo fmt --all -- --check
```

- [ ] **Step 2: clippy across the 3 touched crates**

```
for c in tau-mcp-tokio tau-cli tau-ir-conformance; do
  timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
    cargo clippy -p "$c" --all-targets -- -D warnings || exit 1
done
```

- [ ] **Step 3: nextest**

```
for c in tau-mcp-tokio tau-cli tau-ir-conformance; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
    cargo nextest run -p "$c" || exit 1
done
```

- [ ] **Step 4: doctests**

```
for c in tau-mcp-tokio tau-cli tau-ir-conformance; do
  timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
    cargo test -p "$c" --doc || exit 1
done
```

- [ ] **Step 5: Canary downstream check** — `tau-mcp-tokio` is consumed by `tau-cli`, `tau-runtime-tokio`, `tau-app`. The first two are covered; spot-check `tau-app`:

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo check -p tau-app
```

### Task 7.2 — Push + PR + auto-merge

- [ ] **Step 1: Push**

```
git push --no-verify -u origin feat/beta-3-pr-6-mcp-cli
```

- [ ] **Step 2: Open PR**

```bash
gh pr create --title "β.3 PR-6 — MCP CLI verbs + conformance #07 + ADR-0038 finalize + docs" \
  --body "$(cat <<'EOF'
## Summary

Closes the β.3 MCP facilitator sub-project.

- 5 new `tau mcp` verbs: pin / ls / show / refresh / diff
- New `cassette:<path>` URL scheme (`McpUrl::Cassette` variant + `cassette_dial::dial`)
- `tau check mcp_contracts` aggregator phase (verifies pinned contracts vs lockfile)
- Conformance fixture #07 (cassette-replay weather, DevMode + BundleMode)
- ADR-0038 finalized (was a placeholder shipped in PR-1)
- 2 mdBook pages: `how-to/mcp-servers.md` + `reference/tau-mcp.md`

Spec: `docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md` §10, §11, §12, §15.
Plan: `docs/superpowers/plans/2026-06-10-beta-3-mcp-facilitator-pr-6.md`.

## Test plan

- [ ] tau-mcp-tokio: 12 url tests + 2 cassette_dial integration tests
- [ ] tau-cli: ~12 mcp_dispatch tests (pin/ls/show/refresh/diff × shapes)
- [ ] tau-cli: 2 check_mcp tests (pass + drift)
- [ ] tau-ir-conformance: fixture 07 in DevMode + BundleMode (2 cases)
- [ ] mdbook build clean (linkcheck error-on-warn)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Capture PR number** + enrol auto-merge

```
PR=$(gh pr view --json number --jq .number)
gh pr merge "$PR" --auto
```

- [ ] **Step 4: Wait for ci-summary** — monitor via:

```
gh pr view "$PR" --json statusCheckRollup --jq '.statusCheckRollup[] | select(.name=="ci-summary")'
```

If macOS infra flake hits (`chat_ephemeral_writes_no_file`, `echo-tool` race, `child_crash_mid_call_surfaces_transport_error`): `gh run rerun <id> --failed` + `gh pr merge $PR --auto` to re-enrol.

---

## Self-review checklist

Run through this list once after the plan is written. Fix issues inline.

- [ ] **Phase 1 — McpUrl::Cassette** added with `PathBuf`, parses `cassette:<path>`, rejects empty path; new `EmptyCassettePath` error variant.
- [ ] **Phase 1 — cassette_dial::dial** reads file, builds `CassetteTransport`, returns transport handle; `LifecycleError::Io` + `LifecycleError::CassetteParse` variants added.
- [ ] **Phase 1 — open() arm** dispatches `McpUrl::Cassette { path }` to `open_cassette` which drives handshake.
- [ ] **Phase 2 — cmd/mcp scaffold** mirrors `cmd/skill/`; dispatch from `main.rs`; 5 verb stub modules link.
- [ ] **Phase 2 — pin.rs** loads project, resolves URL (`--from` wins), opens transport, builds `PinnedContract::from_parts`, writes to `.tau/mcp/<name>.contract.json`.
- [ ] **Phase 2 — ls.rs** enumerates `.tau/mcp/*.contract.json`, parses each, sorts by name, renders human + json.
- [ ] **Phase 3 — show.rs** uses `OutputFormat::from_flags` to pick human / json / sarif; SARIF goes through `render_sarif("tau-mcp/show", payload)`.
- [ ] **Phase 3 — refresh.rs** factors out shared probe-and-pin helper; reports `changed: bool` via hash comparison.
- [ ] **Phase 3 — diff.rs** read-only; exits 0 on match, 64 on drift; emits hash-level diff line.
- [ ] **Phase 4 — check mcp_contracts** walks lockfile mcp entries, verifies each pin's self-hash + lockfile hash; surfaces typed `CheckFinding` with code `mcp.contract.{missing,malformed,self_drift,lockfile_drift}`.
- [ ] **Phase 4 — wired** into `CheckCategory` enum, runner dispatch, `all_categories()` listing.
- [ ] **Phase 5 — fixture #07** has workflow.toml + weather_cassette.jsonl + pinned contract + expected_report.json + is enumerated in tests.
- [ ] **Phase 5 — conformance runner** handles `cassette:` URL (either auto via the existing IR dispatch or via an explicit arm in the harness).
- [ ] **Phase 6 — ADR-0038** lists 3 transports, 4 dial paths, lockfile v7, CLI surface, conformance #07, and the 3-tier drift defence.
- [ ] **Phase 6 — both mdBook pages** added to SUMMARY.md; mdbook build clean locally; docs/book/ cleaned before commit.
- [ ] **Phase 7 — fmt + clippy + nextest + doctests** green on tau-mcp-tokio, tau-cli, tau-ir-conformance; tau-app `cargo check` clean as canary.
- [ ] **No `features = ["test-support"]` added** to any `tau-runtime-tokio` dev-dep.
- [ ] **No `Option::map_or(false, ...)`** introduced — used `is_some_and` instead.
- [ ] **No `[[profile.ci.overrides]]`** added to `.config/nextest.toml`.
- [ ] **Auto-merge enrolled bare** — `gh pr merge $PR --auto` (no `--squash`, no `--delete-branch`, no `--admin`).

---

## What's next (post-β.3)

PR-6 closes the β.3 sub-project. Immediate follow-ups (β.3.1 / out of scope here):

1. **OAuth for HTTPS MCP servers** — current dial path is anonymous; commercial MCP servers (GitHub, Slack) need OAuth flows.
2. **Per-spawn `DenyEntry` threading** through MCP tool calls — currently deny rules apply at the runtime level, not per call.
3. **Live trace render for MCP tool calls** in `tau workflow log` — currently rolled up into the agent span.
4. **`tau mcp pin` against a deny-only project** — pin should warn if the contract surface exposes capabilities the project plan denies.
5. **Bespoke plugin protocol → MCP migration** — the philosophy pivot 2026-05-29 names this; tracked separately as β.4-ish.

Roadmap-wise, β.3 closing frees the β.4 slot. Next priority per `ROADMAP.md` is the resolver + DX work (γ / δ tracks).
