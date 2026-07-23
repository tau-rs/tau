# Subflow Runtime Capability Attenuation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce, at runtime in the IR interpreter, that a subflow child (and every descendant) is clamped to the meet of its ancestors' declared cap_subsets — denying any child tool call whose required capabilities exceed the narrowing envelope.

**Architecture:** A no_std `AttenuatedDispatcher` decorator wraps the child's `ToolDispatcher` at each `ToolImpl::Subflow` spawn, carrying that subflow tool's `capabilities` as the frame grant. Its `invoke` gates each child tool call against the frame (reusing `check_capabilities`) and delegates inward; recursive nesting composes into the exact meet with no materialized intersection. Denials are soft (`is_error` tool result), reuse `CapabilityDenial`, and emit a `tracing` event — all in `tau-runtime-core` so `tau dev`, `tau run --bundle`, and the wasm guest behave identically.

**Tech Stack:** Rust (no_std `tau-runtime-core`), `tracing`, `cargo nextest`, `tau-ir-conformance` fixture harness.

**Spec:** `docs/superpowers/specs/2026-07-19-subflow-runtime-attenuation-design.md`

## Global Constraints

- `tau-runtime-core` is **no_std** — use `alloc::` types, no `std::`. New code must compile under the crate's default (no_std) build.
- **No IR format bump, no lowering change, no new lowering error variant.** cap_subset = the subflow tool's existing `workflow.tools[id].capabilities`.
- Reuse existing helpers: `crate::capability::check_capabilities(granted: &[Capability], required: &[Capability]) -> Option<&Capability>` (capability.rs:72) and `crate::capability::capability_kind_str(cap) -> String` (capability.rs:136). Do not write new subset logic.
- Denial construction mirrors the kernel path (`stream.rs:748-755`): `required_kind = capability_kind_str(cap)`, `required_detail = format!("{cap:?}")`.
- Every cargo command (per repo `CLAUDE.md`): `timeout <t> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate>` (build/check 180s, test 300s). Doctests: `cargo test --doc`.
- Interpreter integration tests are gated `#![cfg(feature = "test-fixtures")]` and run with `--features test-fixtures`.
- Commit after each task with a conventional-commit message.

---

### Task 1: Extend `CapabilityDenial` with a narrowing frame

Add an optional provenance field naming the subflow tool whose cap_subset removed a capability, without breaking the existing `new()` signature or callers.

**Files:**
- Modify: `crates/tau-runtime-core/src/error.rs:130-180`
- Test: same file's `#[cfg(test)]` module (add there; create one if absent)

**Interfaces:**
- Produces: `CapabilityDenial::with_narrowing_frame(self, frame: impl Into<String>) -> Self`; new public field `narrowing_frame: Option<String>`; `Display` appends `" (narrowed by subflow \`<frame>\`)"` when `Some`.

- [ ] **Step 1: Write the failing test**

Add to `crates/tau-runtime-core/src/error.rs` test module:

```rust
#[test]
fn denial_with_narrowing_frame_renders_frame() {
    let d = CapabilityDenial::new(
        "worker", "ir-agent", "page", "net.http", "Network(Http { .. })",
    )
    .with_narrowing_frame("notify");
    let s = d.to_string();
    assert!(s.contains("page"), "{s}");
    assert!(s.contains("net.http"), "{s}");
    assert!(s.contains("narrowed by subflow `notify`"), "{s}");
    assert_eq!(d.narrowing_frame.as_deref(), Some("notify"));
}

#[test]
fn denial_without_frame_is_unchanged() {
    let d = CapabilityDenial::new("a", "p", "t", "k", "detail");
    assert_eq!(d.narrowing_frame, None);
    assert!(!d.to_string().contains("narrowed by subflow"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core denial_with_narrowing_frame`
Expected: FAIL — no method `with_narrowing_frame`, no field `narrowing_frame`.

- [ ] **Step 3: Add the field, constructor default, builder, and Display branch**

In `crates/tau-runtime-core/src/error.rs`, add the field to the struct (after `required_detail`):

