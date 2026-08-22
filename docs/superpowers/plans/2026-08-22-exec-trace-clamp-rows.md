# Execution-Trace TUI Clamp Rows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A meet-clamped MCP tool call renders a `clamp:<to>` row in the trace-TUI waterfall — produced by threading the open-time narrowing decision onto the tool object and mapping it to `CapabilityVerdict::Clamp` at the kernel's existing per-call `ToolCall` emit sites.

**Architecture:** Authority-on-the-object (ocap pattern, in-repo precedent `AttenuatedDispatcher`): a new defaulted `Tool::effective_capabilities()` port method carries the narrowed authority; `setup_mcp_runtime` (tau-cli) computes the per-server-tool meet against the entry's clamped `CapabilityPlan` and stores it on `McpBackedTool`; the kernel (`stream.rs`) maps it to `Clamp { to }` via a shared `capability_verdict` helper at both `ToolCall` emit sites. Observability only — the kernel gate and OS-boundary enforcement are unchanged.

**Tech Stack:** Rust workspace crates `tau-ports`, `tau-runtime-core`, `tau-mcp-tokio`, `tau-cli`; `tau_domain::{meet, capability_subset}` lattice API; mdBook docs.

**Spec:** `docs/superpowers/specs/2026-08-21-execution-trace-tui-design.md` §12 (read §12 in full before starting any task).

## Global Constraints

- **CARGO RULES (root `CLAUDE.md`) — every cargo invocation, no exceptions:**
  `timeout <t> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <single-crate>`
  Timeouts: test 300, build/check 180, clippy 240, fmt 30. Prefer `cargo nextest run` for tests (matches CI). Never `--workspace`, never bare `cargo`, never omit `-p`. Type the `env …` prefix literally (zsh does not word-split shell vars).
