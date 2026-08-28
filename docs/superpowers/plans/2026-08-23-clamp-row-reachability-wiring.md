# Clamp-Row Reachability Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a governed, host-clamped MCP tool render an amber `clamp:<to>` row in `tau run --bundle --tui` and `tau trace`, closing issue #631.

**Architecture:** Three seams, all additive. (1) The kernel's `capability_verdict` checks `effective_capabilities()` *before* its empty-required early return, so narrowed authority is visible independent of gate participation. (2) `ToolDispatcher` gains two defaulted accessors — `tool_effective_capabilities()` and `trace_sink()` — which the IR interpreter consumes in `prepare_agent_run` to forward clamped authority onto `DispatcherTool` and to build a synthetic `RunState` carrying a trace sink. (3) `tau-cli`'s `ForwardingDispatcher` implements both from data it already holds, and `run_via_ir` mints one run id, attaches the JSONL writer, and joins the TUI. A pre-existing `tau trace` ingestion bug (writer emits an envelope, reader expects a bare event) is fixed in the same PR because the end-to-end DoD depends on it.

**Tech Stack:** Rust 2021, tokio, `tau-runtime-core` (no_std kernel + interpreter), `tau-cli`, `tau-trace`, `tau-runtime-tokio`, ratatui TUI, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-21-execution-trace-tui-design.md` §13 (with §12 as background). Read §13 before starting — every task cites a subsection.

## Global Constraints

- **CARGO RULES (project `CLAUDE.md`) are mandatory.** Every cargo invocation:
  `timeout <n> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`.
  Timeouts: test 300s, build/check 180s, clippy 240s, fmt 30s. Never `--workspace`, never bare `cargo`, always `-p <crate>`.
- **Use `cargo nextest run` for tests** (matches CI), `cargo test --doc` for doctests.
- **`tau-ports` stays at 0.7.1.** No task in this plan edits `crates/tau-ports/`. If you believe you need to, stop and escalate — it triggers the `tau-ports ABI (cargo-semver-checks)` job and a workspace-root pin.
- **`forbid(unsafe_code)`** across the workspace. Workspace lints treat warnings as errors in CI.
- **The kernel (`tau-runtime-core`) is `no_std` + `alloc`.** Use `alloc::vec::Vec`, `alloc::string::String`, `alloc::sync::Arc`, `alloc::format!`. Never `std::` in `src/` (tests may use std).
- **Issue #581 pinned contract:** `DispatcherTool` must NEVER override `Tool::capabilities()`. Only `effective_capabilities()` is forwarded. `crates/tau-runtime-core/tests/ir_dispatch_gate_inert.rs` must stay green unmodified except for the additive test in Task 5.
- **Commits:** `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."`. Conventional commits, imperative, scoped. Backticks in `-m` get shell-substituted — use `-F` with a heredoc file if the message needs them.
- **Branch:** `clamp-row-wiring-631` (already checked out). Never commit to `main`.

---

### Task 1: Kernel verdict decoupling (spec §13.1)

Make a clamp visible even when the tool does not participate in the kernel grant gate. This is the change that unblocks every IR-path clamp, because IR tools always present `required = []`.

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs:1906-1919` (the `capability_verdict` helper)
- Test: `crates/tau-runtime-core/src/stream.rs` (in-file `mod tests`, next to `capability_verdict_empty_required_is_none_default_is_allow` at ~line 4092)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `fn capability_verdict(tool: &dyn DynTool, required: &[Capability]) -> Option<tau_ports::CapabilityVerdict>` — same signature as today, new precedence. Tasks 3, 5 and 9 depend on the new behavior: a tool with `Some(effective)` and `required == []` now yields `Some(Clamp { to })` instead of `None`.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/tau-runtime-core/src/stream.rs`, immediately after the existing `capability_verdict_empty_required_is_none_default_is_allow` test (~line 4110):

```rust
    #[test]
    fn capability_verdict_clamps_even_when_required_is_empty() {
        // Spec §13.1: IR-authored tools present `required = []` at the gate
        // (issue #581's pinned contract), but a clamp on one of those must
        // still surface — narrowed authority is a property of the object,
        // not of gate participation. Before §13.1 this returned None.
        use tau_ports::fixtures::{make_tool_spec, MockTool};
        let effective = test_cap("[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n");
        let tool = ClampedTool {
            inner: MockTool::new(
                "narrowed",
                make_tool_spec("narrowed".into(), "narrowed".into(), Value::Null),
            ),
            // Declared caps are irrelevant here: the gate sees `required`,
            // which the caller passes as `&[]` for an IR-wrapped tool.
            required: vec![],
            effective: vec![effective],
        };
        let tool: Arc<dyn DynTool> = Arc::new(tool);

        assert_eq!(
            super::capability_verdict(&*tool, &[]),
            Some(tau_ports::CapabilityVerdict::Clamp {
                to: "api.weather.com".into()
            }),
            "a narrowed tool must report Clamp even with an empty required set"
        );
    }

    #[test]
    fn capability_verdict_clamps_when_required_is_non_empty() {
        // The pre-§13.1 behavior for gated tools is unchanged.
        use tau_ports::fixtures::{make_tool_spec, MockTool};
        let declared = test_cap(
            "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\", \"evil.example\"]\n",
        );
        let effective = test_cap("[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n");
        let tool = ClampedTool {
            inner: MockTool::new(
                "narrowed",
                make_tool_spec("narrowed".into(), "narrowed".into(), Value::Null),
            ),
            required: vec![declared.clone()],
            effective: vec![effective],
        };
        let tool: Arc<dyn DynTool> = Arc::new(tool);

        assert_eq!(
            super::capability_verdict(&*tool, &[declared]),
            Some(tau_ports::CapabilityVerdict::Clamp {
                to: "api.weather.com".into()
            })
        );
    }
```