```rust
    /// Human-readable description of the capability that wasn't satisfied.
    pub required_detail: String,
    /// When this denial came from subflow attenuation, the subflow tool id
    /// whose cap_subset removed the capability. `None` for kernel-path
    /// (non-subflow) denials.
    pub narrowing_frame: Option<String>,
```

In `new()`, initialize it:

```rust
        Self {
            agent_id: agent_id.into(),
            package_id: package_id.into(),
            tool_name: tool_name.into(),
            required_kind: required_kind.into(),
            required_detail: required_detail.into(),
            narrowing_frame: None,
        }
```

Add the builder after `new()`:

```rust
    /// Attach the subflow tool id that imposed the narrowing (provenance for
    /// subflow attenuation denials).
    pub fn with_narrowing_frame(mut self, frame: impl Into<String>) -> Self {
        self.narrowing_frame = Some(frame.into());
        self
    }
```

Update `Display::fmt` to append the frame when present:

```rust
impl core::fmt::Display for CapabilityDenial {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "agent {} (package {}) lacks capability `{}` ({}) required to call tool `{}`",
            self.agent_id,
            self.package_id,
            self.required_kind,
            self.required_detail,
            self.tool_name,
        )?;
        if let Some(frame) = &self.narrowing_frame {
            write!(f, " (narrowed by subflow `{frame}`)")?;
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core denial_`
Expected: PASS (both new tests).

- [ ] **Step 5: Run the doctest for `CapabilityDenial`**

The struct has a doctest (`error.rs:113-129`) that must still pass with the new field.
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --doc error`
Expected: PASS (the field is defaulted; existing doctest unaffected).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/error.rs
git commit -m "feat(tau-runtime-core): add narrowing_frame provenance to CapabilityDenial"
```

---

### Task 2: `AttenuatedDispatcher` decorator + unit tests

The core: a `ToolDispatcher` decorator that gates one frame's cap_subset and delegates everything else. Nesting composes into the exact meet.

**Files:**
- Create: `crates/tau-runtime-core/src/interpreter/attenuate.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs` (add `pub(crate) mod attenuate;`)
- Test: unit tests inside `attenuate.rs`

**Interfaces:**
- Consumes: `CapabilityDenial::with_narrowing_frame` (Task 1); `crate::capability::{check_capabilities, capability_kind_str}`; `ToolDispatcher`, `ToolInvocationResult` (tool_dispatch.rs); `tau_ir::{IrModule, ToolId}`; `tau_ir::capability::CapabilityRequirements`.
- Produces: `pub(crate) struct AttenuatedDispatcher { grant: CapabilityRequirements, frame: ToolId, agent_id: String, module: Arc<IrModule>, inner: Arc<dyn ToolDispatcher + Send + Sync> }` with `pub(crate) fn new(...) -> Self`. Implements `ToolDispatcher`. Used by Task 3.

- [ ] **Step 1: Register the module**

In `crates/tau-runtime-core/src/interpreter/mod.rs`, add alongside the other submodule declarations:

```rust
pub(crate) mod attenuate;
```

- [ ] **Step 2: Write the failing unit tests**

Create `crates/tau-runtime-core/src/interpreter/attenuate.rs` with **only** the test module first (the impl follows in Step 4). Use a stub inner dispatcher that records whether `invoke` was reached:

```rust
//! test scaffold — impl added in Step 4
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::string::ToString;
    use core::sync::atomic::{AtomicBool, Ordering};
    use serde_json::json;
    use tau_ir::capability::{CapabilityRequirements, CapabilityTable};
    use tau_ir::ids::{AgentId, ToolId};
    use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ir::node::{Tool, ToolSpec};
    use tau_ir::tool_impl::{NativeFnRef, ToolImpl};
    use crate::error::RuntimeError;
    use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

    // capability from canonical TOML (variant #[non_exhaustive]).
    fn cap(toml_str: &str) -> tau_domain::Capability {
        #[derive(serde::Deserialize)]
        struct W { cap: tau_domain::Capability }
        toml::from_str::<W>(toml_str).expect("cap toml").cap
    }
    fn reqs(caps: alloc::vec::Vec<tau_domain::Capability>) -> CapabilityRequirements {
        CapabilityRequirements { declared: caps }
    }

    /// Inner dispatcher that flips a flag when `invoke` is reached.
    struct Spy(Arc<AtomicBool>);
    impl ToolDispatcher for Spy {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a ToolId,
            _args: &'a serde_json::Value,
        ) -> core::pin::Pin<alloc::boxed::Box<dyn core::future::Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
            self.0.store(true, Ordering::SeqCst);
            alloc::boxed::Box::pin(async {
                Ok(ToolInvocationResult { body: Some(json!("ok")), error: None })
            })
        }
        fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn crate::builder::DynLlmBackend>, RuntimeError> {
            Err(RuntimeError::Internal { message: "spy: no backend".into() })
        }
    }

    /// Module with one tool `t` carrying `t_caps`.
    fn module_with_tool(tool_id: &str, t_caps: CapabilityRequirements) -> Arc<IrModule> {
        let mut tools = alloc::collections::BTreeMap::new();
        tools.insert(
            ToolId(tool_id.to_string()),
            Tool {
                id: ToolId(tool_id.to_string()),
                impl_: ToolImpl::Native { fn_ref: NativeFnRef { name: tool_id.into() }, content_hash: [1u8; 32] },
                capabilities: t_caps,
                spec: ToolSpec { name: tool_id.into(), description: String::new(), input_schema: serde_json::Value::Null },
            },
        );
        Arc::new(IrModule {
            ir_format: IrFormatVersion::CURRENT,
            workflow: Workflow { tools, ..Default::default() },
            ..Default::default()
        })
    }

    fn block_on<F: core::future::Future>(f: F) -> F::Output {
        futures_executor::block_on(f)
    }

    #[test]
    fn denies_when_required_exceeds_frame_grant() {
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("page", reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]));
        let att = AttenuatedDispatcher::new(
            reqs(alloc::vec![]), // empty cap_subset
            ToolId("notify".into()),
            "worker".into(),
            module,
            Arc::new(Spy(reached.clone())),
        );
        let res = block_on(att.invoke(&ToolId("page".into()), &json!({}))).unwrap();
        assert!(res.error.is_some(), "expected denial");
        let msg = res.error.unwrap();
        assert!(msg.contains("page") && msg.contains("net.http"), "{msg}");
        assert!(msg.contains("narrowed by subflow `notify`"), "{msg}");
        assert!(!reached.load(Ordering::SeqCst), "inner.invoke must NOT run on denial");
    }

    #[test]
    fn allows_when_required_within_frame_grant() {
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("page", reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]));
        let att = AttenuatedDispatcher::new(
            reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]),
            ToolId("notify".into()), "worker".into(), module, Arc::new(Spy(reached.clone())),
        );
        let res = block_on(att.invoke(&ToolId("page".into()), &json!({}))).unwrap();
        assert!(res.error.is_none() && reached.load(Ordering::SeqCst));
    }

    #[test]
    fn allows_tool_with_no_declared_caps() {
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("noop", reqs(alloc::vec![]));
        let att = AttenuatedDispatcher::new(
            reqs(alloc::vec![]), ToolId("notify".into()), "worker".into(), module, Arc::new(Spy(reached.clone())),
        );
        let res = block_on(att.invoke(&ToolId("noop".into()), &json!({}))).unwrap();
        assert!(res.error.is_none() && reached.load(Ordering::SeqCst));
    }

    #[test]
    fn nested_frames_compose_to_meet() {
        // outer grant C2 = {fs.read /proj/**}; inner grant C1 = {net.http}.
        // tool needs net.http: allowed by C1 but not C2 -> denied at outer.
        let reached = Arc::new(AtomicBool::new(false));
        let module = module_with_tool("page", reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]));
        let inner = AttenuatedDispatcher::new(
            reqs(alloc::vec![cap("[cap]\nkind=\"net.http\"\n")]),
            ToolId("c1".into()), "child".into(), module.clone(), Arc::new(Spy(reached.clone())),
        );
        let outer = AttenuatedDispatcher::new(
            reqs(alloc::vec![cap("[cap]\nkind=\"fs.read\"\npaths=[\"/proj/**\"]\n")]),
            ToolId("c2".into()), "grandchild".into(), module, Arc::new(inner),
        );
        let res = block_on(outer.invoke(&ToolId("page".into()), &json!({}))).unwrap();
        assert!(res.error.is_some(), "outer frame C2 must deny net.http");
        assert!(!reached.load(Ordering::SeqCst));
    }
}
```