- **Commits:** `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."` — conventional, imperative, scoped.
- **tau-ports semver:** this plan adds a *defaulted* trait method → minor: `0.7.0` → `0.7.1` in `crates/tau-ports/Cargo.toml` AND the root `Cargo.toml` pin (line ~91). The `tau-ports ABI (cargo-semver-checks)` CI job is the arbiter; if it demands major, re-bump to `0.8.0` in both places (do NOT restructure the change).
- **`tau-runtime-core` is `#![no_std]` + alloc:** in non-test code use `alloc::` types (`alloc::string::String`, `alloc::collections::BTreeSet`, `alloc::vec::Vec`); test modules may use `std`.
- **`Capability`/variants are `#[non_exhaustive]` outside tau-domain:** construct test capabilities via serde round-trip (`toml::from_str` with a `CapWrapper` in tau-runtime-core; `serde_json::from_value` in tau-cli — both patterns exist in the files you'll touch). `HostSet` (`Any` | `Exact(BTreeSet<HostName>)`) is exhaustive — match WITHOUT a wildcard arm (`-D warnings` rejects unreachable arms).
- **Branch:** work on `exec-trace-tui-clamp-rows` (already checked out). Do not rename it.

---

### Task 1: `Tool::effective_capabilities()` port method (tau-ports)

**Files:**
- Modify: `crates/tau-ports/src/tool.rs` (Tool trait, after `capabilities()` ~line 311)
- Modify: `crates/tau-ports/Cargo.toml` (version `0.7.0` → `0.7.1`)
- Modify: `Cargo.toml` (workspace root, tau-ports pin ~line 91: `version = "0.7.0"` → `"0.7.1"`)
- Test: same file, new `#[cfg(test)]` module at the bottom of `tool.rs`

**Interfaces:**
- Produces: `Tool::effective_capabilities(&self) -> Option<&[tau_domain::Capability]>` — defaulted to `None`. Tasks 2–4 rely on this exact name and signature.

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/tau-ports/src/tool.rs` (there is no existing test module in this file — add one):

```rust
#[cfg(test)]
mod effective_capabilities_tests {
    use super::Tool;
    use crate::fixtures::{make_tool_spec, MockTool};
    use tau_domain::Value;

    #[test]
    fn default_effective_capabilities_is_none() {
        // The defaulted method means every existing Tool impl reports
        // "not narrowed" without changes.
        let tool = MockTool::new(
            "noop",
            make_tool_spec("noop".into(), "noop".into(), Value::Null),
        );
        assert!(Tool::effective_capabilities(&tool).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports default_effective_capabilities_is_none`
Expected: COMPILE FAIL — `no function or associated item named 'effective_capabilities'`

- [ ] **Step 3: Add the defaulted method**

In the `Tool` trait in `crates/tau-ports/src/tool.rs`, directly after the `capabilities()` method (after its closing `}` at ~line 311):

```rust
    /// The authority this tool actually runs under, when narrower than
    /// [`Tool::capabilities`] — e.g. an MCP entry whose `net.http` hosts
    /// were meet-clamped against the `[allow.mcp.<entry>].hosts` ceiling
    /// at open time. `None` (the default) means the runtime authority
    /// equals the declared capabilities.
    ///
    /// Observability-only in v0.1: the kernel maps `Some` to a
    /// `CapabilityVerdict::Clamp` on the call's `ToolCall` trace event.
    /// Enforcement is unchanged — the kernel grant gate still checks the
    /// declared capabilities, and the OS boundary enforces the narrowed
    /// plan (execution-trace TUI spec §12).
    fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
        None
    }
```

- [ ] **Step 4: Bump versions**

- `crates/tau-ports/Cargo.toml` line 4: `version = "0.7.1"`
- Root `Cargo.toml` line ~91: `tau-ports = { path = "crates/tau-ports",  version = "0.7.1", default-features = false }` (change only the version string; keep alignment/whitespace as-is)

- [ ] **Step 5: Run test to verify it passes (also refreshes Cargo.lock)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports default_effective_capabilities_is_none`
Expected: PASS. Then `git status` — `Cargo.lock` should show the 0.7.1 bump; if not, run `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ports`.

- [ ] **Step 6: Crate gates**

Run (each, in order):
- `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ports -- --check` (on failure: run without `--check`, re-verify)
- `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ports --all-targets -- -D warnings`
Expected: both clean.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ports/src/tool.rs crates/tau-ports/Cargo.toml Cargo.toml Cargo.lock
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ports): add defaulted Tool::effective_capabilities (0.7.1)"
```

---

### Task 2: `DynTool::effective_capabilities` + blanket forwarding (tau-runtime-core)

**Files:**
- Modify: `crates/tau-runtime-core/src/builder.rs` (trait `DynTool` ~line 126–150; blanket `impl<T: Tool<Session = ()>> DynTool for T` ~line 152–186)
- Test: existing `#[cfg(test)]` tests module in the same file (a `impl DynTool for TestSchemaTool` exists near line 887 — put the new test in that module)

**Interfaces:**
- Consumes: `Tool::effective_capabilities` from Task 1.
- Produces: `DynTool::effective_capabilities(&self) -> Option<&[tau_domain::Capability]>` (defaulted `None`; blanket impl forwards to `Tool::effective_capabilities`). Task 3's kernel helper calls this through `&dyn DynTool`.

- [ ] **Step 1: Write the failing test**

In the existing tests module of `builder.rs`:

```rust
    #[test]
    fn blanket_dyn_tool_forwards_effective_capabilities() {
        use std::sync::Arc;

        // A Tool overriding effective_capabilities must surface it through
        // the blanket DynTool impl under dyn dispatch — not fall back to
        // the DynTool default of None.
        struct NarrowedTool {
            inner: tau_ports::fixtures::MockTool,
            effective: Vec<tau_domain::Capability>,
        }

        impl tau_ports::Tool for NarrowedTool {
            type Session = ();

            fn name(&self) -> &str {
                tau_ports::Tool::name(&self.inner)
            }

            fn schema(&self) -> tau_ports::ToolSpec {
                tau_ports::Tool::schema(&self.inner)
            }

            fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
                Some(&self.effective)
            }

            async fn init(
                &self,
                ctx: tau_ports::SessionContext,
            ) -> Result<Self::Session, tau_ports::ToolError> {
                tau_ports::Tool::init(&self.inner, ctx).await
            }

            async fn invoke(
                &self,
                session: &mut Self::Session,
                args: tau_domain::Value,
            ) -> Result<tau_ports::ToolResult, tau_ports::ToolError> {
                tau_ports::Tool::invoke(&self.inner, session, args).await
            }

            async fn teardown(&self, session: Self::Session) -> Result<(), tau_ports::ToolError> {
                tau_ports::Tool::teardown(&self.inner, session).await
            }
        }

        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: tau_domain::Capability,
        }
        let cap: tau_domain::Capability = toml::from_str::<CapWrapper>(
            "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n",
        )
        .unwrap()
        .cap;

        let spec = tau_ports::fixtures::make_tool_spec(
            "narrowed".into(),
            "narrowed".into(),
            tau_domain::Value::Null,
        );
        let tool: Arc<dyn DynTool> = Arc::new(NarrowedTool {
            inner: tau_ports::fixtures::MockTool::new("narrowed", spec),
            effective: vec![cap.clone()],
        });

        assert_eq!(tool.effective_capabilities(), Some(&[cap][..]));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core blanket_dyn_tool_forwards_effective_capabilities`
Expected: FAIL — the assert returns `None` if only the trait default exists, or COMPILE FAIL if `DynTool` has no such method yet.

- [ ] **Step 3: Add method + forwarding**

In `pub trait DynTool` (builder.rs, directly after the `capabilities()` declaration ~line 134):

```rust
    /// Runtime authority when narrower than [`DynTool::capabilities`] —
    /// see `Tool::effective_capabilities` (execution-trace TUI spec §12).
    /// Default: not narrowed.
    fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
        None
    }
```

In the blanket `impl<T: Tool<Session = ()> + 'static> DynTool for T` (directly after its `capabilities` forwarding ~line 163):

```rust
    fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
        Tool::effective_capabilities(self)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core blanket_dyn_tool_forwards_effective_capabilities`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/builder.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime-core): forward effective_capabilities through DynTool"