- [ ] **Step 2: Run the tests to verify the first one fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --lib capability_verdict
```

Expected: `capability_verdict_clamps_even_when_required_is_empty` FAILS with `assertion failed: left: None, right: Some(Clamp { to: "api.weather.com" })`. The other two (`..._clamps_when_required_is_non_empty`, `..._empty_required_is_none_default_is_allow`) PASS.

- [ ] **Step 3: Reorder the helper**

Replace the body of `capability_verdict` at `crates/tau-runtime-core/src/stream.rs:1906-1919` with:

```rust
fn capability_verdict(
    tool: &dyn DynTool,
    required: &[Capability],
) -> Option<tau_ports::CapabilityVerdict> {
    // Spec §13.1. Narrowed authority is a property of the object (§12.1's
    // ocap framing), so it surfaces whether or not the in-kernel grant gate
    // looked at this tool. IR-authored tools always present `required = []`
    // (issue #581's pinned contract) yet can still be meet-clamped at MCP
    // open time, so this check MUST precede the empty-required early return.
    if let Some(eff) = tool.effective_capabilities() {
        return Some(tau_ports::CapabilityVerdict::Clamp {
            to: render_clamped_to(eff),
        });
    }
    // Un-gated AND un-narrowed: no verdict. Matches the port contract
    // ("`None` for un-gated tools", tau-ports orchestration.rs).
    if required.is_empty() {
        return None;
    }
    Some(tau_ports::CapabilityVerdict::Allow)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --lib capability_verdict
```

Expected: all 4 `capability_verdict*` tests PASS.

- [ ] **Step 5: Run the whole crate to prove no regression**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core
```

Expected: PASS. Pay attention to `clamped_tool_emits_clamp_trace_event`, `schema_invalid_call_from_clamped_tool_emits_clamp_trace_event`, `dispatch_emits_toolcall_trace_event_with_verdict` — all must stay green.

- [ ] **Step 6: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets
git add crates/tau-runtime-core/src/stream.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(trace): surface clamp verdicts for un-gated tools (#631)"
```

---

### Task 2: `ToolDispatcher::tool_effective_capabilities` + `DispatcherTool` forwarding (spec §13.2)

Give the IR interpreter a way to learn a tool's meet-clamped authority, and cache it on the wrapper. `Tool::capabilities()` stays un-overridden — the #581 contract is untouched.

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` (add trait method after `checkpointing`, ~line 165)
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs:101-115` (struct field), `:117-131` (doc comment + `Tool` impl), `:482-489` (construction)
- Test: `crates/tau-runtime-core/src/interpreter/agent_loop.rs` (new `mod tests` entries) — or the existing test module if one is present in that file

**Interfaces:**
- Consumes: Task 1's `capability_verdict` precedence (an IR tool with effective caps now yields `Clamp`).
- Produces:
  - `ToolDispatcher::tool_effective_capabilities(&self, tool_id: &ToolId) -> Option<alloc::vec::Vec<tau_domain::Capability>>` — defaulted to `None`. Task 6 implements it on `ForwardingDispatcher`.
  - `DispatcherTool` gains a private field `effective_capabilities: Option<alloc::vec::Vec<tau_domain::Capability>>` and overrides `Tool::effective_capabilities(&self) -> Option<&[Capability]>`.

- [ ] **Step 1: Write the failing test**

Append to `crates/tau-runtime-core/src/interpreter/agent_loop.rs`. If the file has no `mod tests` block, add one at the end of the file:

```rust
#[cfg(all(test, feature = "test-fixtures"))]
mod effective_capability_tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use serde_json::Value;

    /// Build a Capability from its canonical TOML form (variants are
    /// `#[non_exhaustive]` outside tau-domain). Same pattern as stream.rs.
    fn test_cap(toml_str: &str) -> tau_domain::Capability {
        #[derive(serde::Deserialize)]
        struct CapWrapper {
            cap: tau_domain::Capability,
        }
        toml::from_str::<CapWrapper>(toml_str).unwrap().cap
    }

    /// A dispatcher that reports a clamped authority for exactly one tool id.
    struct ClampReportingDispatcher {
        clamped_id: ToolId,
        effective: alloc::vec::Vec<tau_domain::Capability>,
    }

    impl ToolDispatcher for ClampReportingDispatcher {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> core::pin::Pin<
            alloc::boxed::Box<
                dyn core::future::Future<
                        Output = Result<
                            crate::interpreter::tool_dispatch::ToolInvocationResult,
                            RuntimeError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            alloc::boxed::Box::pin(async {
                Ok(crate::interpreter::tool_dispatch::ToolInvocationResult {
                    body: Some(Value::String("ok".into())),
                    error: None,
                })
            })
        }

        fn llm_backend_for(
            &self,
            _backend: &str,
        ) -> Result<Arc<dyn crate::builder::DynLlmBackend>, RuntimeError> {
            Err(RuntimeError::Internal {
                message: "not used in this test".into(),
            })
        }

        fn tool_effective_capabilities(
            &self,
            tool_id: &ToolId,
        ) -> Option<alloc::vec::Vec<tau_domain::Capability>> {
            if tool_id == &self.clamped_id {
                Some(self.effective.clone())
            } else {
                None
            }
        }
    }

    #[test]
    fn dispatcher_tool_forwards_effective_capabilities_but_not_declared() {
        let clamped_id = ToolId("weather.get_forecast".into());
        let effective = vec![test_cap(
            "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n",
        )];
        let dispatcher = Arc::new(ClampReportingDispatcher {
            clamped_id: clamped_id.clone(),
            effective: effective.clone(),
        });

        let module = Arc::new(tau_ir::IrModule {
            ir_format: tau_ir::module::IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target: tau_ports::target::registry::list_available()
                .next()
                .expect("at least one available target")
                .triple,
            workflow: tau_ir::module::Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: tau_ir::capability::CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        });

        let tool = DispatcherTool {
            tool_name: "get_forecast".into(),
            tool_id: clamped_id.clone(),
            spec: make_tool_spec("get_forecast", "", &Value::Null),
            module: module.clone(),
            tool_impl: tau_ir::ToolImpl::Mcp {
                server: "weather".into(),
                tool: "get_forecast".into(),
            },
            dispatcher: dispatcher.clone(),
            effective_capabilities: dispatcher.tool_effective_capabilities(&clamped_id),
        };

        // Issue #581: declared caps are NEVER forwarded — the gate must keep
        // seeing an empty required set for IR-authored tools.
        assert!(
            tau_ports::Tool::capabilities(&tool).is_empty(),
            "DispatcherTool must not forward declared capabilities (#581)"
        );
        // Spec §13.2: effective (meet-clamped) authority IS forwarded.
        assert_eq!(
            tau_ports::Tool::effective_capabilities(&tool),
            Some(effective.as_slice()),
        );
    }

    #[test]
    fn dispatcher_tool_reports_no_effective_capabilities_when_unclamped() {
        let dispatcher = Arc::new(ClampReportingDispatcher {
            clamped_id: ToolId("other".into()),
            effective: vec![test_cap(
                "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n",
            )],
        });
        let tool_id = ToolId("plain".into());

        let module = Arc::new(tau_ir::IrModule {
            ir_format: tau_ir::module::IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target: tau_ports::target::registry::list_available()
                .next()
                .expect("at least one available target")
                .triple,
            workflow: tau_ir::module::Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: tau_ir::capability::CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        });

        let tool = DispatcherTool {
            tool_name: "plain".into(),
            tool_id: tool_id.clone(),
            spec: make_tool_spec("plain", "", &Value::Null),
            module,
            tool_impl: tau_ir::ToolImpl::Native {
                fn_ref: tau_ir::tool_impl::NativeFnRef {
                    name: "plain".into(),
                },
                content_hash: [0u8; 32],
            },
            dispatcher: dispatcher.clone(),
            effective_capabilities: dispatcher.tool_effective_capabilities(&tool_id),
        };

        assert_eq!(tau_ports::Tool::effective_capabilities(&tool), None);
    }
}
```

Note: if `make_tool_spec`'s signature in this file differs from `(&str, &str, &Value)`, match the call at `agent_loop.rs:476-480` exactly.

- [ ] **Step 2: Run the test to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures effective_capability
```

Expected: COMPILE ERROR — `struct DispatcherTool has no field named effective_capabilities`, and `no method named tool_effective_capabilities`.

- [ ] **Step 3: Add the trait method**

In `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs`, add inside `trait ToolDispatcher`, after `checkpointing` (~line 165):

```rust
    /// The meet-clamped authority a tool actually runs under, when narrower
    /// than its declared capabilities (execution-trace TUI spec §12/§13.2).
    ///
    /// Returning `None` (the default) means "not narrowed, or this
    /// dispatcher does not track authority". `tau-cli`'s
    /// `ForwardingDispatcher` answers from the `Arc<dyn DynTool>` it holds
    /// for each MCP-backed tool, whose effective set was computed at MCP
    /// open time by `setup_mcp_runtime`.
    ///
    /// This is **observability only**. The value is forwarded onto the
    /// interpreter's `DispatcherTool` wrapper via
    /// `Tool::effective_capabilities()` so the kernel can emit a
    /// `CapabilityVerdict::Clamp` on the call's `ToolCall` trace event.
    /// Declared capabilities are deliberately NOT forwarded — see issue
    /// #581 and `tests/ir_dispatch_gate_inert.rs`.
    fn tool_effective_capabilities(
        &self,
        tool_id: &ToolId,
    ) -> Option<alloc::vec::Vec<tau_domain::Capability>> {
        let _ = tool_id;
        None
    }
```

- [ ] **Step 4: Add the field, the override, and the construction**

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs`, add the field to the struct (after `dispatcher`, ~line 114):

```rust
    /// Meet-clamped authority for this tool, when narrower than declared
    /// (spec §13.2). Sourced once at construction from
    /// [`ToolDispatcher::tool_effective_capabilities`]. `None` = not
    /// narrowed. Surfaced through `Tool::effective_capabilities()` so the
    /// kernel emits a `Clamp` verdict; NEVER through `capabilities()`.
    effective_capabilities: Option<alloc::vec::Vec<tau_domain::Capability>>,
```

Extend the existing `// Tool::capabilities() is intentionally NOT overridden (issue #581)` comment block above `impl ... Tool for DispatcherTool` (~line 125) by appending one paragraph before `// Contract pinned by ...`:

```rust
// `Tool::effective_capabilities()` IS overridden (spec §13.2): it carries
// the MCP open-time meet-clamp for observability only. It never reaches the
// grant gate — `capability_verdict` reads it purely to label the ToolCall
// trace row — so the #581 contract below is unaffected.
```

Add the override inside `impl ... Tool for DispatcherTool`, immediately after `fn schema` (~line 138):

```rust
    fn effective_capabilities(&self) -> Option<&[tau_domain::Capability]> {
        self.effective_capabilities.as_deref()
    }
```

Update the construction at `agent_loop.rs:482-489`:

```rust
        builder = builder.with_tool(DispatcherTool {
            tool_name: ir_tool.spec.name.clone(),
            tool_id: tool_id.clone(),
            spec,
            module: module.clone(),
            tool_impl: ir_tool.impl_.clone(),
            dispatcher: dispatcher.clone(),
            // Spec §13.2: ask the host dispatcher once, at wrapper
            // construction, whether this tool's authority was narrowed.
            effective_capabilities: dispatcher.tool_effective_capabilities(tool_id),
        });
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures
```

Expected: both new tests PASS; `ir_declared_caps_do_not_gate_root_dispatch` still PASSES.

- [ ] **Step 6: Verify the no_std kernel still builds**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo check -p tau-runtime-core --no-default-features
```

Expected: PASS (no `std::` leaked into the new code).

- [ ] **Step 7: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets
git add crates/tau-runtime-core/src/interpreter/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(interpreter): forward meet-clamped tool authority to the kernel (#631)"
```

---

### Task 3: `ToolDispatcher::trace_sink` + synthetic `RunState` (spec §13.3)

Give IR-interpreter runs an orchestration trace sink, so the three guarded `ToolCall` emit sites in `stream.rs` fire. Combined with Tasks 1 and 2, this is the first point where an IR run can produce a `Clamp` row.

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` (add `TraceSinkConfig` struct + `trace_sink` method)
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs` (in `prepare_agent_run`, after the durable block at ~line 629)
- Test: `crates/tau-runtime-core/tests/ir_trace_sink.rs` (new integration test)