Note: add `futures-executor` and `toml` as `[dev-dependencies]` of `tau-runtime-core` if not already present (check `crates/tau-runtime-core/Cargo.toml`; `toml` is already used by `capability.rs` tests, so it is present — verify `futures-executor`, else use the executor already used by sibling tests, e.g. `futures_util`/`pollster`).

- [ ] **Step 3: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core attenuate`
Expected: FAIL to compile — `AttenuatedDispatcher` undefined.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/tau-runtime-core/src/interpreter/attenuate.rs` (above the test module):

```rust
//! Subflow capability attenuation decorator.
//!
//! Wraps a child agent's `ToolDispatcher` at a `ToolImpl::Subflow` spawn,
//! gating every child tool call against the subflow tool's declared
//! `capabilities` (the frame grant). Nesting composes into the exact meet
//! of all ancestor frames — see the design spec
//! `docs/superpowers/specs/2026-07-19-subflow-runtime-attenuation-design.md`.
//!
//! The static half (EPIC 1.5 lattice L2) checks cap_subset ⊆ agent-effective
//! for tau-cli-authored workflows; this runtime half additionally clamps
//! descendants under the runtime narrowing chain and catches hand-crafted IR.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;

use serde_json::Value;

use tau_ir::capability::CapabilityRequirements;
use tau_ir::{IrModule, ToolId};

use crate::builder::DynLlmBackend;
use crate::error::{CapabilityDenial, RuntimeError};
use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

/// A `ToolDispatcher` decorator enforcing one subflow frame's cap_subset.
pub(crate) struct AttenuatedDispatcher {
    /// This frame's cap_subset (the invoking subflow tool's `capabilities`).
    grant: CapabilityRequirements,
    /// The subflow tool id that imposed this frame — denial provenance.
    frame: ToolId,
    /// The child agent id running under this frame — denial `agent_id`.
    agent_id: String,
    /// Source of a called tool's declared required caps.
    module: Arc<IrModule>,
    /// `dyn` so recursive nesting does not create unbounded monomorphized types.
    inner: Arc<dyn ToolDispatcher + Send + Sync>,
}

impl AttenuatedDispatcher {
    pub(crate) fn new(
        grant: CapabilityRequirements,
        frame: ToolId,
        agent_id: String,
        module: Arc<IrModule>,
        inner: Arc<dyn ToolDispatcher + Send + Sync>,
    ) -> Self {
        Self { grant, frame, agent_id, module, inner }
    }
}

impl ToolDispatcher for AttenuatedDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        // A called tool's declared caps live on its Tool node; absent ⇒ none ⇒ allowed.
        let required: &[tau_domain::Capability] = self
            .module
            .workflow
            .tools
            .get(tool_id)
            .map(|t| t.capabilities.declared.as_slice())
            .unwrap_or(&[]);

        if let Some(missing) = crate::capability::check_capabilities(&self.grant.declared, required) {
            let kind = crate::capability::capability_kind_str(missing);
            let denial = CapabilityDenial::new(
                self.agent_id.clone(),
                "ir-agent",
                tool_id.0.clone(),
                kind.clone(),
                alloc::format!("{missing:?}"),
            )
            .with_narrowing_frame(self.frame.0.clone());
            tracing::warn!(
                name = "runtime.subflow.attenuation_denied",
                tool = %tool_id.0,
                missing = %kind,
                frame = %self.frame.0,
            );
            let msg = denial.to_string();
            return Box::pin(async move {
                Ok(ToolInvocationResult { body: None, error: Some(msg) })
            });
        }
        // Permitted at this frame — delegate inward (which may re-check a parent frame).
        self.inner.invoke(tool_id, args)
    }

    fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        self.inner.llm_backend_for(backend)
    }
    fn deterministic_registry(
        &self,
    ) -> Option<Arc<dyn crate::interpreter::deterministic::DeterministicRegistry>> {
        self.inner.deterministic_registry()
    }
    fn clock(&self) -> Option<Arc<dyn tau_ports::Clock>> {
        self.inner.clock()
    }
    fn random(&self) -> Option<Arc<dyn tau_ports::RandomSource>> {
        self.inner.random()
    }
    fn artifact_reader(&self) -> Option<Arc<dyn crate::interpreter::artifact::ArtifactReader>> {
        self.inner.artifact_reader()
    }
    fn context_transformer_registry(
        &self,
    ) -> Option<Arc<dyn crate::context::ContextTransformerRegistry>> {
        self.inner.context_transformer_registry()
    }
    fn checkpointing(&self) -> Option<crate::interpreter::tool_dispatch::DurableHandles> {
        self.inner.checkpointing()
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core attenuate`
Expected: PASS (4 tests). If `check_capabilities`'s argument order differs, adjust to `check_capabilities(granted, required)` per capability.rs:72.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/attenuate.rs crates/tau-runtime-core/src/interpreter/mod.rs crates/tau-runtime-core/Cargo.toml
git commit -m "feat(tau-runtime-core): AttenuatedDispatcher subflow capability gate"
```

---

### Task 3: Wire the subflow spawn to attenuate + trace, with an integration test

Wrap the child dispatcher in `AttenuatedDispatcher` at the `ToolImpl::Subflow` arm and verify end-to-end that a child tool call excluded by an empty cap_subset is denied and never dispatched.

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs:190-206` (the `ToolImpl::Subflow` arm)
- Test: create `crates/tau-runtime-core/tests/subflow_attenuation.rs`