```

---

### Task 3: kernel verdict mapping + producer test (tau-runtime-core)

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs` — two helper fns near `emit_capability_drop` (~line 1899); the two `ToolCall` emit sites (~line 1648 schema-invalid path, ~line 1801 success path)
- Test: existing tests module in `stream.rs` (near `dispatch_emits_toolcall_trace_event_with_verdict` ~line 3975)

**Interfaces:**
- Consumes: `DynTool::effective_capabilities` (Task 2); existing test scaffolding `collecting_trace_state`, `first_tool_call`, `make_tool_entry`, `test_run_options`, `run_streaming_inner`, `ScriptedLlm`, `collect_events`, `agent_def`, `manifest_with_no_capabilities`, `user_msg` (all already in stream.rs tests).
- Produces: `fn capability_verdict(tool: &dyn DynTool, required: &[Capability]) -> Option<tau_ports::CapabilityVerdict>` and `fn render_clamped_to(effective: &[Capability]) -> String` (private to stream.rs; no later task consumes them directly).

- [ ] **Step 1: Write the failing unit tests (helpers)**

In the stream.rs tests module (next to `first_tool_call`):

```rust
    /// Build a Capability from its canonical TOML form (variants are
    /// #[non_exhaustive] outside tau-domain). Same pattern as capability.rs.
    fn test_cap(toml_str: &str) -> tau_domain::Capability {
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: tau_domain::Capability,
        }
        toml::from_str::<CapWrapper>(toml_str).unwrap().cap
    }

    #[test]
    fn render_clamped_to_joins_sorted_hosts() {
        let eff = vec![test_cap(
            "[cap]\nkind = \"net.http\"\nhosts = [\"b.example\", \"a.example\"]\n",
        )];
        assert_eq!(super::render_clamped_to(&eff), "a.example,b.example");
    }

    #[test]
    fn render_clamped_to_any_hosts() {
        let eff = vec![test_cap("[cap]\nkind = \"net.http\"\nhosts = \"any\"\n")];
        assert_eq!(super::render_clamped_to(&eff), "any");
    }

    #[test]
    fn render_clamped_to_without_net_caps_is_none() {
        // Empty-meet fail-closed case: no net authority survived the clamp.
        let eff = vec![test_cap("[cap]\nkind = \"fs.read\"\npaths = [\"/tmp/**\"]\n")];
        assert_eq!(super::render_clamped_to(&eff), "none");
        assert_eq!(super::render_clamped_to(&[]), "none");
    }

    #[test]
    fn capability_verdict_empty_required_is_none_default_is_allow() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};
        let tool = MockTool::new(
            "plain",
            make_tool_spec("plain".into(), "plain".into(), Value::Null),
        );
        let tool: Arc<dyn DynTool> = Arc::new(tool);
        // Un-gated tool → no verdict at all.
        assert_eq!(super::capability_verdict(&*tool, &[]), None);
        // Gated tool without narrowing → Allow.
        let req = vec![test_cap("[cap]\nkind = \"fs.read\"\npaths = [\"/tmp/**\"]\n")];
        assert_eq!(
            super::capability_verdict(&*tool, &req),
            Some(tau_ports::CapabilityVerdict::Allow)
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core render_clamped_to`
Expected: COMPILE FAIL — `render_clamped_to` / `capability_verdict` not found.

- [ ] **Step 3: Implement the helpers**

In `stream.rs`, directly above `fn emit_capability_drop` (~line 1899):

```rust
/// Render the `to` of a clamp verdict: the sorted, comma-joined host list
/// of the effective net authority. `any` when any effective net cap is
/// host-unbounded; `none` when no net cap survived the open-time meet
/// (fail-closed empty meet). Execution-trace TUI spec §12.5 — rendering
/// lives kernel-side so the port carries semantic `Capability` values only.
fn render_clamped_to(effective: &[Capability]) -> String {
    use tau_domain::{HostSet, NetCapability};
    let mut any = false;
    let mut hosts: alloc::collections::BTreeSet<String> = alloc::collections::BTreeSet::new();
    for cap in effective {
        if let Capability::Network(NetCapability::Http { hosts: h, .. }) = cap {
            match h {
                HostSet::Any => any = true,
                HostSet::Exact(set) => {
                    hosts.extend(set.iter().map(|hn| String::from(hn.as_str())));
                }
            }
        }
    }
    if any {
        String::from("any")
    } else if hosts.is_empty() {
        String::from("none")
    } else {
        hosts.into_iter().collect::<Vec<String>>().join(",")
    }
}

/// Map a dispatch-completed call's capability posture to the trace verdict
/// (execution-trace TUI spec §12.4). `None` for un-gated tools; `Clamp`
/// when the tool reports open-time-narrowed authority; `Allow` otherwise.
/// Denials never reach this — they abort dispatch earlier and are recorded
/// by `emit_capability_drop`.
fn capability_verdict(
    tool: &dyn DynTool,
    required: &[Capability],
) -> Option<tau_ports::CapabilityVerdict> {
    if required.is_empty() {
        return None;
    }
    Some(match tool.effective_capabilities() {
        Some(eff) => tau_ports::CapabilityVerdict::Clamp {
            to: render_clamped_to(eff),
        },
        None => tau_ports::CapabilityVerdict::Allow,
    })
}
```

