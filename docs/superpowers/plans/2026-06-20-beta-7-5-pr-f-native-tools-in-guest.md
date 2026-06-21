# β.7.5 PR-F — Shared native tools + simplified fan-monitor in-guest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Depends on PR-E2 having landed** (the guest must already drive `run_ir_streaming` over a baked IR — see `2026-06-20-beta-7-5-pr-e2-guest-ir-driving.md`).

**Goal:** Extract the deterministic native tools (`read_temp`, `set_fan`) into a shared `no_std` `tau-native-tools` crate consumed by *both* the dev conformance profile and the wasm guest, then prove the **simplified fan-monitor** (one agent + `read_temp` + `set_fan` + cassette LLM) runs end-to-end inside the wasm component — with its tool bodies produced by the same code that produces them in dev (structural parity). Record the in-wasm MCP-facilitator decision as ADR-0053.

**Architecture:** Today the native tools are hard-coded match arms inside `crates/tau-conformance/src/dispatcher.rs` (a std crate). PR-F lifts that logic into a tiny `no_std` library whose `invoke(tool_id, args) -> Option<Value>` both dispatchers call, so the bytes a tool returns can never drift between profiles — the property PR-G's `dev == wasm` gate depends on. The guest's `GuestDispatcher` (built in PR-E2) routes tool calls to this library instead of erroring. The full fan-monitor's MCP `weather` tool and the β.4 context pipeline are **out of scope** (fixture 07 + 13 are PR-G; β.4 is unshipped) — PR-F ships the *simplified* scenario the design's DoD names.

**Tech Stack:** Rust, `no_std` + `alloc`, `serde_json` (no_std/alloc), `wasm32-wasip2`, `wasmtime` (host test), `wit-bindgen`.

## Global Constraints

- **CARGO discipline (CLAUDE.md):** `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Never bare `cargo`, never `--workspace`. `cargo nextest run` for tests. Check `pgrep -af cargo` first.
- **Parity is byte-exact:** `tau-native-tools::invoke` is the *single* source of `read_temp`/`set_fan` bodies. After Task 2, the existing conformance golden must remain **byte-identical** (the dev profile now calls the shared fn but returns the same `json!(32)` / `json!({"ok": true})`).
- **no_std:** `tau-native-tools` is `#![no_std]` + `extern crate alloc`; it is a normal library (no `#[panic_handler]`, no cdylib), so it **is** host-unit-testable (std under `cfg(test)`). The guest crate remains wasm32-only and is exercised only through `tau-wasm-host` integration tests.
- **initial_messages parity:** the guest passes `Vec::new()` to `run_ir_streaming`, matching `crates/tau-conformance/src/profile/dev.rs`. Do **not** thread the WIT `prompt` into a `Message` — that would diverge the wasm stream from dev and break PR-G's gate. (User-facing prompt threading is a γ concern.)
- **ADR number:** the in-wasm MCP-facilitator ADR is **0053** (0050/0051/0052 are taken). The β.7.5 design's "0050" reference is stale.
- **Invariant:** every existing test stays green — all 13 conformance fixtures (dev+bundle), the fan-monitor golden, the PR-E2 roundtrip tests, plugins, sandbox, bundle, Skills, workflow.
- **Commits:** Conventional Commits, `feat(β.7.5): …`. Commit per task.

---

## File Structure