**Interfaces:**
- Consumes: `AttenuatedDispatcher::new` (Task 2); the `DispatcherTool` fields `self.tool_id`, `self.module`, `self.dispatcher` (agent_loop.rs:101-118).

- [ ] **Step 1: Write the failing integration test**

Create `crates/tau-runtime-core/tests/subflow_attenuation.rs`. It mirrors the scripted-backend + recording-dispatcher pattern in `tests/run_ir_streaming.rs` and `tests/pipeline_executor.rs`. A two-agent module (parent → subflow `notify` with empty cap_subset → child `worker` whose tool `page` needs `net.http`); the child is scripted to call `page`; assert `page` is never recorded by the inner dispatcher and the child saw an `is_error` result.

```rust
#![cfg(feature = "test-fixtures")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use tau_ir::budget::AgentBudget;
use tau_ir::capability::CapabilityRequirements;
use tau_ir::ids::{AgentId, ToolId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::{Agent, Tool, ToolSpec};
use tau_ir::tool_impl::ToolImpl;
use tau_ports::{CompletionRequest, CompletionResponse, LlmBackend, LlmError};

use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::run_ir;
use tau_runtime_core::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
use tau_runtime_core::outcome::RunOutcome;

// --- scripted backend: parent calls `notify`, worker calls `page`, then both end ---
struct Scripted { queue: Mutex<Vec<CompletionResponse>> }
fn resp(json: serde_json::Value) -> CompletionResponse {
    serde_json::from_value(json).expect("CompletionResponse deserializes")
}
impl LlmBackend for Scripted {
    fn name(&self) -> &str { "mock-llm" }
    async fn complete(&self, _r: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.queue.lock().unwrap().remove(0))
    }
}

// --- recording inner dispatcher: records every tool it is actually asked to invoke ---
struct Recording { seen: Arc<Mutex<Vec<String>>>, backend: Arc<dyn DynLlmBackend> }
impl ToolDispatcher for Recording {
    fn invoke<'a>(
        &'a self, tool_id: &'a ToolId, _args: &'a Value,
    ) -> core::pin::Pin<Box<dyn core::future::Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        self.seen.lock().unwrap().push(tool_id.0.clone());
        Box::pin(async { Ok(ToolInvocationResult { body: Some(Value::String("ok".into())), error: None }) })
    }
    fn llm_backend_for(&self, _b: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }
}

fn tool(id: &str, impl_: ToolImpl, caps: Vec<tau_domain::Capability>) -> Tool {
    Tool {
        id: ToolId(id.into()), impl_,
        capabilities: CapabilityRequirements { declared: caps },
        spec: ToolSpec { name: id.into(), description: String::new(), input_schema: Value::Null },
    }
}
fn agent(id: &str, tools: &[&str]) -> Agent {
    Agent {
        id: AgentId(id.into()), prompt: String::new(),
        model_ref: tau_ir::model_ref::ModelRef { backend: "mock-llm".into(), model_id: "m".into() },
        tool_refs: tools.iter().map(|s| ToolId(s.to_string())).collect(),
        context: None, budget: AgentBudget { max_turns: Some(3), max_tokens: None },
        produces: vec![], output_schema: None, durable: None,
    }
}
fn net_http() -> tau_domain::Capability {
    #[derive(serde::Deserialize)] struct W { cap: tau_domain::Capability }
    toml::from_str::<W>("[cap]\nkind=\"net.http\"\n").unwrap().cap
}

#[tokio::test]
async fn empty_cap_subset_denies_child_tool_call() {
    let mut agents = BTreeMap::new();
    agents.insert(AgentId("parent".into()), agent("parent", &["notify"]));
    agents.insert(AgentId("worker".into()), agent("worker", &["page"]));
    let mut tools = BTreeMap::new();
    // notify: subflow -> worker, EMPTY cap_subset.
    tools.insert(ToolId("notify".into()),
        tool("notify", ToolImpl::Subflow { target: AgentId("worker".into()) }, vec![]));
    // page: needs net.http.
    tools.insert(ToolId("page".into()),
        tool("page", ToolImpl::Native { fn_ref: tau_ir::tool_impl::NativeFnRef { name: "page".into() }, content_hash: [2u8;32] },
             vec![net_http()]));
    let module = Arc::new(IrModule {
        ir_format: IrFormatVersion::CURRENT,
        workflow: Workflow { agents, tools, ..Default::default() },
        ..Default::default()
    });

    let queue = vec![
        resp(serde_json::json!({"text":"","tool_uses":[{"id":"p1","name":"notify","input":{}}],"stop_reason":"ToolUse","usage":null})),
        resp(serde_json::json!({"text":"","tool_uses":[{"id":"w1","name":"page","input":{}}],"stop_reason":"ToolUse","usage":null})),
        resp(serde_json::json!({"text":"paged","tool_uses":[],"stop_reason":"EndTurn","usage":null})),
        resp(serde_json::json!({"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null})),
    ];
    let backend: Arc<dyn DynLlmBackend> = Arc::new(Scripted { queue: Mutex::new(queue) });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatcher = Arc::new(Recording { seen: seen.clone(), backend });

    let outcome = run_ir(module, &AgentId("parent".into()), dispatcher, Vec::new()).await.unwrap();

    // The child's `page` call was denied by the attenuation frame (empty
    // cap_subset), so the inner dispatcher was NEVER asked to invoke `page`.
    assert!(!seen.lock().unwrap().iter().any(|t| t == "page"),
        "page must be denied before reaching the dispatcher; saw {:?}", seen.lock().unwrap());
    assert!(matches!(outcome, RunOutcome::Completed { .. } | RunOutcome::Succeeded { .. }),
        "soft-deny: run still completes; got {outcome:?}");
}
```