Note: `Capability` is already imported at the top of stream.rs (via `use tau_domain::{...}`); `NetCapability`/`HostSet` are root re-exports of tau-domain — import them in-function as shown to avoid widening the file-top import.

- [ ] **Step 4: Run to verify helpers pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core render_clamped_to capability_verdict_empty`
Expected: PASS (4 tests)

- [ ] **Step 5: Write the failing producer test**

Next to `dispatch_emits_toolcall_trace_event_with_verdict` (~line 3975) — same harness, but the tool reports narrowed authority:

```rust
    #[tokio::test]
    async fn clamped_tool_emits_clamp_trace_event() {
        use tau_ports::fixtures::{make_tool_spec, MockTool};

        // A tool whose runtime authority was narrowed at open time (an MCP
        // entry meet-clamped by [allow.mcp.<entry>].hosts): declared caps
        // request two hosts, effective caps carry one. The grant covers the
        // DECLARED caps — kernel gate semantics unchanged — so dispatch
        // succeeds, but the ToolCall verdict must be Clamp, not Allow.
        struct ClampedTool {
            inner: MockTool,
            required: Vec<tau_domain::Capability>,
            effective: Vec<tau_domain::Capability>,
        }

        impl tau_ports::Tool for ClampedTool {
            type Session = ();

            fn name(&self) -> &str {
                tau_ports::Tool::name(&self.inner)
            }

            fn schema(&self) -> tau_ports::ToolSpec {
                tau_ports::Tool::schema(&self.inner)
            }

            fn capabilities(&self) -> &[tau_domain::Capability] {
                &self.required
            }

            fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
                Some(&self.effective)
            }

            async fn init(
                &self,
                ctx: tau_ports::SessionContext,
            ) -> Result<Self::Session, tau_ports::ToolError> {
                tau_ports::Tool::init(&self.inner, ctx).await
            }

            async fn invoke(
                &self,
                session: &mut Self::Session,
                args: tau_domain::Value,
            ) -> Result<tau_ports::ToolResult, tau_ports::ToolError> {
                tau_ports::Tool::invoke(&self.inner, session, args).await
            }

            async fn teardown(&self, session: Self::Session) -> Result<(), tau_ports::ToolError> {
                tau_ports::Tool::teardown(&self.inner, session).await
            }
        }

        let declared = test_cap(
            "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\", \"evil.example\"]\n",
        );
        let effective =
            test_cap("[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n");

        let spec = make_tool_spec("echo".into(), "echo tool".into(), Value::Null);
        let tool = ClampedTool {
            inner: MockTool::new("echo", spec),
            required: vec![declared.clone()],
            effective: vec![effective],
        };
        let tool_arc: Arc<dyn DynTool> = Arc::new(tool);
        let (tools, validators, tool_specs_list) = make_tool_entry("echo", tool_arc);

        let (state_arc, collector) = collecting_trace_state("run-clamp-trace");
        let options = {
            let mut o = test_run_options();
            o.orchestration_state = Some(state_arc.clone());
            o
        };

        // Two-turn script: ToolUse("echo") then a plain-text final turn —
        // same shape as dispatch_emits_toolcall_trace_event_with_verdict.
        let llm: Arc<dyn DynLlmBackend> = Arc::new(ScriptedLlm::multi_turn(vec![
            vec![
                Ok(CompletionChunk::ToolUse(tau_ports::fixtures::make_tool_use(
                    "call_1".into(),
                    "echo".into(),
                    Value::Null,
                ))),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::ToolUse,
                    usage: Some(PortsTokenUsage::new(10, 5)),
                }),
            ],
            vec![
                Ok(CompletionChunk::Text {
                    delta: "Done!".into(),
                }),
                Ok(CompletionChunk::Finish {
                    stop_reason: PortsStopReason::EndTurn,
                    usage: Some(PortsTokenUsage::new(5, 3)),
                }),
            ],
        ]));

        let stream = run_streaming_inner(
            llm,
            agent_def(),
            manifest_with_no_capabilities(),
            vec![],
            user_msg("hi"),
            options,
            tools,
            validators,
            vec![declared.clone()], // grant covers DECLARED → kernel gate passes
            tool_specs_list,
            vec![],
            vec![declared],
        );
        let _events = collect_events(Box::pin(stream)).await;

        let trace_events = collector.0.lock().unwrap();
        let tool_call =
            first_tool_call(&trace_events).expect("a ToolCall trace event must be emitted");

        assert_eq!(tool_call.0, "echo");
        assert_eq!(tool_call.2, "ok");
        match tool_call.3 {
            Some(tau_ports::CapabilityVerdict::Clamp { ref to }) => {
                assert_eq!(to, "api.weather.com");
            }
            ref other => panic!("expected Clamp verdict, got {other:?}"),
        }
    }