- `crates/tau-native-tools/Cargo.toml` — **new** no_std lib crate.
- `crates/tau-native-tools/src/lib.rs` — **new**; `invoke(tool_id, args) -> Option<Value>` + unit tests.
- `Cargo.toml` (root) — **modify**; add the crate to `[workspace] members` and `[workspace.dependencies]`.
- `crates/tau-conformance/Cargo.toml` — **modify**; depend on `tau-native-tools`.
- `crates/tau-conformance/src/dispatcher.rs` — **modify**; native arms call `tau_native_tools::invoke`.
- `crates/tau-wasm-guest/Cargo.toml` — **modify**; wasm-only dep on `tau-native-tools`.
- `crates/tau-wasm-guest/src/dispatcher.rs` — **modify**; `invoke` routes to `tau_native_tools::invoke`.
- `crates/tau-conformance/fixtures/fan_monitor_simple/{tau.toml,mock_llm.jsonl}` — **new** simplified fixture (also reused by PR-G's dev run).
- `crates/tau-wasm-host/tests/fan_monitor_simple.rs` — **new** integration test.
- `docs/decisions/0053-in-wasm-mcp-facilitator.md` — **new** ADR.
- `docs/SUMMARY.md` — **modify**; list ADR-0053.

---

## Task 1: `tau-native-tools` crate (shared, no_std, unit-tested)

**Files:**
- Create: `crates/tau-native-tools/Cargo.toml`
- Create: `crates/tau-native-tools/src/lib.rs`
- Modify: `Cargo.toml` (root — members + workspace.dependencies)

**Interfaces:**
- Produces: `pub fn invoke(tool_id: &str, args: &serde_json::Value) -> Option<serde_json::Value>` — `Some(body)` for a known native tool, `None` for anything else. `read_temp` → `Value` integer `32`; `set_fan` → `{"ok": true}`. The `args` parameter is accepted for forward-compat (set_fan ignores it in v0, matching the current dispatcher).

- [ ] **Step 1: Register the crate in the workspace**

In the root `Cargo.toml`, add `"crates/tau-native-tools",` to `[workspace] members` (alphabetically near the other `tau-*` entries), and add to `[workspace.dependencies]`:

```toml
tau-native-tools = { path = "crates/tau-native-tools", version = "0.0.0", default-features = false }
```

- [ ] **Step 2: Write the crate manifest**

Create `crates/tau-native-tools/Cargo.toml`:

```toml
[package]
name = "tau-native-tools"
description = "β.7.5 shared deterministic native tools (read_temp/set_fan), no_std; one source of tool bodies for both the dev conformance profile and the wasm guest."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[dependencies]
serde_json = { workspace = true, default-features = false, features = ["alloc"] }
```

> Mirror `crates/tau-ir/Cargo.toml`'s `serde_json` line exactly — it is the proven no_std/alloc configuration in this workspace.

- [ ] **Step 3: Write the failing unit test + implementation**

Create `crates/tau-native-tools/src/lib.rs`:

```rust
//! Deterministic native tools shared by the dev conformance profile and the
//! wasm guest (β.7.5 PR-F). One source of truth for each tool's body so the
//! bytes never drift between execution profiles — the property PR-G's
//! `dev == wasm` conformance gate depends on.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(test)]
extern crate std;

use serde_json::{json, Value};

/// Invoke a native tool by its IR `ToolId` string.
///
/// Returns `Some(body)` for a known tool, `None` otherwise (the caller turns
/// `None` into its own "unknown tool" error). Bodies are deterministic and
/// independent of `args` in v0 — exactly the behaviour the conformance
/// fan-monitor relies on.
pub fn invoke(tool_id: &str, _args: &Value) -> Option<Value> {
    match tool_id {
        "read_temp" => Some(json!(32)),
        "set_fan" => Some(json!({ "ok": true })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_temp_returns_32() {
        assert_eq!(invoke("read_temp", &json!({})), Some(json!(32)));
    }

    #[test]
    fn set_fan_returns_ok_true_ignoring_args() {
        assert_eq!(invoke("set_fan", &json!({ "on": true })), Some(json!({ "ok": true })));
        assert_eq!(invoke("set_fan", &json!({})), Some(json!({ "ok": true })));
    }

    #[test]
    fn unknown_tool_is_none() {
        assert_eq!(invoke("weather", &json!({})), None);
        assert_eq!(invoke("nope", &json!({})), None);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-native-tools 2>&1 | tail -15`
Expected: 3 tests PASS.

- [ ] **Step 5: Confirm it builds for wasm (no std creep)**

Run: `timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-native-tools --target wasm32-wasip2 2>&1 | tail -10`
Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-native-tools Cargo.toml
git commit -m "feat(β.7.5): tau-native-tools — shared no_std read_temp/set_fan (PR-F)"
```

---

## Task 2: Route the dev profile through `tau-native-tools` (golden unchanged)

**Files:**
- Modify: `crates/tau-conformance/Cargo.toml` (add dep)
- Modify: `crates/tau-conformance/src/dispatcher.rs` (native arms → shared fn)

**Interfaces:**
- Consumes: `tau_native_tools::invoke`.

- [ ] **Step 1: Add the dependency**

In `crates/tau-conformance/Cargo.toml` `[dependencies]` add:

```toml
tau-native-tools = { workspace = true }
```

> `tau-conformance` is a std crate; the default-featureless workspace alias is fine (the crate has no features).

- [ ] **Step 2: Replace the native match arms**

In `crates/tau-conformance/src/dispatcher.rs`, inside `ConformanceDispatcher::invoke`'s `match name.as_str()`, replace the `"read_temp"` and `"set_fan"` arms with a shared lookup, keeping the `weather` and unknown arms unchanged:

```rust
        Box::pin(async move {
            // Native tools come from the shared no_std crate so dev and the
            // wasm guest return byte-identical bodies (PR-F).
            if let Some(body) = tau_native_tools::invoke(&name, &args_owned) {
                return Ok(ToolInvocationResult {
                    body: Some(body),
                    error: None,
                });
            }
            match name.as_str() {
                "weather" => {
                    let client = weather.ok_or_else(|| RuntimeError::Internal {
                        message: "weather invoked but no MCP client wired".to_string(),
                    })?;
                    let resp = client.call_tool("weather", args_owned).await.map_err(|e| {
                        RuntimeError::Internal {
                            message: format!("MCP weather call_tool failed: {e}"),
                        }
                    })?;
                    Ok(ToolInvocationResult {
                        body: Some(mcp_response_to_json(&resp)),
                        error: None,
                    })
                }
                other => Err(RuntimeError::Internal {
                    message: format!("unknown conformance tool {other:?}"),
                }),
            }
        })
```

> Keep the `let name = tool_id.0.clone();` / `let args_owned = args.clone();` / `let weather = self.weather.clone();` bindings above the `Box::pin` unchanged.

- [ ] **Step 3: Run the conformance golden (must stay byte-identical)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance 2>&1 | tail -20`
Expected: PASS — especially `fan_monitor_dev_matches_golden` and `dev_profile_is_deterministic`. **Do not re-bless the golden** — the bytes must not change. If the golden diffs, the refactor changed behavior; fix the code, not the golden.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-conformance/Cargo.toml crates/tau-conformance/src/dispatcher.rs
git commit -m "refactor(β.7.5): dev profile sources native tools from tau-native-tools (PR-F)"
```

---

## Task 3: Route the guest dispatcher through `tau-native-tools`

**Files:**
- Modify: `crates/tau-wasm-guest/Cargo.toml` (wasm-only dep)
- Modify: `crates/tau-wasm-guest/src/dispatcher.rs` (invoke routes to shared fn)

**Interfaces:**
- Consumes: `tau_native_tools::invoke`. `GuestDispatcher` (from PR-E2) gains tool routing; the error fallback is preserved for unknown tools.

**Testability note:** the guest crate is wasm32-only; this task's behavior is verified by Task 4's `tau-wasm-host` integration test.

- [ ] **Step 1: Add the wasm-only dependency**

In `crates/tau-wasm-guest/Cargo.toml`, under `[target.'cfg(target_arch = "wasm32")'.dependencies]`, add:

```toml
tau-native-tools = { path = "../tau-native-tools", default-features = false }
```

> Use the direct path dep with `default-features = false` (the guest's established no_std pattern — see the `tau-runtime-core` line comment in that manifest).

- [ ] **Step 2: Route `invoke` to the shared tools**

In `crates/tau-wasm-guest/src/dispatcher.rs`, replace the body of `GuestDispatcher::invoke` (the version PR-E2 left erroring for every tool):

```rust
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let name = tool_id.0.clone();
        let args_owned = args.clone();
        Box::pin(async move {
            match tau_native_tools::invoke(&name, &args_owned) {
                Some(body) => Ok(ToolInvocationResult {
                    body: Some(body),
                    error: None,
                }),
                None => Err(RuntimeError::Internal {
                    message: format!("tau-wasm-guest: unknown native tool `{name}`"),
                }),
            }
        })
    }