Adjust `resp(...)` field names / `RunOutcome` variant to the exact shapes in `tests/run_ir_streaming.rs` (that file constructs `CompletionResponse` via the same serde escape hatch and shows the canonical `stop_reason`/`usage` spelling and the real `RunOutcome` variant name for a completed run).

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures empty_cap_subset_denies`
Expected: FAIL — `page` IS recorded (attenuation not yet wired), assertion fires.

- [ ] **Step 3: Wire the subflow arm**

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs`, replace the `ToolImpl::Subflow { target } => { … }` body (currently `:190-206`) so it wraps the dispatcher before recursing:

```rust
            ToolImpl::Subflow { target } => {
                let module = self.module.clone();
                let target = target.clone();

                // Attenuation frame: this subflow tool's declared capabilities
                // are the cap_subset the child (and its descendants) run under.
                let cap_subset = module
                    .workflow
                    .tools
                    .get(&self.tool_id)
                    .map(|t| t.capabilities.clone())
                    .unwrap_or_default();
                let child_dispatcher: Arc<dyn ToolDispatcher + Send + Sync> =
                    Arc::new(crate::interpreter::attenuate::AttenuatedDispatcher::new(
                        cap_subset,
                        self.tool_id.clone(),
                        target.0.clone(),
                        module.clone(),
                        self.dispatcher.clone(), // Arc<D> → Arc<dyn ToolDispatcher + Send + Sync>
                    ));

                let outcome = alloc::boxed::Box::pin(crate::interpreter::run_ir(
                    module,
                    &target,
                    child_dispatcher,
                    Vec::new(),
                ))
                .await
                .map_err(|e| ToolError::Internal {
                    message: alloc::format!("subflow recursion error: {e}"),
                })?;

                // ... unchanged: the existing RunOutcome::Failed / success handling below ...
```