```

(If `test_cap` from Step 1 lives in a different sub-module scope, move it so both test sites can call it — one definition only.)

- [ ] **Step 6: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core clamped_tool_emits_clamp_trace_event`
Expected: FAIL — verdict is `Some(Allow)` (emit sites not wired yet).

- [ ] **Step 7: Wire both emit sites**

**Success path (~line 1794–1805).** Replace the comment + inline block:

```rust
                    // The `missing.is_some()` branch above already returned
                    // before dispatch, so reaching here means every required
                    // capability was satisfied: a non-empty requirement maps
                    // to `Allow`. `Clamp`/`Drop` are produced by other
                    // capability-decision sites (e.g. the MCP sandbox's
                    // egress meet-clamp — PR-5.1), never by this in-kernel
                    // dispatch-site check, which is a pass/fail gate.
                    let capability = if required.is_empty() {
                        None
                    } else {
                        Some(tau_ports::CapabilityVerdict::Allow)
                    };
```

with:

```rust
                    // The `missing.is_some()` branch above already returned
                    // before dispatch, so reaching here means the kernel gate
                    // passed: the verdict is `Allow`, or `Clamp` when the tool
                    // reports open-time-narrowed authority (spec §12.4).
                    // `Drop` rows come from `emit_capability_drop`.
                    let capability = capability_verdict(&*tool, required);
```

**Schema-invalid path (~line 1634–1652).** Replace the trailing sentences of that site's comment (from "See the success-path comment…" through "…is always `Allow` here too.") plus its inline block:

```rust
                            let capability = if required.is_empty() {
                                None
                            } else {
                                Some(tau_ports::CapabilityVerdict::Allow)
                            };
```

with a comment pointing at the helper and the same call:

```rust
                            let capability = capability_verdict(&*tool, required);
```

(`tool` at both sites is the resolved `Arc<dyn DynTool>`/reference from the registry lookup at ~line 1447; `&*tool` derefs to `&dyn DynTool` — if the binding is a `&Arc`, use `&**tool`. The compiler will tell you.)

- [ ] **Step 8: Run the full crate test suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: ALL PASS — including `clamped_tool_emits_clamp_trace_event`, the pre-existing `dispatch_emits_toolcall_trace_event_with_verdict` (Allow unchanged), the error-status test, and the drop test.

- [ ] **Step 9: Crate gates**

- `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check` (fix + re-check if needed)
- `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add crates/tau-runtime-core/src/stream.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime-core): emit Clamp verdict for tools with narrowed authority"
```

---

### Task 4: `McpBackedTool` carries its effective capabilities (tau-mcp-tokio)

**Files:**
- Modify: `crates/tau-mcp-tokio/src/bridge.rs` (struct + `new` + `impl Tool`)
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs:1022` (call site — pass `None` for now; Task 5 supplies the real value)
- Modify: `crates/tau-cli/tests/cmd_run_mcp.rs:62` (call site + new assertions)

**Interfaces:**
- Consumes: `Tool::effective_capabilities` (Task 1).
- Produces: `McpBackedTool::new(ir_tool_id, client, server_tool_name, capabilities: Vec<Capability>, effective_capabilities: Option<Vec<Capability>>, input_schema_json, description) -> Arc<Self>` — the new 5th parameter, `None` = not narrowed. Task 5 passes `Some(...)` here.

- [ ] **Step 1: Write the failing test**

In `crates/tau-cli/tests/cmd_run_mcp.rs`, extend `mcp_backed_tool_round_trips_via_cassette`: change line 56 to keep the client reusable (`let client = Arc::new(...)` stays; add `.clone()` at the first use), update the existing `McpBackedTool::new` call to the new signature with `None`, and append after the existing assertions:

```rust
    // Clamp threading (spec §12): a tool constructed with Some(effective)
    // must surface it via Tool::effective_capabilities; None (the tool
    // above) must report not-narrowed.
    assert!(tau_ports::Tool::effective_capabilities(&*tool).is_none());

    let declared: tau_domain::Capability = serde_json::from_value(serde_json::json!({
        "kind": "net.http",
        "hosts": ["api.weather.com", "evil.example"],
    }))
    .expect("declared cap");
    let effective: tau_domain::Capability = serde_json::from_value(serde_json::json!({
        "kind": "net.http",
        "hosts": ["api.weather.com"],
    }))
    .expect("effective cap");

    let clamped = McpBackedTool::new(
        "weather.echo2",
        client,
        "echo",
        vec![declared],
        Some(vec![effective.clone()]),
        serde_json::json!({}),
        "echo tool",
    );
    assert_eq!(
        tau_ports::Tool::effective_capabilities(&*clamped),
        Some(&[effective][..])
    );