```

> `args` is now used (cloned into the async block); the previous `_args` underscore is removed. Keep `llm_backend_for`/`clock`/`random` unchanged.

- [ ] **Step 3: Confirm the guest still builds standalone for wasm**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-wasm-guest --target wasm32-wasip2 --release 2>&1 | tail -12`
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-wasm-guest/Cargo.toml crates/tau-wasm-guest/src/dispatcher.rs
git commit -m "feat(β.7.5): guest dispatcher routes tools to tau-native-tools (PR-F)"
```

---

## Task 4: Simplified fan-monitor fixture + guest runs it (the PR-F DoD)

**Files:**
- Create: `crates/tau-conformance/fixtures/fan_monitor_simple/tau.toml`
- Create: `crates/tau-conformance/fixtures/fan_monitor_simple/mock_llm.jsonl`
- Create: `crates/tau-wasm-host/tests/fan_monitor_simple.rs`

**Interfaces:**
- Consumes: PR-E2's `tau_wasm_host::run_component`; `tau-ir`/`tau-ir-lower`/`tau-pkg` (already dev-deps of `tau-wasm-host` after PR-E2).

**Notes:**
- The simplified fixture lives under `tau-conformance/fixtures` so PR-G can also run it under the dev profile for the parity gate. PR-F only consumes it from the wasm side.
- Base it on the existing `crates/tau-conformance/fixtures/fan_monitor/tau.toml`, **minus** the `weather` tool (and its `tool_refs` entry) and **minus** the `[[agents.fan-monitor.context.pipeline]]` blocks (context manager is β.4, unshipped). Mirror the existing fixture's model-table schema verbatim so lowering succeeds.

- [ ] **Step 1: Write the fixture**

Create `crates/tau-conformance/fixtures/fan_monitor_simple/tau.toml` (adjust field names to match the existing `fan_monitor/tau.toml` if they differ post-#368):

```toml
packages = ["mock-llm"]