Leave the outcome-handling block (`match &outcome { RunOutcome::Failed … }` onward) untouched. Confirm `ToolDispatcher` is object-safe (it is — no generic methods); the `Arc<D> → Arc<dyn ToolDispatcher + Send + Sync>` coercion holds because the `impl` bounds `D: ToolDispatcher + Send + Sync + 'static` (agent_loop.rs:119).

- [ ] **Step 4: Run the integration test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures empty_cap_subset_denies`
Expected: PASS — `page` not recorded, run completes.

- [ ] **Step 5: Run the full crate test suite (guard against regressions)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures`
Expected: PASS. Existing subflow tests that pass empty-cap subflows and expect the child tool to run WILL break here — that is Task 4's job (conformance fixtures); if any live in this crate, note them and fix under Task 4.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/agent_loop.rs crates/tau-runtime-core/tests/subflow_attenuation.rs
git commit -m "feat(tau-runtime-core): attenuate child capabilities at subflow spawn"
```

---

### Task 4: Conformance fixtures — proper narrowing (allowed) + attenuation (denied)

Update the existing subflow fixture so it demonstrates *proper narrowing* (unbroken), and add a new fixture demonstrating *denial*, both under the dev-vs-bundle multiset conformance harness.

**Files:**
- Modify: `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/19_subflow_attenuation_denied/{workflow.toml,mock_llm.jsonl,expected_report.json}`
- Reference (do not edit unless enumeration is manual): `crates/tau-ir-conformance/src/lib.rs`, `dev_mode.rs`, `bundle_mode.rs`

**Interfaces:**
- Consumes: the attenuation behavior from Task 3.

- [ ] **Step 1: Fix fixture 04 to model proper narrowing**

Fixture 04 currently sets `notify.capabilities = []`; with attenuation the child's `page` (net.http) call is now denied, breaking its `page:{}: 1` expectation. Widen the cap_subset so the child may still page. In `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/workflow.toml`, change the `notify` tool:

```toml
[tools.notify]
subflow      = "worker"
description  = "Hand off the alert to the worker agent."
capabilities = [{ kind = "net.http" }]
```

`expected_report.json` is unchanged (`page:{}: 1`) — this is now the "proper narrowing → allowed" case: `meet(⊤, {net.http}) = {net.http}` ⊇ page's requirement.

- [ ] **Step 2: Run the conformance suite to confirm 04 passes again**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance fixture_04`
Expected: PASS (dev + bundle multiset match, `page:{}: 1`). If it still fails, the fixture is enumerated by a different test name — list tests with `cargo nextest list -p tau-ir-conformance | grep 04`.

- [ ] **Step 3: Create the denial fixture directory + files**

Create `crates/tau-ir-conformance/fixtures/19_subflow_attenuation_denied/workflow.toml` (identical to 04 except the empty cap_subset):