```

(First use of the client becomes `client.clone()`; the second, moved use is shown above. Add `use tau_domain::Capability;`-style imports only if the file lacks them — it already imports `Value` from tau_domain.)

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli mcp_backed_tool_round_trips_via_cassette`
Expected: COMPILE FAIL — `new` takes 6 arguments but 7 were supplied.

- [ ] **Step 3: Implement**

In `crates/tau-mcp-tokio/src/bridge.rs`:

Struct — after the `capabilities` field (~line 31):

```rust
    /// Runtime authority when the entry's meet-clamped `CapabilityPlan`
    /// narrows this tool's declared net caps (execution-trace TUI spec
    /// §12.3). `None` = not narrowed. Computed by the runtime composer
    /// (`setup_mcp_runtime`), not here — the bridge just carries it.
    effective_capabilities: Option<Vec<Capability>>,
```

`new` — add the parameter after `capabilities` and store it:

```rust
    pub fn new(
        ir_tool_id: impl Into<String>,
        client: Arc<McpClient>,
        server_tool_name: impl Into<String>,
        capabilities: Vec<Capability>,
        effective_capabilities: Option<Vec<Capability>>,
        input_schema_json: serde_json::Value,
        description: impl Into<String>,
    ) -> Arc<Self> {
```

(and `effective_capabilities,` in the `Self { ... }` literal).

`impl Tool for McpBackedTool` — after `capabilities()` (~line 94):

```rust
    fn effective_capabilities(&self) -> Option<&[Capability]> {
        self.effective_capabilities.as_deref()
    }
```

Call site `crates/tau-cli/src/cmd/ir_dispatcher.rs:1022` — insert `None,` between `st.caps.clone(),` and `st.input_schema.0.clone(),`.

- [ ] **Step 4: Run to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-mcp-tokio` then `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli mcp_backed_tool_round_trips_via_cassette`
Expected: PASS.

- [ ] **Step 5: Crate gates**

- `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-mcp-tokio -- --check` and `... -p tau-cli -- --check` (fix + re-check if needed)
- `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-mcp-tokio --all-targets -- -D warnings`
Expected: clean. (tau-cli clippy runs in Task 5 — it's touched again there.)

- [ ] **Step 6: Commit**

```bash
git add crates/tau-mcp-tokio/src/bridge.rs crates/tau-cli/src/cmd/ir_dispatcher.rs crates/tau-cli/tests/cmd_run_mcp.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(mcp): McpBackedTool carries open-time effective capabilities"
```

---

### Task 5: compute the per-tool meet in `setup_mcp_runtime` (tau-cli)

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs` — new helper below `mcp_capability_plan` (~line 939); call site ~line 1020–1031
- Test: existing `mod sandbox_plan_tests` in the same file (~line 1106; reuse its `cap()` and `allow_with_hosts()` fixtures)

**Interfaces:**
- Consumes: `mcp_capability_plan` (existing), `McpBackedTool::new` with the `effective_capabilities` param (Task 4), `tau_domain::{meet, capability_subset}` (root re-exports).
- Produces: `fn tool_effective_capabilities(declared: &[tau_domain::Capability], plan: &CapabilityPlan) -> Option<Vec<tau_domain::Capability>>` (private to ir_dispatcher.rs).

- [ ] **Step 1: Write the failing tests**

In `mod sandbox_plan_tests` (add `tool_effective_capabilities` to the `use super::{...}` list):