**Interfaces:**
- Consumes: Task 1's verdict precedence; Task 2's `tool_effective_capabilities` (the integration test exercises both).
- Produces:
  - `pub struct TraceSinkConfig { pub run_id: tau_ports::RunId, pub subscribers: alloc::vec::Vec<Arc<dyn crate::orchestration::trace::TraceSubscriber>> }` in `interpreter::tool_dispatch`.
  - `ToolDispatcher::trace_sink(&self) -> Option<TraceSinkConfig>`, defaulted to `None`. Task 6 implements it on `ForwardingDispatcher`.

- [ ] **Step 1: Write the failing test**

Create `crates/tau-runtime-core/tests/ir_trace_sink.rs`:

```rust
//! Spec §13.3 + §13.1 + §13.2: an IR-interpreter run with a trace sink
//! emits `ToolCall` trace events, and a meet-clamped tool's row carries
//! `CapabilityVerdict::Clamp` even though the IR gate sees `required = []`
//! (issue #581).
#![cfg(feature = "test-fixtures")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt as _;
use serde_json::Value;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityRequirements;
use tau_ir::ids::{AgentId, ToolId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Tool, ToolSpec};
use tau_ir::tool_impl::{NativeFnRef, ToolImpl};
use tau_ports::{
    CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError, TraceEvent,
};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir_streaming;
use tau_runtime_core::interpreter::tool_dispatch::{
    ToolDispatcher, ToolInvocationResult, TraceSinkConfig,
};
use tau_runtime_core::orchestration::trace::TraceSubscriber;
use tau_runtime_core::stream::RunEvent;

fn resp(json: serde_json::Value) -> CompletionResponse {
    serde_json::from_value(json).expect("CompletionResponse deserializes")
}

fn test_cap(toml_str: &str) -> tau_domain::Capability {
    #[derive(serde::Deserialize)]
    struct CapWrapper {
        cap: tau_domain::Capability,
    }
    toml::from_str::<CapWrapper>(toml_str).unwrap().cap
}

struct Scripted {
    queue: Mutex<Vec<CompletionResponse>>,
}

impl LlmBackend for Scripted {
    fn name(&self) -> &str {
        "mock-llm"
    }
    async fn complete(&self, _r: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.queue.lock().unwrap().remove(0))
    }
    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        Ok(tau_ports::batch_to_stream(
            LlmBackend::complete(self, req).await?,
        ))
    }
}

/// Collects every trace event the kernel emits.
struct Collector(Mutex<Vec<TraceEvent>>);

impl TraceSubscriber for Collector {
    fn emit(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// A dispatcher that supplies both a trace sink and a clamped authority.
struct SinkDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    collector: Arc<Collector>,
    clamped_id: ToolId,
    effective: Vec<tau_domain::Capability>,
}

impl ToolDispatcher for SinkDispatcher {
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>,
    > {
        Box::pin(async {
            Ok(ToolInvocationResult {
                body: Some(Value::String("ok".into())),
                error: None,
            })
        })
    }

    fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn trace_sink(&self) -> Option<TraceSinkConfig> {
        Some(TraceSinkConfig {
            run_id: "run-ir-clamp".into(),
            subscribers: vec![Arc::clone(&self.collector) as Arc<dyn TraceSubscriber>],
        })
    }

    fn tool_effective_capabilities(
        &self,
        tool_id: &ToolId,
    ) -> Option<Vec<tau_domain::Capability>> {
        (tool_id == &self.clamped_id).then(|| self.effective.clone())
    }
}

/// Single agent, single `net.http`-declaring tool (mirrors
/// `ir_dispatch_gate_inert.rs`'s fixture).
fn module_with_net_tool() -> (IrModule, AgentId) {
    let entry = AgentId("a".into());
    let net_any = test_cap("[cap]\nkind=\"net.http\"\nhosts=\"any\"\n");

    let mut tools = BTreeMap::new();
    tools.insert(
        ToolId("fetch".into()),
        Tool {
            id: ToolId("fetch".into()),
            impl_: ToolImpl::Native {
                fn_ref: NativeFnRef {
                    name: "fetch".into(),
                },
                content_hash: [1u8; 32],
            },
            capabilities: CapabilityRequirements {
                declared: vec![net_any],
            },
            spec: ToolSpec {
                name: "fetch".into(),
                description: String::new(),
                input_schema: Value::Null,
            },
        },
    );

    let mut agents = BTreeMap::new();
    agents.insert(
        entry.clone(),
        Agent {
            id: entry.clone(),
            prompt: tau_ir::prompt::PromptSource::Inline(String::new()),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: "mock-llm".into(),
                model_id: "m".into(),
            },
            tool_refs: vec![ToolId("fetch".into())],
            context: None,
            budget: AgentBudget {
                max_turns: Some(3),
                max_tokens: None,
            },
            produces: vec![],
            output_schema: None,
            durable: None,
        },
    );

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple;

    let module = IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target,
        workflow: Workflow {
            agents,
            tools,
            steps: BTreeMap::new(),
            edges: Vec::new(),
            capability_table: tau_ir::capability::CapabilityTable(BTreeMap::new()),
            pipeline: None,
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    };

    (module, entry)
}

fn scripted_backend() -> Arc<dyn DynLlmBackend> {
    Arc::new(Scripted {
        queue: Mutex::new(vec![
            resp(serde_json::json!({
                "text":"","tool_uses":[{"id":"t1","name":"fetch","input":{}}],
                "stop_reason":"ToolUse","usage":null
            })),
            resp(serde_json::json!({
                "text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null
            })),
        ]),
    })
}

#[tokio::test]
async fn ir_run_with_sink_emits_clamp_tool_call_row() {
    let (module, entry) = module_with_net_tool();
    let collector = Arc::new(Collector(Mutex::new(Vec::new())));
    let dispatcher = Arc::new(SinkDispatcher {
        backend: scripted_backend(),
        collector: collector.clone(),
        clamped_id: ToolId("fetch".into()),
        effective: vec![test_cap(
            "[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n",
        )],
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let _events: Vec<RunEvent> = Box::pin(stream).collect().await;

    let events = collector.0.lock().unwrap().clone();
    let tool_call = events
        .iter()
        .find_map(|e| match &e.kind {
            tau_ports::TraceEventKind::ToolCall {
                tool_name,
                capability,
                ..
            } => Some((tool_name.clone(), capability.clone())),
            _ => None,
        })
        .expect("an IR run with a trace sink must emit a ToolCall trace event");

    assert_eq!(tool_call.0, "fetch");
    assert_eq!(
        tool_call.1,
        Some(tau_ports::CapabilityVerdict::Clamp {
            to: "api.weather.com".into()
        }),
        "the meet-clamped IR tool must render an amber clamp row"
    );
    assert!(
        events.iter().all(|e| e.run_id == "run-ir-clamp"),
        "every event must carry the sink's run id"
    );
}

#[tokio::test]
async fn ir_run_without_sink_emits_nothing() {
    // Regression guard: dispatchers that don't override `trace_sink` (the
    // wasm guest, `tau dev`, conformance) must behave exactly as before.
    struct NoSink {
        backend: Arc<dyn DynLlmBackend>,
    }
    impl ToolDispatcher for NoSink {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Ok(ToolInvocationResult {
                    body: Some(Value::String("ok".into())),
                    error: None,
                })
            })
        }
        fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
            Ok(self.backend.clone())
        }
    }

    let (module, entry) = module_with_net_tool();
    let dispatcher = Arc::new(NoSink {
        backend: scripted_backend(),
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;

    // The run still completes; there is simply no trace sink to emit into.
    assert!(matches!(events.last(), Some(RunEvent::RunCompleted { .. })));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures --test ir_trace_sink
```

Expected: COMPILE ERROR — `no TraceSinkConfig in interpreter::tool_dispatch`.

- [ ] **Step 3: Add `TraceSinkConfig` + the trait method**

In `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs`, add above `trait ToolDispatcher` (next to `DurableHandles`, ~line 42):

```rust
/// Trace sink supplied by the host shell for an IR-interpreter run
/// (execution-trace TUI spec §13.3).
///
/// `spawn_root_agent` builds a full `RunState` for multi-agent runs; the
/// interpreter has no equivalent, so the host passes the two ingredients
/// the kernel's `TraceEvent` emit sites actually need — a run id and the
/// subscribers to fan out to — and `prepare_agent_run` assembles a
/// synthetic, orchestration-inert `RunState` around them.
///
/// Both fields are `Send + Sync`, so this crosses the dispatcher's
/// `D: Send + Sync` bound (an `Arc<RefCell<RunState>>` could not).
pub struct TraceSinkConfig {
    /// Run id stamped on every emitted `TraceEvent`; also the
    /// `.tau/runs/<run_id>.jsonl` filename the host writes.
    pub run_id: tau_ports::RunId,
    /// Sinks to fan events out to (JSONL writer, live TUI channel, …).
    pub subscribers: alloc::vec::Vec<Arc<dyn crate::orchestration::trace::TraceSubscriber>>,
}
```

Add inside `trait ToolDispatcher`, after `tool_effective_capabilities`:

```rust
    /// Optional trace sink for this run (spec §13.3).
    ///
    /// Returning `Some` makes the kernel's `TraceEvent` emit sites live for
    /// an IR-interpreter run: `prepare_agent_run` builds a synthetic
    /// `RunState` from it and attaches it as `RunOptions::orchestration_state`.
    /// Returning `None` (the default) preserves today's behavior — no trace
    /// emission at all — which is what the wasm guest, `tau dev` and the
    /// conformance runner rely on.
    ///
    /// The synthetic state is orchestration-*inert*: it carries a default
    /// (unlimited) budget and no `orchestration_runtime`, so the budget
    /// watchdog no-ops and the virtual-tool intercept stays disabled
    /// (§13.4).
    fn trace_sink(&self) -> Option<TraceSinkConfig> {
        None
    }
```

- [ ] **Step 4: Assemble the synthetic `RunState` in `prepare_agent_run`**

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs`, insert after the durable block (after the `if agent.durable.is_some() { ... }` closing brace at ~line 629, before "7. Split initial_messages"):

```rust
    // 6d. Spec §13.3: attach the host's trace sink, if any, so the kernel's
    //     `TraceEvent` emit sites fire for this IR run. The `RunState` is
    //     synthetic and orchestration-inert — a default (unlimited) budget
    //     so the BudgetWatchdog no-ops, an empty task list, and NO
    //     `orchestration_runtime`, which keeps the virtual-tool intercept
    //     disabled (§13.4). It exists purely to carry the run id and the
    //     subscriber fan-out into the emit sites. Dispatchers that don't
    //     override `trace_sink` (wasm guest, `tau dev`, conformance) return
    //     `None` and this is a no-op.
    if let Some(sink) = dispatcher.trace_sink() {
        let clock = run_options
            .clock
            .as_ref()
            .expect("clock is present (checked above)");
        let mut state = crate::orchestration::run_state::RunState::new(
            sink.run_id,
            agent.id.0.clone(),
            tau_ports::RunBudget::default(),
            crate::ids::now_utc(clock),
        );
        for subscriber in sink.subscribers {
            state.trace.add_subscriber(subscriber);
        }
        // `RunState` is `!Send` (interior `RefCell` sharing); it never
        // crosses a spawn boundary — the run future stays on the caller's
        // task. See §13.3's `!Send` discipline note.
        #[allow(clippy::arc_with_non_send_sync)]
        {
            run_options.orchestration_state =
                Some(alloc::sync::Arc::new(core::cell::RefCell::new(state)));
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures --test ir_trace_sink
```

Expected: both tests PASS. If `ir_run_with_sink_emits_clamp_tool_call_row` reports `capability: None`, Task 1's reorder is missing; if it finds no ToolCall event at all, the sink wiring in Step 4 is not reached.

- [ ] **Step 6: Full crate + no_std check**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo check -p tau-runtime-core --no-default-features
```

Expected: PASS. Note in your report whether any pipeline/subflow test changed behavior — pipelines build one `RunState` per agent step.

- [ ] **Step 7: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets
git add crates/tau-runtime-core/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(interpreter): attach a host trace sink to IR runs (#631)"
```

---

### Task 4: Virtual-tool intercept hardening (spec §13.4)

Now that IR runs can carry `orchestration_state`, an MCP tool whose IR id happens to be `task.create` (entry `task`, server tool `create`) would be hijacked by the in-kernel orchestration intercept. Require `orchestration_runtime` as well.

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs:745-746` (the intercept gate)
- Test: `crates/tau-runtime-core/tests/ir_trace_sink.rs` (extend Task 3's file)

**Interfaces:**
- Consumes: Task 3's `TraceSinkConfig` + `SinkDispatcher` test fixture.
- Produces: no API change. Behavioral contract: the virtual-tool intercept requires BOTH `orchestration_state` and `orchestration_runtime`.

- [ ] **Step 1: Write the failing test**

Append to `crates/tau-runtime-core/tests/ir_trace_sink.rs`. It reuses the file's existing helpers but needs a module whose tool is named `task.create`:

```rust
/// Spec §13.4: attaching a trace sink to an IR run must NOT activate the
/// in-kernel orchestration virtual-tool intercept. An MCP tool whose IR id
/// collides with a virtual name (`[tools.task]` + server tool `create`)
/// must still reach the dispatcher.
#[tokio::test]
async fn ir_run_with_sink_does_not_hijack_virtual_named_tools() {
    let entry = AgentId("a".into());

    let mut tools = BTreeMap::new();
    tools.insert(
        ToolId("task.create".into()),
        Tool {
            id: ToolId("task.create".into()),
            impl_: ToolImpl::Mcp {
                server: "task".into(),
                tool: "create".into(),
            },
            capabilities: CapabilityRequirements { declared: vec![] },
            spec: ToolSpec {
                name: "task.create".into(),
                description: String::new(),
                input_schema: Value::Null,
            },
        },
    );

    let mut agents = BTreeMap::new();
    agents.insert(
        entry.clone(),
        Agent {
            id: entry.clone(),
            prompt: tau_ir::prompt::PromptSource::Inline(String::new()),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: "mock-llm".into(),
                model_id: "m".into(),
            },
            tool_refs: vec![ToolId("task.create".into())],
            context: None,
            budget: AgentBudget {
                max_turns: Some(3),
                max_tokens: None,
            },
            produces: vec![],
            output_schema: None,
            durable: None,
        },
    );

    let target = tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple;
    let module = IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target,
        workflow: Workflow {
            agents,
            tools,
            steps: BTreeMap::new(),
            edges: Vec::new(),
            capability_table: tau_ir::capability::CapabilityTable(BTreeMap::new()),
            pipeline: None,
            checks: BTreeMap::new(),
        },
        triggers: Vec::new(),
    };

    /// Records every tool id the dispatcher is actually asked to invoke.
    struct RecordingSink {
        backend: Arc<dyn DynLlmBackend>,
        collector: Arc<Collector>,
        seen: Arc<Mutex<Vec<String>>>,
    }
    impl ToolDispatcher for RecordingSink {
        fn invoke<'a>(
            &'a self,
            tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            self.seen.lock().unwrap().push(tool_id.0.clone());
            Box::pin(async {
                Ok(ToolInvocationResult {
                    body: Some(Value::String("ok".into())),
                    error: None,
                })
            })
        }
        fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
            Ok(self.backend.clone())
        }
        fn trace_sink(&self) -> Option<TraceSinkConfig> {
            Some(TraceSinkConfig {
                run_id: "run-virtual-collision".into(),
                subscribers: vec![Arc::clone(&self.collector) as Arc<dyn TraceSubscriber>],
            })
        }
    }

    let collector = Arc::new(Collector(Mutex::new(Vec::new())));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let backend: Arc<dyn DynLlmBackend> = Arc::new(Scripted {
        queue: Mutex::new(vec![
            resp(serde_json::json!({
                "text":"","tool_uses":[{"id":"t1","name":"task.create","input":{}}],
                "stop_reason":"ToolUse","usage":null
            })),
            resp(serde_json::json!({
                "text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null
            })),
        ]),
    });
    let dispatcher = Arc::new(RecordingSink {
        backend,
        collector: collector.clone(),
        seen: seen.clone(),
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let _events: Vec<RunEvent> = Box::pin(stream).collect().await;

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["task.create"],
        "a virtual-named MCP tool must reach the dispatcher, not the kernel intercept"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures --test ir_trace_sink
```

Expected: `ir_run_with_sink_does_not_hijack_virtual_named_tools` FAILS — `seen` is empty, because the kernel's orchestration intercept handled `task.create` instead of forwarding it.

- [ ] **Step 3: Harden the gate**

In `crates/tau-runtime-core/src/stream.rs`, change the intercept gate at line ~745 from:

```rust
                if let Some(state_arc) = options.orchestration_state.as_ref() {
                    if crate::orchestration::is_virtual(&tool_use.name) {
```

to:

```rust
                // Spec §13.4: the intercept requires BOTH pieces of
                // orchestration context. `spawn_root_agent_inner` sets state
                // AND runtime; IR-interpreter runs (§13.3) set state ONLY,
                // to carry a trace sink. Gating on state alone would let an
                // MCP tool whose IR id collides with a virtual name (e.g.
                // `[tools.task]` + server tool `create` => `task.create`) be
                // hijacked in-kernel on a bundle run.
                if let (Some(state_arc), true) = (
                    options.orchestration_state.as_ref(),
                    options.orchestration_runtime.is_some(),
                ) {
                    if crate::orchestration::is_virtual(&tool_use.name) {
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures --test ir_trace_sink
```

Expected: all tests in the file PASS.

- [ ] **Step 5: Prove the multi-agent path is unaffected**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-orch \
  cargo nextest run -p tau-runtime-tokio
```

Expected: PASS, including `skill_spawn_e2e` (every virtual-tool test drives `spawn_root_agent_with_scope`, which sets both fields).

- [ ] **Step 6: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets
git add crates/tau-runtime-core/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "fix(kernel): require orchestration runtime for virtual-tool intercept (#631)"
```

---

### Task 5: #581 sibling pin test (spec §13.2, §13.6)

Pin the invariant that makes this whole design legal: forwarding *effective* capabilities does not re-activate the dispatch gate for IR tools.

**Files:**
- Modify: `crates/tau-runtime-core/tests/ir_dispatch_gate_inert.rs` (add a test + a paragraph to the module doc; leave the existing test byte-identical)

**Interfaces:**
- Consumes: Task 2's `tool_effective_capabilities`, Task 3's `TraceSinkConfig`, Task 1's verdict precedence.
- Produces: no API. A contract pin future changes must not break.

- [ ] **Step 1: Write the failing test**

In `crates/tau-runtime-core/tests/ir_dispatch_gate_inert.rs`, append this paragraph to the module doc (after the existing paragraph ending `...break every capable bundle run.`, before `#![cfg(feature = "test-fixtures")]`):

```rust
//!
//! Execution-trace TUI spec §13.2 layers observability on top of this
//! contract WITHOUT weakening it: `DispatcherTool` forwards the host's
//! meet-clamped `effective_capabilities()` so a clamped MCP tool renders an
//! amber `clamp:<to>` waterfall row, while `capabilities()` stays
//! un-overridden so `required` at the gate remains `[]`. The second test
//! below pins exactly that split.
```

Then append the test at the end of the file:

```rust
/// Spec §13.2 + §13.6: forwarding *effective* capabilities must not
/// re-activate the dispatch gate. The tool still reaches the dispatcher
/// un-gated (issue #581), and its ToolCall row carries `Clamp`.
#[tokio::test]
async fn forwarded_effective_caps_do_not_gate_but_do_label_the_row() {
    use std::sync::Mutex as StdMutex;
    use tau_ports::TraceEvent;
    use tau_runtime_core::interpreter::tool_dispatch::TraceSinkConfig;
    use tau_runtime_core::orchestration::trace::TraceSubscriber;

    struct Collector(StdMutex<Vec<TraceEvent>>);
    impl TraceSubscriber for Collector {
        fn emit(&self, event: TraceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    /// `Recording`, plus a trace sink and a clamped authority for `fetch`.
    struct RecordingClamped {
        seen: Arc<Mutex<Vec<String>>>,
        backend: Arc<dyn DynLlmBackend>,
        collector: Arc<Collector>,
    }

    impl ToolDispatcher for RecordingClamped {
        fn invoke<'a>(
            &'a self,
            tool_id: &'a ToolId,
            _args: &'a Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>>
                    + Send
                    + 'a,
            >,
        > {
            self.seen.lock().unwrap().push(tool_id.0.clone());
            Box::pin(async {
                Ok(ToolInvocationResult {
                    body: Some(Value::String("ok".into())),
                    error: None,
                })
            })
        }

        fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
            Ok(self.backend.clone())
        }

        fn trace_sink(&self) -> Option<TraceSinkConfig> {
            Some(TraceSinkConfig {
                run_id: "run-581-sibling".into(),
                subscribers: vec![Arc::clone(&self.collector) as Arc<dyn TraceSubscriber>],
            })
        }

        fn tool_effective_capabilities(
            &self,
            tool_id: &ToolId,
        ) -> Option<Vec<tau_domain::Capability>> {
            #[derive(serde::Deserialize)]
            struct W {
                cap: tau_domain::Capability,
            }
            (tool_id.0 == "fetch").then(|| {
                vec![
                    toml::from_str::<W>("[cap]\nkind=\"net.http\"\nhosts=[\"api.weather.com\"]\n")
                        .unwrap()
                        .cap,
                ]
            })
        }
    }

    let (module, entry) = module_with_net_tool();
    let backend: Arc<dyn DynLlmBackend> = Arc::new(Scripted {
        queue: Mutex::new(vec![
            resp(serde_json::json!({
                "text":"","tool_uses":[{"id":"t1","name":"fetch","input":{}}],
                "stop_reason":"ToolUse","usage":null
            })),
            resp(serde_json::json!({
                "text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null
            })),
        ]),
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collector = Arc::new(Collector(StdMutex::new(Vec::new())));
    let dispatcher = Arc::new(RecordingClamped {
        seen: seen.clone(),
        backend,
        collector: collector.clone(),
    });

    let stream = run_ir_streaming(Arc::new(module), &entry, dispatcher, Vec::new())
        .await
        .expect("stream builds");
    let events: Vec<RunEvent> = Box::pin(stream).collect().await;

    // (1) The #581 contract still holds: no PolicyDenied, tool dispatched.
    match events.last() {
        Some(RunEvent::RunCompleted {
            outcome: RunOutcome::Completed { .. },
        }) => {}
        other => panic!("expected RunCompleted/Completed, got {other:?}"),
    }
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["fetch"],
        "forwarding effective caps must NOT gate the tool (#581)"
    );

    // (2) …and the row is labelled with the clamp.
    let verdict = collector
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match &e.kind {
            tau_ports::TraceEventKind::ToolCall { capability, .. } => Some(capability.clone()),
            _ => None,
        })
        .expect("a ToolCall trace event");
    assert_eq!(
        verdict,
        Some(tau_ports::CapabilityVerdict::Clamp {
            to: "api.weather.com".into()
        })
    );
}
```

- [ ] **Step 2: Run both tests in the file**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-runtime-core --features test-fixtures --test ir_dispatch_gate_inert
```

Expected: both PASS (Tasks 1–3 already implement the behavior; this test pins it). If the new test fails on the `seen` assertion, a declared-capability forward leaked in — revisit Task 2.

- [ ] **Step 3: Commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-runtime-core -- --check
git add crates/tau-runtime-core/tests/ir_dispatch_gate_inert.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(kernel): pin that effective-cap forwarding keeps the IR gate inert (#581, #631)"
```

---

### Task 6: `tau-trace` envelope-tolerant ingestion (spec §13.5)

Pre-existing bug, DoD-blocking: the writer emits `{"line_kind":"trace_event","event":{…}}` but the reader deserializes a bare `TraceEvent`, so `tau trace` renders an empty waterfall for every run log ever written. Independent of Tasks 1–5; can be done in any order.

**Files:**
- Modify: `crates/tau-trace/src/ingest.rs`
- Test: `crates/tau-trace/src/ingest.rs` (in-file `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `parse_line(&str) -> Result<Option<TraceEvent>, IngestError>` — same signature, now accepting both the envelope and bare shapes, and returning `Ok(None)` for non-`trace_event` envelope kinds.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/tau-trace/src/ingest.rs`:

```rust
    #[test]
    fn parses_the_wrapped_run_log_envelope() {
        // This is the shape `tau-runtime-tokio`'s persistence writer actually
        // produces (`RunLogLine::TraceEvent`). Before spec §13.5 the reader
        // only accepted the bare event, so `tau trace` rendered nothing.
        let evt = TraceEvent {
            id: "evt-9".into(),
            ts: Utc.timestamp_opt(1_700_000_300, 0).unwrap(),
            run_id: "run-9".into(),
            agent_id: Some("agent-9".into()),
            kind: TraceEventKind::ToolCall {
                tool_name: "weather.get_forecast".into(),
                duration_ms: 7,
                status: "ok".into(),
                capability: Some(tau_ports::CapabilityVerdict::Clamp {
                    to: "api.weather.com".into(),
                }),
                turn_index: 0,
            },
        };
        let line = serde_json::json!({
            "line_kind": "trace_event",
            "event": evt,
        })
        .to_string();

        let parsed = parse_line(&line).unwrap().unwrap();

        assert_eq!(parsed, evt);
    }

    #[test]
    fn skips_non_trace_event_envelope_kinds() {
        // `RunLogLine::TaskMutation` is a forward-compat line kind. It is not
        // a trace event, so it is skipped — not an error.
        let line = serde_json::json!({
            "line_kind": "task_mutation",
            "task_id": "01",
            "mutation": "{\"status\":\"done\"}",
        })
        .to_string();

        assert!(parse_line(&line).unwrap().is_none());
    }

    #[test]
    fn still_parses_a_bare_trace_event() {
        // Back-compat: older logs and in-repo fixtures hold bare events.
        let evt = TraceEvent {
            id: "evt-10".into(),
            ts: Utc.timestamp_opt(1_700_000_400, 0).unwrap(),
            run_id: "run-10".into(),
            agent_id: None,
            kind: TraceEventKind::Abort {
                reason: "watchdog".into(),
            },
        };
        let line = serde_json::to_string(&evt).unwrap();

        assert_eq!(parse_line(&line).unwrap().unwrap(), evt);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-trace
```

Expected: `parses_the_wrapped_run_log_envelope` and `skips_non_trace_event_envelope_kinds` FAIL (`bad trace line: unknown field 'line_kind'` or `missing field 'id'`). `still_parses_a_bare_trace_event` PASSES.

- [ ] **Step 3: Implement lenient parsing**

Replace `parse_line` and add the local envelope mirror in `crates/tau-trace/src/ingest.rs`:

```rust
/// Local mirror of `tau_runtime_tokio::orchestration::persistence::RunLogLine`.
///
/// `tau-trace` is pure and headless — it must not depend on the tokio host
/// crate — so the wire shape is mirrored here instead of imported. The
/// `envelope_shape_matches_the_writer` test guards against drift: if the
/// writer's tag or field names change, that test fails rather than the
/// reader silently rendering an empty waterfall (spec §13.5).
#[derive(serde::Deserialize)]
#[serde(tag = "line_kind", rename_all = "snake_case")]
enum RunLogLineMirror {
    TraceEvent { event: TraceEvent },
    /// Any other known line kind is skipped, not an error.
    #[serde(other)]
    Other,
}

/// Parse one `.tau/runs/<id>.jsonl` line into a [`TraceEvent`].
///
/// Accepts both line shapes:
/// - the **envelope** the run-log writer produces,
///   `{"line_kind":"trace_event","event":{…}}` — non-`trace_event` kinds
///   (e.g. `task_mutation`) yield `Ok(None)`;
/// - a **bare** `TraceEvent`, for older logs and test fixtures.
///
/// Returns `Ok(None)` for a blank/whitespace-only line (a trailing newline,
/// or a live tail reading past the last complete line) and for envelope
/// lines that carry no trace event. Returns `Err` for any other non-empty
/// line that fails to deserialize — including a half-written/truncated line
/// read mid-write, which is *not* treated as `Ok(None)`. A live-tail caller
/// should treat such an `Err` on the still-growing tail line as retryable.
pub fn parse_line(line: &str) -> Result<Option<TraceEvent>, IngestError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // Try the envelope first: it is what the writer emits today.
    if let Ok(wrapped) = serde_json::from_str::<RunLogLineMirror>(trimmed) {
        return Ok(match wrapped {
            RunLogLineMirror::TraceEvent { event } => Some(event),
            RunLogLineMirror::Other => None,
        });
    }
    serde_json::from_str::<TraceEvent>(trimmed)
        .map(Some)
        .map_err(|e| IngestError::Json(e.to_string()))
}
```

Also update the module doc at the top of the file (lines 1-13) to describe both shapes — replace the first sentence with:

```rust
//! [`parse_line`] turns one line of a `.tau/runs/<id>.jsonl` file into a
//! [`tau_ports::TraceEvent`], accepting both the writer's
//! `{"line_kind":"trace_event","event":{…}}` envelope and a bare event
//! (spec §13.5).
```

- [ ] **Step 4: Add the anti-drift test**

`tau-trace` must not depend on `tau-runtime-tokio` in its normal dependency graph. Add the guard as a *literal-shape* assertion so no dependency is needed:

```rust
    #[test]
    fn envelope_shape_matches_the_writer() {
        // Drift guard (spec §13.5): this literal is the exact shape
        // `tau_runtime_tokio::orchestration::persistence::spawn_writer`
        // serializes via `RunLogLine::TraceEvent`. If the writer's tag name
        // (`line_kind`), tag value (`trace_event`), or payload field
        // (`event`) ever changes, this test fails loudly instead of
        // `tau trace` silently rendering an empty waterfall.
        let evt = TraceEvent {
            id: "evt-11".into(),
            ts: Utc.timestamp_opt(1_700_000_500, 0).unwrap(),
            run_id: "run-11".into(),
            agent_id: None,
            kind: TraceEventKind::Turn {
                agent_id: "a".into(),
                turn_index: 0,
                duration_ms: 1,
                tokens: 0,
            },
        };
        let mut line = serde_json::Map::new();
        line.insert("line_kind".into(), serde_json::Value::String("trace_event".into()));
        line.insert("event".into(), serde_json::to_value(&evt).unwrap());
        let encoded = serde_json::Value::Object(line).to_string();

        assert_eq!(parse_line(&encoded).unwrap().unwrap(), evt);
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-trace
```

Expected: all PASS, including the pre-existing `garbage_is_err_not_panic` and `truncated_mid_write_line_is_err_not_panic` (a truncated envelope fails both branches and still returns `Err`).

- [ ] **Step 6: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-trace -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-trace --all-targets
git add crates/tau-trace/src/ingest.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "fix(trace): accept the run-log envelope in parse_line (#631)"
```

---

### Task 7: `ForwardingDispatcher` implements both accessors (spec §13.2, §13.3, §13.5)

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs` — struct (~line 425-443), `new`/`single` (~446-507), new `with_trace` builder, `impl ToolDispatcher` (~line 510+)
- Test: `crates/tau-cli/src/cmd/ir_dispatcher.rs` (in-file `mod tests`, near the existing `tool_effective_capabilities` unit tests at ~line 1358)

**Interfaces:**
- Consumes: Task 2's `ToolDispatcher::tool_effective_capabilities`, Task 3's `TraceSinkConfig` + `ToolDispatcher::trace_sink`.
- Produces: `ForwardingDispatcher::with_trace(mut self, run_id: String, subscribers: Vec<Arc<dyn TraceSubscriber>>) -> Self`. Task 8 calls it.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/tau-cli/src/cmd/ir_dispatcher.rs`:

```rust
    #[test]
    fn dispatcher_reports_effective_caps_from_its_tool_map() {
        use tau_runtime_core::interpreter::tool_dispatch::ToolDispatcher as _;

        // An McpBackedTool-like handle carrying a narrowed authority.
        let declared = cap_toml("[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\", \"evil.example\"]\n");
        let effective = cap_toml("[cap]\nkind = \"net.http\"\nhosts = [\"api.weather.com\"]\n");
        let clamped: Arc<dyn DynTool> = tau_mcp_tokio::bridge::McpBackedTool::new(
            "weather.get_forecast".to_string(),
            test_mcp_client(),
            "get_forecast".to_string(),
            vec![declared],
            Some(vec![effective.clone()]),
            serde_json::json!({"type": "object"}),
            String::new(),
        );

        let mut tools: BTreeMap<ToolId, Arc<dyn DynTool>> = BTreeMap::new();
        tools.insert(ToolId("weather.get_forecast".into()), clamped);

        let dispatcher = ForwardingDispatcher::new(BTreeMap::new(), tools);

        assert_eq!(
            dispatcher.tool_effective_capabilities(&ToolId("weather.get_forecast".into())),
            Some(vec![effective]),
            "the dispatcher must surface the tool's meet-clamped authority"
        );
        assert_eq!(
            dispatcher.tool_effective_capabilities(&ToolId("not-registered".into())),
            None,
            "unknown ids report no clamp rather than panicking"
        );
    }

    #[test]
    fn dispatcher_reports_no_trace_sink_until_with_trace_is_called() {
        use tau_runtime_core::interpreter::tool_dispatch::ToolDispatcher as _;

        let dispatcher = ForwardingDispatcher::new(BTreeMap::new(), BTreeMap::new());
        assert!(
            dispatcher.trace_sink().is_none(),
            "a dispatcher without with_trace must not enable trace emission"
        );

        let collector: Arc<dyn tau_runtime_core::orchestration::trace::TraceSubscriber> =
            Arc::new(tau_runtime_core::orchestration::trace::NoopTraceSubscriber);
        let dispatcher = ForwardingDispatcher::new(BTreeMap::new(), BTreeMap::new())
            .with_trace("run-abc".to_string(), vec![collector]);

        let sink = dispatcher.trace_sink().expect("sink after with_trace");
        assert_eq!(sink.run_id, "run-abc");
        assert_eq!(sink.subscribers.len(), 1);
    }
```

If a `cap_toml` helper does not already exist in that test module, add it (mirroring the pattern already used by `narrowed_tool_reports_effective_with_clamped_hosts`), and reuse whatever helper that test uses to construct an MCP client handle — if constructing a real `McpBackedTool` is impractical in a unit test, substitute a local `struct ClampedStub` implementing `DynTool` with `effective_capabilities()` returning `Some(&self.effective)`, which exercises the same dispatcher code path.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo nextest run -p tau-cli --lib ir_dispatcher
```

Expected: COMPILE ERROR — `no method named with_trace`; `trace_sink`/`tool_effective_capabilities` resolve to the trait defaults and the assertions fail.

- [ ] **Step 3: Add the fields, the builder, and the trait impls**

Add to `struct ForwardingDispatcher` (after `assets`, ~line 442):

```rust
    /// Trace sink for this run (execution-trace TUI spec §13.3). `None`
    /// unless [`Self::with_trace`] was called, which keeps IR runs that
    /// don't want tracing byte-identical to their pre-§13 behavior.
    trace_run_id: Option<String>,
    trace_subscribers: Vec<Arc<dyn tau_runtime_core::orchestration::trace::TraceSubscriber>>,
```

Initialize both in `new` and in the `#[cfg(test)] fn single` constructor:

```rust
            trace_run_id: None,
            trace_subscribers: Vec::new(),
```

Add the builder next to `with_durable`:

```rust
    /// Attach the run's trace sink (spec §13.3). The interpreter reads it
    /// via [`ToolDispatcher::trace_sink`] and builds a synthetic `RunState`
    /// so the kernel's `TraceEvent` emit sites fire for this IR run.
    ///
    /// `run_id` is the id stamped on every event and the
    /// `.tau/runs/<run_id>.jsonl` filename; `subscribers` is typically the
    /// JSONL writer plus, under `--tui`, a live channel.
    pub(crate) fn with_trace(
        mut self,
        run_id: String,
        subscribers: Vec<Arc<dyn tau_runtime_core::orchestration::trace::TraceSubscriber>>,
    ) -> Self {
        if !subscribers.is_empty() {
            self.trace_run_id = Some(run_id);
            self.trace_subscribers = subscribers;
        }
        self
    }
```

Add both methods to `impl ToolDispatcher for ForwardingDispatcher`:

```rust
    /// Spec §13.2: surface the meet-clamped authority `setup_mcp_runtime`
    /// computed at MCP open time. The `McpBackedTool` in our map carries it;
    /// every other tool reports `None` (the `DynTool` default).
    fn tool_effective_capabilities(
        &self,
        tool_id: &ToolId,
    ) -> Option<Vec<tau_domain::Capability>> {
        self.tools
            .get(tool_id)?
            .effective_capabilities()
            .map(<[tau_domain::Capability]>::to_vec)
    }

    /// Spec §13.3: hand the interpreter this run's trace sink, if the host
    /// attached one via [`Self::with_trace`].
    fn trace_sink(
        &self,
    ) -> Option<tau_runtime_core::interpreter::tool_dispatch::TraceSinkConfig> {
        Some(
            tau_runtime_core::interpreter::tool_dispatch::TraceSinkConfig {
                run_id: self.trace_run_id.clone()?,
                subscribers: self.trace_subscribers.clone(),
            },
        )
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo nextest run -p tau-cli --lib ir_dispatcher
```

Expected: PASS, including the pre-existing `tool_effective_capabilities` meet tests.

- [ ] **Step 5: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo fmt -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo clippy -p tau-cli --all-targets
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(cli): forwarding dispatcher exposes clamp authority and trace sink (#631)"
```

---

### Task 8: `run_via_ir` sink construction + live `--tui` (spec §13.5)

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs` — `run_via_ir`: run-id minting (~line 141), dispatcher construction (~line 283), the two drive sites (~line 373 pipeline, ~line 395 single-agent)
- Modify (read first): `crates/tau-cli/src/cmd/run.rs:366-411` for the exact `tokio::join!` TUI pattern to mirror

**Interfaces:**
- Consumes: Task 7's `ForwardingDispatcher::with_trace`.
- Produces: `tau run --bundle` writes `<scope>/.tau/runs/<run_id>.jsonl`; `tau run --bundle --tui` renders live. Task 9's e2e depends on the file path and its contents.

- [ ] **Step 1: Mint one run id and build the subscribers**

In `run_via_ir`, replace the trace-context id block at ~line 141:

```rust
    // One ULID per run, shared by the plugin TraceContext (log grouping) and
    // the orchestration trace sink (spec §13.5), so `.tau/runs/<id>.jsonl`,
    // `tau trace <id>` and the plugin logs all agree on the run's identity.
    let run_id = crate::cmd::run::mint_run_id();
    let trace_context = TraceContext::new(
        run_id.clone(),
        entry_agent_id.0.clone(),
        "root".to_string(),
    );
```

- [ ] **Step 2: Attach the sink to the dispatcher**

At the dispatcher construction (~line 283), before `let dispatcher = Arc::new(dispatcher.with_assets(assets));`, insert:

```rust
    // Spec §13.5: attach the run-log writer (and, under --tui, a live
    // channel) so the interpreter's agent loop emits TraceEvents. Reuses the
    // same writer + file namespace as the multi-agent path, so
    // `tau trace --last` picks up bundle runs with no reader changes.
    let mut trace_subscribers: Vec<
        Arc<dyn tau_runtime_core::orchestration::trace::TraceSubscriber>,
    > = Vec::new();
    trace_subscribers.push(
        tau_runtime_tokio::orchestration::trace_mpsc::channel_with_writer(
            tau_runtime_tokio::orchestration::persistence::run_log_path(scope.path(), &run_id),
        ),
    );
    let tui_rx = if args.tui {
        let (subscriber, rx) =
            tau_runtime_tokio::orchestration::trace_mpsc::MpscTraceSubscriber::channel();
        trace_subscribers.push(Arc::new(subscriber));
        Some(rx)
    } else {
        None
    };
    let dispatcher = dispatcher.with_trace(run_id.clone(), trace_subscribers);
```

Verify the exact module paths for `channel_with_writer`, `run_log_path` and `MpscTraceSubscriber` against `crates/tau-runtime-tokio/src/orchestration/` and the re-exports in that crate's `lib.rs`; adjust the `use` paths if they are re-exported more shallowly (as `drive.rs` does).

- [ ] **Step 3: Join the TUI around both drive sites**

The run future must stay on the current task (`RunState` is `!Send`). Wrap the single-agent drive at ~line 395:

```rust
    // 8. Drive the single entry agent.
    let run_fut = run_ir(module, &entry_agent_id, dispatcher, vec![initial]);
    let run_outcome = match tui_rx {
        Some(rx) => {
            // Mirrors cmd/run.rs's multi-agent --tui join: `run_tui` is a
            // blocking raw-mode loop, so it goes to the blocking pool while
            // the run future keeps driving here. Do NOT `tokio::spawn` the
            // run future — its RunState is !Send (spec §13.3).
            let tui_task = tokio::task::spawn_blocking(move || {
                crate::tui::run_tui(crate::tui::TraceSource::Live(rx))
            });
            let (run_res, tui_res) = tokio::join!(run_fut, tui_task);
            match tui_res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e.context("execution-trace TUI")),
                Err(join_err) => {
                    return Err(anyhow::anyhow!(
                        "execution-trace TUI task panicked: {join_err}"
                    ))
                }
            }
            run_res
        }
        None => run_fut.await,
    };
```

Apply the same shape to the pipeline drive at ~line 373 (`run_pipeline(...)`). If both sites would duplicate more than a few lines, extract a small local helper `async fn drive_with_optional_tui<F>(fut: F, tui_rx: Option<...>) -> F::Output` in this file and call it from both.

Note the error-precedence rule from `cmd/run.rs:389-411`: when the run itself fails, surface the run error first and attach the TUI failure as context. Mirror that if you extract the helper.

- [ ] **Step 4: Verify `--tui` reaches this path**

Read `crates/tau-cli/src/cmd/run.rs:163` and confirm `validate_tui_flag` runs before the `--bundle` short-circuit (it validates TTY + `--json` conflicts). If it does not, call it at the top of `run_via_ir` when `args.tui` is set, so `--tui` on a non-TTY still fails with the existing clear message rather than corrupting output.

- [ ] **Step 5: Build and run the CLI test suite**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo check -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo nextest run -p tau-cli
```

Expected: PASS. `cmd_run_mcp.rs` and any bundle-run tests must stay green.

- [ ] **Step 6: Format, lint, commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo fmt -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo clippy -p tau-cli --all-targets
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(cli): write a run log and wire --tui for bundle IR runs (#631)"
```

---

### Task 9: Clamp badge render test + docs (spec §13.6)

Spec §12.7 claimed this renderer test existed; it does not. Close the last hop from event to pixels, and document the IR-path `-` asymmetry.

**Files:**
- Test: `crates/tau-cli/src/tui/render.rs` (in-file `mod tests`, next to the existing `Drop`/`Allow`/`None` badge tests at ~lines 328-463)
- Modify: `docs/reference/tau-trace.md` (the "Capability badges" table area, ~lines 49-59)

**Interfaces:**
- Consumes: nothing (the renderer arm already exists).
- Produces: nothing consumed downstream.

- [ ] **Step 1: Write the failing test**

Read the existing badge tests at `crates/tau-cli/src/tui/render.rs:328` (Drop), `:377`/`:424` (Allow), `:463` (None) and add a `Clamp` sibling in the same style. Sketch — match the existing helpers' names and assertion style exactly:

```rust
    #[test]
    fn clamp_verdict_renders_amber_badge_with_host_list() {
        // Spec §13.6: the amber `clamp:<to>` arm shipped in M1.5 without a
        // test; #631 makes it reachable, so pin the rendering.
        let (label, style) = capability_badge(Some(&CapabilityVerdict::Clamp {
            to: "api.weather.com".into(),
        }));
        assert_eq!(label, "clamp:api.weather.com");
        assert_eq!(style.fg, Some(Color::Yellow));
    }

    #[test]
    fn clamp_verdict_detail_pane_shows_an_arrow() {
        assert_eq!(
            capability_detail(Some(&CapabilityVerdict::Clamp {
                to: "a.com,b.com".into()
            })),
            "clamp -> a.com,b.com"
        );
    }
```

Confirm `capability_detail`'s exact return type and format string at `render.rs:284-291` before writing the second assertion.

- [ ] **Step 2: Run to verify they pass (or fix the expectations)**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo nextest run -p tau-cli --lib render
```

Expected: PASS. These pin existing behavior rather than driving new code — if one fails, correct the *test* to the real format, not the renderer.

- [ ] **Step 3: Update the reference doc**

In `docs/reference/tau-trace.md`, extend the `-` badge row's description in the "Capability badges" table so it reads:

> | `-` | The tool is un-gated, or the trace predates capability recording. Bundle (IR) runs report `-` for every un-clamped tool: those tools dispatch through the interpreter's wrapper, which deliberately presents no declared capabilities to the kernel gate (issue #581), so there is no `allow` decision to record. A clamped tool still shows `clamp:<to>` on that path. |

Verify the table renders by building the book:

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book
```

Expected: only `[INFO]` lines.

- [ ] **Step 4: Commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo fmt -p tau-cli -- --check
git add crates/tau-cli/src/tui/render.rs docs/reference/tau-trace.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(tui): pin the clamp badge rendering; document the IR-path badge (#631)"
```

---

### Task 10: End-to-end governed cassette-MCP clamp row (spec §13.6 — DoD anchor)

The definition of done: a governed MCP project whose entry is host-clamped by `[allow.mcp]` produces a `clamp:<to>` row, read back through the real reader.

**Files:**
- Create: `crates/tau-cli/tests/clamp_row_e2e.rs`
- Reference (read first): `crates/tau-cli/tests/mcp_dispatch.rs:11-40` (`setup_project_with_pin`, `cassette:` URL), `crates/tau-cli/tests/cmd_build_mcp.rs:200-300` (`write_pinned_weather_contract`, `write_empty_v7_lock`, `make_tau_home`), `crates/tau-cli/tests/cmd_run_mcp.rs:21-114` (inline cassette + clamp threading), `crates/tau-cli/tests/north_star_demo.rs` (governed-project scaffolding: `.tau/config.toml`, manifests, lockfile)

**Interfaces:**
- Consumes: Tasks 1-8 (the whole chain) and Task 6's reader fix.
- Produces: the DoD evidence.

- [ ] **Step 1: Write the failing e2e test**

Create `crates/tau-cli/tests/clamp_row_e2e.rs`. Structure (fill in the scaffolding by copying the referenced helpers verbatim — do not invent new fixture shapes):

```rust
//! Issue #631 / spec §13.6 — definition of done.
//!
//! A governed MCP project whose entry is host-clamped by `[allow.mcp]`
//! renders an amber `clamp:<to>` row: `tau run --bundle` writes
//! `.tau/runs/<id>.jsonl`, and reading it back through the real
//! `tau_trace::parse_line` yields a `ToolCall` carrying
//! `CapabilityVerdict::Clamp`.

use assert_fs::prelude::*;

// ... copy write_empty_v7_lock / write_pinned_weather_contract / make_tau_home
// from cmd_build_mcp.rs, and the inline cassette builder from cmd_run_mcp.rs ...

#[test]
fn governed_clamped_mcp_run_writes_a_clamp_row() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");

    // 1. Cassette-backed MCP server (no process spawn ⇒ no sandbox gate).
    //    Copy the fixture the other MCP tests use.
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");
    tmp.child("fixtures/weather.jsonl")
        .write_binary(&std::fs::read(&cassette_src).expect("read fixture"))
        .expect("write fixture");

    // 2. Governed project: the tool declares two hosts, the [allow.mcp]
    //    ceiling permits one ⇒ the meet narrows ⇒ a clamp.
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name = "clamp-e2e"
version = "0.0.1"

[allow]
# ... mirror the governed fixture in north_star_demo.rs / governance.rs:844 ...

[allow.mcp.weather]
hosts = ["api.weather.com"]

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
capabilities = [{ kind = "net.http", hosts = ["api.weather.com", "evil.example"] }]

# ... one agent referencing weather.get_forecast, echo/mock LLM backend ...
"#,
        )
        .expect("write tau.toml");

    // 3. Pin the contract, write the v7 lockfile, build the bundle,
    //    then run it. Use `tau mcp pin weather` (as mcp_dispatch.rs does)
    //    so the contract hash is self-consistent.
    // ... assert_cmd invocations: `tau mcp pin weather`, `tau build`,
    //     `tau run --bundle <path> --prompt "..."` ...

    // 4. The DoD assertion: read the run log through the REAL reader.
    let runs_dir = tmp.path().join(".tau").join("runs");
    let log = std::fs::read_dir(&runs_dir)
        .expect("`.tau/runs` must exist after a bundle run")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .expect("a run log jsonl must have been written");

    let contents = std::fs::read_to_string(&log).expect("read run log");
    let events: Vec<tau_ports::TraceEvent> = contents
        .lines()
        .filter_map(|l| tau_trace::parse_line(l).expect("every written line must parse"))
        .collect();

    let clamp = events
        .iter()
        .find_map(|e| match &e.kind {
            tau_ports::TraceEventKind::ToolCall {
                tool_name,
                capability: Some(tau_ports::CapabilityVerdict::Clamp { to }),
                ..
            } => Some((tool_name.clone(), to.clone())),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("no clamp ToolCall row in the run log; events were: {events:#?}")
        });

    assert!(clamp.0.contains("get_forecast"), "got tool {:?}", clamp.0);
    assert_eq!(
        clamp.1, "api.weather.com",
        "the row must name the surviving host from the [allow.mcp] ceiling"
    );
}
```

Add `tau-trace` and `tau-ports` to `crates/tau-cli`'s `[dev-dependencies]` if they are not already there.

- [ ] **Step 2: Run it and iterate on the scaffolding**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo nextest run -p tau-cli --test clamp_row_e2e --no-capture
```

The first failures will be fixture/governance shape errors (missing `[allow.models]`, lockfile version, `TAU_HOME`), not the assertion. Work them out against the referenced helper files. Known traps from prior sessions: `[allow.tools]` without a ceiling yields an empty set; `${PROJECT}` breaks glob-subset; the agent grant comes from the package manifest capabilities.

Expected end state before the fix is in: the run succeeds and the assertion fails with "no clamp ToolCall row". If it fails because `.tau/runs` doesn't exist, Task 8's sink wiring isn't reached on this path.

- [ ] **Step 3: Verify it passes with the full chain**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli \
  cargo nextest run -p tau-cli --test clamp_row_e2e
```

Expected: PASS. This is the #631 DoD.

**Post-hoc note (whole-branch review):** it does not pass — the test ships `#[ignore]`d, gated on two pre-existing upstream bugs outside #631's scope (#712 empty MCP handshake capabilities, #714 bundle re-lowering rejects MCP projects). See the test's own doc comment for the full account; un-ignore once both are fixed.

- [ ] **Step 4: Full-suite regression sweep**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo nextest run -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-orch cargo nextest run -p tau-runtime-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-trace
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-runtime-core
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo fmt -p tau-cli -- --check
git add crates/tau-cli/tests/clamp_row_e2e.rs crates/tau-cli/Cargo.toml
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(cli): e2e clamp row for a governed cassette-MCP bundle run (closes #631)"
```

---

### Task 11: Lift the spec's dormancy caveat and open the PR

**Files:**
- Modify: `docs/superpowers/specs/2026-08-21-execution-trace-tui-design.md` §12.1 (the "Reachability today, corrected" paragraph)

- [ ] **Step 1: Update the spec to past tense**

The paragraph currently says the producer "is dormant … Both gaps are closed by the reachability wiring designed in §13 (issue #631)." Change the final sentence to record that it shipped:

```markdown
at all, clamped or otherwise. Net effect: as shipped in #630 the §12
producer was correct but dormant for the stock `tau run` CLI. Both gaps
were closed by the §13 reachability wiring (issue #631): IR runs now
attach a host trace sink and `DispatcherTool` forwards the meet-clamped
`effective_capabilities()`.
```

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin clamp-row-wiring-631
```

PR body must include: the DoD statement, a note that §13.4 hardens the virtual-tool intercept, an explicit **"#581 contract: unchanged"** paragraph explaining that only `effective_capabilities()` is forwarded and that `ir_dispatch_gate_inert.rs`'s original test is untouched (one additive sibling test), and a note that the `tau trace` envelope fix repairs a pre-existing bug affecting multi-agent runs too.

```bash
gh pr create --base main --title "feat(trace): wire the clamp-row producer into stock tau run (closes #631)" --body-file <path>
```

- [ ] **Step 3: Babysit to merge**

Per the user's standing CI rules: watch CI in the background, fix red checks, `gh pr update-branch <N>` when BEHIND, enroll auto-merge **bare** (`gh pr merge <N> --auto` — the merge queue rejects `--squash`). Notify only on merge or a genuine block.

---

## Self-Review

**Spec coverage:** §13.1 → Task 1. §13.2 → Tasks 2, 7 (dispatcher impl), 5 (pin). §13.3 → Tasks 3, 7. §13.4 → Task 4. §13.5 → Tasks 6 (ingestion), 8 (run_via_ir + `--tui`). §13.6 → Tasks 1-5 (kernel/pins), 7 (dispatcher units), 9 (renderer + docs), 10 (e2e). §13.7 (out of scope) → nothing, correctly. §12.1 caveat lift → Task 11.

**Type consistency:** `TraceSinkConfig { run_id: tau_ports::RunId (= String), subscribers: Vec<Arc<dyn crate::orchestration::trace::TraceSubscriber>> }` is defined in Task 3 and used identically in Tasks 4, 5, 7. `tool_effective_capabilities(&self, tool_id: &ToolId) -> Option<Vec<tau_domain::Capability>>` is defined in Task 2 and implemented with the same signature in Task 7. `DispatcherTool.effective_capabilities` is the field (Task 2); `Tool::effective_capabilities()` is the method — the field is `Option<Vec<_>>`, the method returns `Option<&[_]>` via `as_deref()`. `with_trace(run_id: String, subscribers: Vec<...>)` is defined in Task 7 and called in Task 8.

**Known soft spots the executor must resolve against real code, not guess:** the exact `make_tool_spec` signature in `agent_loop.rs`; whether `agent_loop.rs` already has a `mod tests`; the re-export depth of `channel_with_writer` / `run_log_path` / `MpscTraceSubscriber` in `tau-runtime-tokio`; whether `cap_toml` and an MCP-client test helper exist in `ir_dispatcher.rs`'s test module; `capability_detail`'s exact format string; and the full governed `[allow]` block shape for Task 10's fixture. Each step says to check.