```toml
packages = ["mock-llm"]

[project]
name = "fixture-19"

[models.mock-1]
backend = "mock-llm"
model = "mock-1"

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
model        = "mock-1"
tool_refs    = ["notify"]
max_turns    = 3

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
model        = "mock-1"
tool_refs    = ["page"]
max_turns    = 3

[tools.notify]
subflow      = "worker"
description  = "Hand off the alert to the worker agent."
capabilities = []

[tools.page]
mcp          = "https://mcp.pager.example.com"
description  = "Page the on-call rotation."
capabilities = [{ kind = "net.http" }]
```

Create `.../19_subflow_attenuation_denied/mock_llm.jsonl` (same script as 04 — the worker still *attempts* `page`; attenuation suppresses it, the worker then ends):

```jsonl
{"turn": 0, "response": {"tool_uses": [{"id": "p1", "name": "notify", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"tool_uses": [{"id": "w1", "name": "page", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 2, "response": {"text": "paged", "stop_reason": "end_turn"}}
{"turn": 3, "response": {"text": "done", "stop_reason": "end_turn"}}
```

Create `.../19_subflow_attenuation_denied/expected_report.json`. The `page` call is denied by the empty cap_subset before reaching `dispatcher.invoke`, so it does NOT appear in `tool_calls`; the run still completes (soft-deny):

```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": {},
  "_note": "notify's cap_subset is empty, so the worker's `page` (net.http) call is denied by AttenuatedDispatcher before dispatch. Contrast fixture 04 (notify grants net.http -> page:1). Soft-deny: the worker receives an is_error tool result and completes. Denial is also emitted as tracing event runtime.subflow.attenuation_denied."
}
```

- [ ] **Step 4: Run the new fixture under conformance (dev vs bundle multiset)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
Expected: PASS including the new `19_*` fixture; dev-mode and bundle-mode produce the identical `{ Completed, tool_calls: {} }` multiset — proving the attenuation denial is behaviorally identical across runtimes (the decorator lives in shared `tau-runtime-core`). If fixtures are enumerated manually (a `for` list rather than a glob), add `19_subflow_attenuation_denied` to that list in `src/lib.rs`.

- [ ] **Step 5: Note wasm parity**

The wasm guest (`tau-wasm-guest`) drives the same `tau-runtime-core` interpreter, so it inherits the AttenuatedDispatcher identically. A dedicated wasm-execution conformance lane is gated on separate CI work (per project memory "PR-G wasm-execution CI lane") and is **out of scope** here; parity is by-construction (shared no_std core), not by a new wasm test in this plan. State this in the PR body.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/workflow.toml \
        crates/tau-ir-conformance/fixtures/19_subflow_attenuation_denied/ \
        crates/tau-ir-conformance/src/lib.rs
git commit -m "test(tau-ir-conformance): subflow attenuation fixtures (proper narrowing + denial)"
```

---

## Final verification

- [ ] Run the affected crates' suites:
  `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core --features test-fixtures` and `... -p tau-ir-conformance`, plus `cargo test -p tau-runtime-core --doc`.
- [ ] `cargo clippy` and `cargo fmt --check` on the touched crates (per repo timeouts).
- [ ] Open the PR from a fresh `feat/subflow-runtime-attenuation` branch (current branch `cap-subset-typecheck` is a stale name from the discarded static-half plan). PR body: reference Decision D5-C (runtime half) / D1-C, link the spec, and note (a) no IR format bump, (b) wasm parity is by-construction, (c) fixture 04 semantics changed (now models proper narrowing).

## Notes on decisions already locked (do not re-litigate)

- cap_subset = subflow tool's existing `capabilities` (no IR field, no format bump).
- Enforcement = gate-by-composition (exact meet, no materialized glob intersection); root agent ungated (`meet(⊤, C)=C`).
- Denial = soft (`is_error` tool result), reuses `CapabilityDenial` + `narrowing_frame`, emits `tracing::warn!(name = "runtime.subflow.attenuation_denied", …)`.
- Rejected: materialized meet / `granted_capabilities_override` reuse (spec Appendix A).