```rust
    #[test]
    fn tool_without_net_caps_is_never_clamped() {
        let allow = allow_with_hosts("weather", &["api.weather.com"]);
        let envelope = vec![cap(serde_json::json!({
            "kind": "net.http",
            "hosts": ["api.weather.com", "evil.example"],
        }))];
        let plan = mcp_capability_plan("weather", &envelope, Some(&allow)).expect("plan");
        let declared = vec![cap(serde_json::json!({"kind": "fs.read", "paths": ["/tmp/**"]}))];
        assert_eq!(tool_effective_capabilities(&declared, &plan), None);
    }

    #[test]
    fn tool_covered_by_plan_is_not_clamped() {
        let allow = allow_with_hosts("weather", &["api.weather.com"]);
        let envelope = vec![cap(serde_json::json!({
            "kind": "net.http",
            "hosts": ["api.weather.com", "evil.example"],
        }))];
        let plan = mcp_capability_plan("weather", &envelope, Some(&allow)).expect("plan");
        // This tool only ever declared the allowed host — nothing narrowed.
        let declared = vec![cap(serde_json::json!({
            "kind": "net.http",
            "hosts": ["api.weather.com"],
        }))];
        assert_eq!(tool_effective_capabilities(&declared, &plan), None);
    }

    #[test]
    fn narrowed_tool_reports_effective_with_clamped_hosts() {
        let allow = allow_with_hosts("weather", &["api.weather.com"]);
        let envelope = vec![cap(serde_json::json!({
            "kind": "net.http",
            "hosts": ["api.weather.com", "evil.example"],
        }))];
        let plan = mcp_capability_plan("weather", &envelope, Some(&allow)).expect("plan");
        let fs = cap(serde_json::json!({"kind": "fs.read", "paths": ["/tmp/**"]}));
        let declared = vec![
            fs.clone(),
            cap(serde_json::json!({
                "kind": "net.http",
                "hosts": ["api.weather.com", "evil.example"],
            })),
        ];
        let effective =
            tool_effective_capabilities(&declared, &plan).expect("narrowed → Some");
        // Non-net declared caps pass through untouched.
        assert!(effective.contains(&fs));
        // Exactly one net cap, meet-clamped to the ceiling host.
        let nets: Vec<&Capability> = effective
            .iter()
            .filter(|c| matches!(c, Capability::Network(_)))
            .collect();
        assert_eq!(nets.len(), 1, "one clamped net cap: {effective:?}");
        let net_json = serde_json::to_value(nets[0]).expect("serialize");
        assert_eq!(net_json["hosts"], serde_json::json!(["api.weather.com"]));
    }

    #[test]
    fn empty_meet_reports_effective_without_net_caps() {
        // The plan dropped the disjoint net cap entirely (fail-closed) —
        // the tool is clamped to zero net authority, which must surface as
        // Some(effective-without-net), not None (kernel renders `none`).
        let allow = allow_with_hosts("weather", &["api.weather.com"]);
        let envelope = vec![cap(serde_json::json!({
            "kind": "net.http",
            "hosts": ["evil.example"],
        }))];
        let plan = mcp_capability_plan("weather", &envelope, Some(&allow)).expect("plan");
        let declared = vec![cap(serde_json::json!({
            "kind": "net.http",
            "hosts": ["evil.example"],
        }))];
        let effective =
            tool_effective_capabilities(&declared, &plan).expect("clamped → Some");
        assert!(
            !effective.iter().any(|c| matches!(c, Capability::Network(_))),
            "no net authority survives: {effective:?}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli tool_effective -E 'test(/clamp|tool_/)'` — or simply `cargo nextest run -p tau-cli sandbox_plan_tests` with the same env prefix.
Expected: COMPILE FAIL — `tool_effective_capabilities` not found.

- [ ] **Step 3: Implement the helper**

Directly below `mcp_capability_plan` (~line 939):

```rust
/// Per-server-tool narrowed authority (execution-trace TUI spec §12.3).
///
/// `Some(effective)` iff the entry's meet-clamped [`CapabilityPlan`]
/// narrows this tool's declared `net.http` caps — i.e. the declared net
/// caps are not a subset of the plan's. The effective set is the tool's
/// non-net declared caps plus `meet(declared_net, plan_net)` (which may
/// contain no net cap at all after a fail-closed empty meet — still a
/// clamp; the kernel renders it `none`). `None` = not narrowed: no net
/// caps declared, ungoverned plan, or the ceiling already covers the
/// declared hosts. Only hosts narrow today — the `[allow.mcp]` registry
/// is an any-method host ceiling — but the comparison spans full net
/// caps so a method-carrying ceiling needs no rework here.
fn tool_effective_capabilities(
    declared: &[tau_domain::Capability],
    plan: &CapabilityPlan,
) -> Option<Vec<tau_domain::Capability>> {
    let (declared_net, declared_rest): (
        Vec<tau_domain::Capability>,
        Vec<tau_domain::Capability>,
    ) = declared
        .iter()
        .cloned()
        .partition(|c| matches!(c, tau_domain::Capability::Network(_)));
    if declared_net.is_empty() {
        return None;
    }
    let plan_net: Vec<tau_domain::Capability> = plan
        .capabilities
        .iter()
        .filter(|c| matches!(c, tau_domain::Capability::Network(_)))
        .cloned()
        .collect();
    if tau_domain::capability_subset(&declared_net, &plan_net).is_ok() {
        return None;
    }
    let mut effective = declared_rest;
    effective.extend(tau_domain::meet(&declared_net, &plan_net));
    Some(effective)
}
```