[project]
name = "fan-monitor-simple"

[models.haiku]
backend = "mock-llm"
model = "claude-haiku-4-5"

[agents.fan-monitor]
display_name = "Fan Monitor"
package      = "fan-monitor@^0.1"
model        = "haiku"
tool_refs    = ["read_temp", "set_fan"]
max_turns    = 6

[tools.read_temp]
native      = "ReadTemp"
description = "Read the current temperature."
capabilities = []

[tools.set_fan]
native      = "SetFan"
description = "Set the fan on or off."
capabilities = []
```

Create `crates/tau-conformance/fixtures/fan_monitor_simple/mock_llm.jsonl` (3 turns: read_temp → set_fan → end):

```jsonl
{"turn": 0, "response": {"tool_uses": [{"id": "t0", "name": "read_temp", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"tool_uses": [{"id": "t1", "name": "set_fan", "input": {"on": true}}], "stop_reason": "tool_use"}}
{"turn": 2, "response": {"text": "Fan is on.", "stop_reason": "end_turn"}}
```

> No `weather.cassette.jsonl` — its absence makes the dev profile pick `new_native_only` (per `dev.rs`), which PR-G will rely on.

- [ ] **Step 2: Write the failing integration test**

Create `crates/tau-wasm-host/tests/fan_monitor_simple.rs`:

```rust
//! β.7.5 PR-F DoD: the simplified fan-monitor (read_temp → set_fan → end)
//! runs inside the wasm guest with its native tools sourced from
//! tau-native-tools. Requires wasm32-wasip2 installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_runtime_core::stream::RunEvent;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

/// Lower the simplified fan-monitor fixture to canonical IR bytes.
fn simple_ir_bytes() -> Vec<u8> {
    let toml_path = workspace_root()
        .join("crates/tau-conformance/fixtures/fan_monitor_simple/tau.toml");
    let toml = std::fs::read_to_string(&toml_path).expect("fixture tau.toml exists");
    let config = tau_pkg::project::ProjectConfig::parse_str(&toml).expect("fixture parses");
    let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
    let caches = tau_ir_lower::Caches {
        native_tool: &|_| Some([0u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
    };
    let module = tau_ir_lower::lower_project(&config, &target, &caches).expect("lowers");
    tau_ir::to_canonical_bytes(&module)
}

/// Build the guest with the given IR baked in (mirrors PR-E2's helper).
fn build_guest_with_ir(bytes: &[u8]) -> Vec<u8> {
    let root = workspace_root();
    let target_dir = root.join("target/tau-build-wasm-prf");
    let ir_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir_file.path(), bytes).unwrap();

    let output = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args([
            "build", "-p", "tau-wasm-guest",
            "--target", "wasm32-wasip2", "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TAU_IR_BYTES", ir_file.path())
        .output()
        .expect("cargo spawn");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .flat_map(|m| {
            m["filenames"].as_array().into_iter().flatten()
                .filter_map(|f| f.as_str().map(str::to_string)).collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .expect("a .wasm artifact");
    std::fs::read(wasm_path).unwrap()
}

/// The 3 cassette completions (CompletionResponse JSON) matching the fixture's
/// mock_llm.jsonl: read_temp tool_use, set_fan tool_use, then end_turn.
fn cassette() -> Vec<String> {
    vec![
        r#"{"text":"","tool_uses":[{"id":"t0","name":"read_temp","input":{}}],"stop_reason":"ToolUse","usage":null}"#.to_string(),
        r#"{"text":"","tool_uses":[{"id":"t1","name":"set_fan","input":{"on":true}}],"stop_reason":"ToolUse","usage":null}"#.to_string(),
        r#"{"text":"Fan is on.","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string(),
    ]
}

#[test]
fn simplified_fan_monitor_runs_in_guest() {
    let component = build_guest_with_ir(&simple_ir_bytes());
    let out = tau_wasm_host::run_component(&component, "", cassette()).expect("guest runs");
    let events: Vec<RunEvent> = serde_json::from_str(&out).expect("typed stream");

    // Both native tools were dispatched in-guest, in order.
    let tool_completions: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCallCompleted { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_completions,
        vec!["read_temp", "set_fan"],
        "expected read_temp then set_fan; got {tool_completions:?}"
    );
    assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })));
    assert!(
        !events.iter().any(|e| matches!(e, RunEvent::FatalError { .. })),
        "no fatal errors expected"
    );
}
```

> Confirm `ToolUse.input` is the field name (`{id,name,input}` per `tau_ports::llm::ToolUse`) and `StopReason` serializes as `"ToolUse"`/`"EndTurn"` — both verified against `crates/tau-ports/src/llm.rs`. If `RunEvent::ToolCallCompleted`'s field is `name` (it is, per `stream.rs`), the matcher above is correct.

- [ ] **Step 3: Run the test to verify it fails, then passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-wasm-host --test fan_monitor_simple 2>&1 | tail -25`
Expected: with Tasks 1–3 complete, PASS. If Task 3 (guest tool routing) were missing, the tools would error and the run would `FatalError` — the assertions catch that.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-conformance/fixtures/fan_monitor_simple crates/tau-wasm-host/tests/fan_monitor_simple.rs
git commit -m "feat(β.7.5): simplified fan-monitor runs in-guest with shared native tools (PR-F)"
```

---

## Task 5: ADR-0053 — in-wasm MCP facilitator (decision record)

**Files:**
- Create: `docs/decisions/0053-in-wasm-mcp-facilitator.md`
- Modify: `docs/SUMMARY.md`

**Notes:** This records the *decision* the design demanded; the in-guest weather tool + fixture 07 *implementation* lands in PR-G (it needs a no_std MCP client over `tau_mcp::cassette::Replayer`, which is its own surface). The simplified fan-monitor DoD does not use MCP, so PR-F ships the ADR but not the facilitator code.

- [ ] **Step 1: Write the ADR (house style — mirror `0049-*.md`)**

Create `docs/decisions/0053-in-wasm-mcp-facilitator.md`:

```markdown
# ADR-0053: in-wasm MCP facilitator

**Status:** Accepted
**Date:** 2026-06-20
**Deciders:** Titouan (architect), implementing session
**Supersedes:** none
**Renumbered from:** the β.7.5 design's "ADR-0050" (0050/0051/0052 were taken
by output-schema, the tau-ir crate split, and per-agent model resolution).

## Context

The β.7.5 wasm guest runs the workflow IR with no host imports beyond
inference, clock, and randomness (ADR-0046). The canonical β.6 fan-monitor
includes an MCP `weather` tool. To run that scenario in-guest the facilitator
must execute inside the wasm component — a host MCP import would re-introduce
a transport the determinism + parity story (ADR-0049) does not account for.

`tau-mcp` is `#![no_std]` and already contains the pure cassette
`Replayer` (`crates/tau-mcp/src/cassette/replayer.rs`); the std pieces
(`CassetteTransport`, `McpClient`) live behind `with-std-adapters` /
`tau-mcp-tokio`.

## Decision

1. The MCP facilitator runs **in-guest** on the no_std `tau-mcp` types. The
   conformance `weather` tool replays a **cassette baked into the component**
   via `tau_mcp::cassette::Replayer` — zero host import.
2. A no_std MCP client path over `Replayer` is built for the guest; the std
   `tau-mcp-tokio` `McpClient` stays the host/dev path. Both consume the same
   cassette bytes, so dev and wasm replay identically.
3. **Real (non-cassette) MCP transport from inside wasm is reserved for γ.1**
   via a future `tau:mcp` WIT import slot. β.7.5 ships cassette-only.

## Consequences

- The simplified fan-monitor (PR-F) needs no MCP and ships first; the full
  fan-monitor with in-guest `weather` + conformance fixture `07` lands in
  **PR-G** alongside `WasmMode`.
- Parity holds: the same cassette bytes drive both profiles.
- Risk: untested no_std corners of `tau-mcp` when linked into wasm — PR-G
  smoke-compiles `tau-mcp` for `wasm32-wasip2` before wiring the tool.

## References

- ADR-0046 — wasm AOT artifact + WIT world.
- ADR-0049 — single-channel typed conformance observable.
- `docs/superpowers/specs/2026-06-14-beta-7-5-wasm-aot-design.md` §10–§11.
```

- [ ] **Step 2: Add to SUMMARY.md**

In `docs/SUMMARY.md`, in the architecture-decisions list, after the ADR-0052 line, add:

```markdown
- [ADR-0053 — In-wasm MCP facilitator (β.7.5)](decisions/0053-in-wasm-mcp-facilitator.md)
```

- [ ] **Step 3: Build the book (docs gate)**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build 2>&1 | tail -15 && rm -rf book`
Expected: only `[INFO]` lines, no broken-link errors. (If `mdbook`/`mdbook-linkcheck` are absent, note it and skip — CI's `docs-deploy` is the gate.)

- [ ] **Step 4: Commit**

```bash
git add docs/decisions/0053-in-wasm-mcp-facilitator.md docs/SUMMARY.md
git commit -m "docs(β.7.5): ADR-0053 in-wasm MCP facilitator (PR-F)"
```

---

## Final verification (before opening the PR)

- [ ] `timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-native-tools`
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-conformance` (golden unchanged)
- [ ] `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-wasm-host`
- [ ] `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-wasm-guest --target wasm32-wasip2 --release`
- [ ] `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-native-tools -p tau-conformance -p tau-wasm-host`
- [ ] `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check`

## Out of scope (PR-G, or blocked)

- Full fan-monitor with in-guest MCP `weather` + conformance fixture `07` (PR-G; needs the no_std MCP client over `Replayer`).
- `WasmMode` as a third `ExecutionMode`, the `conformance (wasm)` CI lane, flipping `fan_monitor_dev_matches_wasm` live, the byte-equal parity fixture (PR-G).
- β.4 context manager + fixture `13_context_pipeline` (blocked on β.4).
- Threading the WIT `prompt` into a user `Message` (γ — would break dev↔wasm parity if added now).