Wire the call site (~line 1020–1031) — compute before constructing the tool and replace the Task-4 `None`:

```rust
        for st in &arc_client.contract().tools {
            let ir_tool_id = tau_ir::ids::ToolId(format!("{}.{}", locked.entry, st.name));
            let effective = tool_effective_capabilities(&st.caps, &plan);
            let mcp_tool: Arc<dyn DynTool> = McpBackedTool::new(
                ir_tool_id.0.clone(),
                arc_client.clone(),
                st.name.clone(),
                st.caps.clone(),
                effective,
                st.input_schema.0.clone(),
                st.description.clone().unwrap_or_default(),
            );
            tools.push((ir_tool_id, mcp_tool));
        }
```

- [ ] **Step 4: Run to verify tests pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli sandbox_plan_tests`
Expected: ALL PASS (the 4 new + the pre-existing plan tests).

- [ ] **Step 5: Full tau-cli suite + gates**

- `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
- `timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-cli -- --check` (fix + re-check if needed)
- `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings`
Expected: all clean.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(cli): thread per-tool meet-clamped authority into McpBackedTool"
```

---

### Task 6: docs — `clamp` badge row in `reference/tau-trace.md`

**Files:**
- Modify: `docs/reference/tau-trace.md` (badge table, lines 54–58)

**Interfaces:** none (docs only). The page is already in `docs/SUMMARY.md`.

- [ ] **Step 1: Add the row**

Insert between the `allow` and `drop:<reason>` rows of the badge table:

```markdown
| `clamp:<to>` (amber) | The call ran, but under authority narrowed at MCP open time: the entry's `net.http` hosts were meet-clamped against the `[allow.mcp.<entry>].hosts` ceiling. `<to>` is the effective host list (`any` = host-unbounded; `none` = no net authority survived the clamp). Observability only — enforcement happens at the OS boundary. |
```

- [ ] **Step 2: Build the book (DOCS RULES)**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines. Then `rm -rf docs/book` (gitignored, but keep the tree clean).

- [ ] **Step 3: Commit**

```bash
git add docs/reference/tau-trace.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(trace): document the clamp capability badge"
```

---

### Task 7: full gates, push, PR

**Files:** none new.

- [ ] **Step 1: Doctests on touched crates**

Run (each): `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ports` and `... -p tau-runtime-core` and `... -p tau-mcp-tokio` and `... -p tau-cli`
Expected: PASS (nextest doesn't run doctests; CI does).

- [ ] **Step 2: Re-verify the four crates' suites are green** (skip any already run un-modified since)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate>` for `tau-ports`, `tau-runtime-core`, `tau-mcp-tokio`, `tau-cli`.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin exec-trace-tui-clamp-rows
gh pr create --base main --title "feat(trace): clamp rows — per-tool meet-clamped authority on ToolCall events" --body "$(cat <<'EOF'
Closes the deferred item 2 of the execution-trace TUI (M1.5, #618).

A meet-clamped MCP tool call now renders a `clamp:<to>` waterfall row:
`setup_mcp_runtime` computes each server-tool's effective authority
(meet of its declared net caps against the entry's `[allow.mcp]`-clamped
`CapabilityPlan`) and threads it onto `McpBackedTool` via the new
defaulted `Tool::effective_capabilities()` port method (tau-ports
0.7.1, additive); the kernel maps it to `CapabilityVerdict::Clamp` at
both existing `ToolCall` emit sites. Observability only — the kernel
grant gate and OS-boundary enforcement are unchanged.

Design: `docs/superpowers/specs/2026-08-21-execution-trace-tui-design.md` §12.

- tau-ports: defaulted `Tool::effective_capabilities()` (0.7.0 → 0.7.1)
- tau-runtime-core: `DynTool` forwarding; `capability_verdict` +
  `render_clamped_to` helpers wired at both ToolCall emit sites;
  producer test mirrors the M1.5 drop test
- tau-mcp-tokio: `McpBackedTool` carries `Option<Vec<Capability>>`
- tau-cli: `tool_effective_capabilities` per-server-tool meet in
  `setup_mcp_runtime` (+ 4 unit tests beside the existing plan tests)
- docs: `clamp` badge row in `reference/tau-trace.md`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Enroll auto-merge and babysit**

```bash
gh pr merge <N> --squash --auto
```

Then monitor CI (`gh pr checks <N> --watch`); if the PR goes `BEHIND` main, `gh pr update-branch <N>`. If the `tau-ports ABI (cargo-semver-checks)` job rejects 0.7.1, bump to 0.8.0 in `crates/tau-ports/Cargo.toml` + root `Cargo.toml` pin, refresh `Cargo.lock`, amend nothing — add a `fix(ports): bump to 0.8.0 per ABI gate` commit, push. If auto-merge drops after a flake re-run, re-enroll with the same bare `gh pr merge <N> --auto`.
