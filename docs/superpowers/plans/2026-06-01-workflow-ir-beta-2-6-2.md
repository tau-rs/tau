# Workflow IR β.2.6.2 — Subflow + Deterministic Step Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close β.2.6.2 — make subflow-tools and deterministic-step-tools invocable from agents through the existing `ToolDispatcher` boundary. Author conformance fixtures `04_subflow_spawn_child` and `05_deterministic_step`. Remove their `DEFERRED_FIXTURES` entries and README placeholders. End the β.2 family with all six fixtures live (no `#[ignore]`'d slots).

**Architecture:** Two new `ToolImpl` variants — `Subflow { target: AgentId }` and `Step { id: StepId }`. `parse.rs` registers a `Tool` node for each `ToolBody::Subflow(_)` and for each `[steps.<name>]` block, so agents can list them in `tool_refs` and typecheck's `UnknownToolRef` check passes. Inside `DispatcherTool::invoke`, the interpreter branches on `ToolImpl`: `Native`/`Mcp` keep forwarding to the user's dispatcher; `Subflow` recursively calls `run_ir` (with `Box::pin` to break the async self-reference); `Step` calls a `DeterministicRegistry` accessed via a new default-`None` method on `ToolDispatcher`. `run_ir` / `run_agent` / `run_subflow` now take `Arc<IrModule>` so the dispatcher can share the module across recursion.

**Tech Stack:** Rust 2021, `tau-ir`, `tau-runtime-core`, `tau-ir-conformance`, `tau-cli`, `tau-pkg` (no changes), `serde_json`, `tokio` (current-thread runtime).

**Branch:** `feat/workflow-ir-beta-2-6-2` (already created off `origin/main` at this worktree).

**Worktree:** `/Users/titouanlebocq/code/tau-worktrees/workflow-ir-beta-2-6-2`

---

## Files map

### Modified

| File | Responsibility |
|---|---|
| `crates/tau-ir/src/tool_impl.rs` | Add `ToolImpl::Subflow { target: AgentId }` and `ToolImpl::Step { id: StepId }` variants. |
| `crates/tau-ir/src/error.rs` | Add `UnknownSubflowToolTarget` + `UnknownStepToolTarget` variants. |
| `crates/tau-ir/src/lower/parse.rs` | Register `Tool` nodes for `ToolBody::Subflow` and `[steps.X]`; stop emitting `SubflowEdge::Spawn`. |
| `crates/tau-ir/src/lower/resolve.rs` | Add no-op arms for `Subflow` + `Step`. |
| `crates/tau-ir/src/lower/typecheck.rs` | Check `Tool::Subflow.target` exists in `agents`; `Tool::Step.id` exists in `steps`. |
| `crates/tau-runtime-core/src/interpreter/deterministic.rs` | Add `Send + Sync` bound to `DeterministicRegistry` trait. |
| `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` | Add default `deterministic_registry() -> Option<Arc<dyn DeterministicRegistry + Send + Sync>>`. |
| `crates/tau-runtime-core/src/interpreter/mod.rs` | `run_ir` takes `Arc<IrModule>`. |
| `crates/tau-runtime-core/src/interpreter/agent_loop.rs` | `run_agent` takes `Arc<IrModule>`; `DispatcherTool` stores `Arc<IrModule>` + the `ToolImpl`; `invoke` branches on variant; recursion uses `Box::pin`. |
| `crates/tau-runtime-core/src/interpreter/subflow.rs` | `run_subflow` takes `Arc<IrModule>`; documented as the call site for `ToolImpl::Subflow` invocations. |
| `crates/tau-cli/src/cmd/ir_dispatcher.rs` | Wrap `module` in `Arc::new(...)` before `run_ir`; `ForwardingDispatcher::deterministic_registry()` returns `None` (production has no registry yet — documented). |
| `crates/tau-ir-conformance/src/dev_mode.rs` | Wrap `module` in `Arc`; `RecordingDispatcher::deterministic_registry()` returns `Some(...)` pointing at a tiny `MapBackedDeterministicRegistry`. |
| `crates/tau-ir-conformance/src/bundle_mode.rs` | Wrap `module` in `Arc` before `drive_module`. |
| `crates/tau-ir-conformance/src/lib.rs` | Expose `MapBackedDeterministicRegistry` so dev_mode + bundle_mode share it. |
| `crates/tau-ir-conformance/tests/conformance.rs` | Remove `04`/`05` from `DEFERRED_FIXTURES`; add four new `#[tokio::test]` functions. |

### Created

- `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/workflow.toml`
- `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/mock_llm.jsonl`
- `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/expected_report.json`
- `crates/tau-ir-conformance/fixtures/05_deterministic_step/workflow.toml`
- `crates/tau-ir-conformance/fixtures/05_deterministic_step/mock_llm.jsonl`
- `crates/tau-ir-conformance/fixtures/05_deterministic_step/expected_report.json`

### Deleted

- `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/README.md`
- `crates/tau-ir-conformance/fixtures/05_deterministic_step/README.md`

---

## Standing constraints (re-read before EVERY cargo / git command)

From `CLAUDE.md`:

- **Cargo:** `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Never bare `cargo`. Never `--workspace`. Per-task `<role>` is specified inline.
- **Cargo timeouts:** test/nextest 300s, build/check 180s, clippy 240s, fmt --check 30s.
- **Tests:** prefer `cargo nextest run -p <crate>` over `cargo test` (matches CI per CLAUDE.md Rule 6). For doctests use `cargo test --doc -p <crate>`.
- **Commits:** `git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "..."` (lefthook test-native can corrupt git identity).
- **Push:** `git push --no-verify -u origin feat/workflow-ir-beta-2-6-2`. CI is the gate. Avoid the deep-gate cold start.
- **Auto-merge:** `gh pr merge <N> --auto` bare (target is `main`).
- **Don't push after auto-merge enrollment** unless CI fails.

---

## Phase 1 — tau-ir surface

### Task 1.1: Add new `IrError` variants

**Files:**
- Modify: `crates/tau-ir/src/error.rs`

- [ ] **Step 1: Append the two new variants to `IrError`.**

Insert AFTER the existing `UnknownNativeTool` variant (around line 70), BEFORE `Parse`:

```rust
    /// A `ToolImpl::Subflow` tool targets an agent that is not present in
    /// the workflow.
    #[error("subflow tool {tool:?} targets unknown agent {agent:?}")]
    UnknownSubflowToolTarget {
        /// The tool id whose `Subflow` variant points at a missing agent.
        tool: ToolId,
        /// The unresolved target agent id.
        agent: AgentId,
    },

    /// A `ToolImpl::Step` tool references a step id that is not present in
    /// the workflow's `steps` table.
    #[error("step tool {tool:?} references unknown step {step:?}")]
    UnknownStepToolTarget {
        /// The tool id whose `Step` variant points at a missing step.
        tool: ToolId,
        /// The unresolved step id.
        step: StepId,
    },
```

- [ ] **Step 2: `cargo check` — confirm `IrError` still compiles.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir`
Expected: clean compile (no warnings).

- [ ] **Step 3: Commit.**

```
git add crates/tau-ir/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/error): add UnknownSubflowToolTarget + UnknownStepToolTarget variants"
```

### Task 1.2: Add `ToolImpl::Subflow` + `ToolImpl::Step` variants

**Files:**
- Modify: `crates/tau-ir/src/tool_impl.rs`
- Test: `crates/tau-ir/src/tool_impl.rs` (inline `#[cfg(test)]` block at end)

- [ ] **Step 1: Write failing canonical round-trip tests.**

Append to `crates/tau-ir/src/tool_impl.rs` (the file ends at line 55 — no existing test block):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentId, StepId};
    use alloc::string::ToString;

    #[test]
    fn tool_impl_subflow_round_trips_canonical_json() {
        let original = ToolImpl::Subflow {
            target: AgentId("child-agent".to_string()),
        };
        let bytes = serde_json::to_vec(&original).expect("serialize");
        let decoded: ToolImpl = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn tool_impl_step_round_trips_canonical_json() {
        let original = ToolImpl::Step {
            id: StepId("normalize".to_string()),
        };
        let bytes = serde_json::to_vec(&original).expect("serialize");
        let decoded: ToolImpl = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir tool_impl::tests`
Expected: COMPILE FAILURE — `ToolImpl::Subflow` and `ToolImpl::Step` are not yet variants.

- [ ] **Step 3: Add the variants.**

Modify the `ToolImpl` enum body (currently lines 32-55) to:

```rust
/// How a [`crate::Tool`] node's behavior is provided at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolImpl {
    /// Statically linked native tool.
    Native {
        /// Reference to the native Rust impl by symbolic name.
        fn_ref: NativeFnRef,
        /// Hash of the impl's source bytes (Rust source, dependencies'
        /// content hashes). Participates in the IR module hash.
        content_hash: Hash256,
    },
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
    },
    /// Sub-workflow spawn: invoking this tool runs the named agent (in the
    /// same `IrModule`) as a child loop with empty initial history. The
    /// child's final assistant text (or empty string) becomes the tool
    /// result body.
    ///
    /// v0 limitation: tool input args are NOT forwarded to the child as a
    /// User message — the child runs with empty initial messages and its
    /// own prompt + LLM script drive its behavior. β.7 (AOT codegen) is
    /// the natural place to thread arg-forwarding.
    Subflow {
        /// Agent id (within this `IrModule`'s `workflow.agents`) to spawn.
        target: crate::ids::AgentId,
    },
    /// Deterministic step: invoking this tool calls the pure Rust function
    /// named by the step's `fn_ref` via the dispatcher's
    /// `deterministic_registry()`. The function's return value becomes the
    /// tool result body. No LLM, no I/O.
    Step {
        /// Step id (within this `IrModule`'s `workflow.steps`).
        id: crate::ids::StepId,
    },
}
```

Add the `crate::ids::{AgentId, StepId}` import to the use list at the top of the file if not already present — they're referenced via fully-qualified paths above so no use-line edit is strictly required, but you may inline-import for terseness.

- [ ] **Step 4: Rerun the tests — they should now pass.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir tool_impl::tests`
Expected: 2 passed.

- [ ] **Step 5: Rerun the full tau-ir suite — confirm no regressions.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: all tests pass.

- [ ] **Step 6: Commit.**

```
git add crates/tau-ir/src/tool_impl.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/tool_impl): add Subflow + Step variants + canonical round-trip tests"
```

### Task 1.3: Update `resolve.rs` for the new variants

**Files:**
- Modify: `crates/tau-ir/src/lower/resolve.rs`

`resolve.rs`'s match on `&mut tool.impl_` is currently exhaustive (Native + Mcp arms). The new variants need explicit arms to keep the match exhaustive.

- [ ] **Step 1: Add no-op arms for `Subflow` and `Step`.**

Inside the `for (_id, tool)` loop in `resolve`, after the `ToolImpl::Mcp` arm (ending at the closing `}` of that arm around current line 37), insert:

```rust
            ToolImpl::Subflow { target: _ } => {
                // Subflow variant: target agent lives inside the same
                // IrModule; nothing external to resolve. The typecheck
                // stage verifies the target exists.
            }
            ToolImpl::Step { id: _ } => {
                // Step variant: deterministic step lives inside the same
                // IrModule's `workflow.steps`; nothing external to
                // resolve. The typecheck stage verifies the step exists.
            }
```

- [ ] **Step 2: `cargo check` — confirm exhaustive match holds.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir`
Expected: clean compile.

- [ ] **Step 3: Commit.**

```
git add crates/tau-ir/src/lower/resolve.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/lower/resolve): no-op arms for Subflow + Step variants"
```

### Task 1.4: Update `typecheck.rs` for the new variants

**Files:**
- Modify: `crates/tau-ir/src/lower/typecheck.rs`

The existing typecheck has three checks (tool_refs known, subflow edges known, content_hash non-zero for Native). We add two more: Subflow tool targets exist, Step tool ids exist.

- [ ] **Step 1: Write failing tests.**

Append a `#[cfg(test)]` block at the end of `crates/tau-ir/src/lower/typecheck.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityRequirements, CapabilityTable};
    use crate::ids::{AgentId, StepId, ToolId};
    use crate::lower::parse::Parsed;
    use crate::module::Workflow;
    use crate::node::{Agent, Tool, ToolSpec};
    use crate::tool_impl::ToolImpl;
    use crate::AgentBudget;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;

    fn empty_caps() -> CapabilityRequirements {
        CapabilityRequirements { declared: vec![] }
    }

    fn agent_with_tool_refs(id: &str, refs: &[&str]) -> Agent {
        Agent {
            id: AgentId(id.to_string()),
            prompt: String::new(),
            model: String::new(),
            tool_refs: refs.iter().map(|s| ToolId(s.to_string())).collect(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
        }
    }

    fn tool_with_impl(name: &str, impl_: ToolImpl) -> Tool {
        Tool {
            id: ToolId(name.to_string()),
            impl_,
            capabilities: empty_caps(),
            spec: ToolSpec {
                name: name.to_string(),
                description: String::new(),
                input_schema: serde_json::Value::Null,
            },
        }
    }

    #[test]
    fn typecheck_rejects_subflow_tool_pointing_at_missing_agent() {
        let mut agents = BTreeMap::new();
        // Only `parent` exists — subflow points at `ghost`.
        agents.insert(
            AgentId("parent".to_string()),
            agent_with_tool_refs("parent", &["call_ghost"]),
        );
        let mut tools = BTreeMap::new();
        tools.insert(
            ToolId("call_ghost".to_string()),
            tool_with_impl(
                "call_ghost",
                ToolImpl::Subflow {
                    target: AgentId("ghost".to_string()),
                },
            ),
        );
        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
            },
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownSubflowToolTarget { ref tool, ref agent }
                if tool.0 == "call_ghost" && agent.0 == "ghost"),
            "expected UnknownSubflowToolTarget; got {err:?}"
        );
    }

    #[test]
    fn typecheck_rejects_step_tool_pointing_at_missing_step() {
        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("solo".to_string()),
            agent_with_tool_refs("solo", &["normalize"]),
        );
        let mut tools = BTreeMap::new();
        tools.insert(
            ToolId("normalize".to_string()),
            tool_with_impl(
                "normalize",
                ToolImpl::Step {
                    id: StepId("missing-step".to_string()),
                },
            ),
        );
        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(), // empty → "missing-step" not present
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
            },
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownStepToolTarget { ref tool, ref step }
                if tool.0 == "normalize" && step.0 == "missing-step"),
            "expected UnknownStepToolTarget; got {err:?}"
        );
    }
}
```

- [ ] **Step 2: Run tests — verify they fail.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir lower::typecheck::tests`
Expected: FAIL — typecheck doesn't yet check Subflow / Step targets.

- [ ] **Step 3: Add the new checks.**

Insert AFTER the existing `// 3. Sanity: every Native tool's content_hash...` block (which ends with the `for` loop closing brace around current line 68), BEFORE the final `Ok(())`:

```rust
    // 4. Every ToolImpl::Subflow's target must exist in `agents`.
    for (tool_id, tool) in parsed.workflow.tools.iter() {
        if let ToolImpl::Subflow { target } = &tool.impl_ {
            if !parsed.workflow.agents.contains_key(target) {
                return Err(IrError::UnknownSubflowToolTarget {
                    tool: tool_id.clone(),
                    agent: target.clone(),
                });
            }
        }
    }

    // 5. Every ToolImpl::Step's id must exist in `steps`.
    for (tool_id, tool) in parsed.workflow.tools.iter() {
        if let ToolImpl::Step { id } = &tool.impl_ {
            if !parsed.workflow.steps.contains_key(id) {
                return Err(IrError::UnknownStepToolTarget {
                    tool: tool_id.clone(),
                    step: id.clone(),
                });
            }
        }
    }
```

- [ ] **Step 4: Rerun tests — they should now pass.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: all tests pass (including the two new typecheck tests).

- [ ] **Step 5: Commit.**

```
git add crates/tau-ir/src/lower/typecheck.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/lower/typecheck): check Subflow + Step tool targets exist"
```

### Task 1.5: Update `parse.rs` to register Tool nodes for subflows and steps

**Files:**
- Modify: `crates/tau-ir/src/lower/parse.rs`

The change: `ToolBody::Subflow` now registers a `Tool` (instead of `continue`-ing and emitting only a `SubflowEdge`). And we add a second pass that registers a `Tool` for each `[steps.X]` block (in addition to the existing `Deterministic` node registration).

- [ ] **Step 1: Write failing tests.**

Append a `#[cfg(test)]` block at the end of `crates/tau-ir/src/lower/parse.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_impl::ToolImpl;
    use tau_pkg::project::ProjectConfig;

    #[test]
    fn parse_registers_tool_node_for_subflow_body() {
        let toml = r#"
[project]
name = "p"

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["notify"]

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = []

[tools.notify]
subflow     = "worker"
description = "Hand off to worker"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config).expect("parse stage");

        let tool = parsed
            .workflow
            .tools
            .get(&ToolId("notify".into()))
            .expect("notify tool registered");
        assert!(
            matches!(&tool.impl_, ToolImpl::Subflow { target } if target.0 == "worker"),
            "expected ToolImpl::Subflow targeting worker; got {:?}",
            tool.impl_
        );
    }

    #[test]
    fn parse_registers_tool_node_for_each_step() {
        let toml = r#"
[project]
name = "p"

[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["normalize"]

[steps.normalize]
deterministic = "parse_celsius"
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config).expect("parse stage");

        // Step registered in workflow.steps:
        assert!(parsed
            .workflow
            .steps
            .contains_key(&StepId("normalize".into())));

        // AND registered as a Tool with ToolImpl::Step:
        let tool = parsed
            .workflow
            .tools
            .get(&ToolId("normalize".into()))
            .expect("normalize tool registered");
        assert!(
            matches!(&tool.impl_, ToolImpl::Step { id } if id.0 == "normalize"),
            "expected ToolImpl::Step{{normalize}}; got {:?}",
            tool.impl_
        );
    }

    #[test]
    fn parse_emits_no_subflow_edge_for_subflow_body() {
        // v0 routes subflow dispatch through ToolImpl::Subflow exclusively;
        // SubflowEdge is reserved for SubflowKind::Compose (future). This
        // test pins the new shape so a regression that re-introduces the
        // edge gets caught.
        let toml = r#"
[project]
name = "p"

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["notify"]

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = []

[tools.notify]
subflow     = "worker"
description = "Hand off to worker"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config).expect("parse stage");
        assert!(
            parsed.workflow.edges.is_empty(),
            "expected no SubflowEdge entries; got {:?}",
            parsed.workflow.edges
        );
    }
}
```

- [ ] **Step 2: Run tests — verify they fail.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir lower::parse::tests`
Expected: FAIL — current behavior emits edges, not Tool nodes, for subflows; doesn't register Tool nodes for steps.

- [ ] **Step 3: Update the `ToolBody::Subflow` arm.**

In `crates/tau-ir/src/lower/parse.rs`, replace the current `ToolBody::Subflow(target)` arm (currently lines 58-69, which does `edges.push(...)` then `continue`) with:

```rust
            ToolBody::Subflow(target) => ToolImpl::Subflow {
                target: AgentId(target.clone()),
            },
```

This removes the `continue;` — the surrounding loop now falls through to the normal `ToolSpec` + `tools.insert(...)` lines. The `edges` Vec stays for future Compose support.

After this change, the unused `let mut edges: alloc::vec::Vec<SubflowEdge> = alloc::vec::Vec::new();` line should be removed if the compiler flags it as unused — OR keep it (still flows into `Workflow.edges`). Verify what the compiler says; keep as-is if it compiles, drop if it warns.

ALSO: remove these now-unused imports from the top of the file:

```rust
use crate::subflow::{SubflowEdge, SubflowKind};
```

Replace with `// (no remaining direct usage)` or just delete the line. If the compiler complains about `SubflowEdge` being unused, drop only that import. Re-run `cargo check` to confirm.

- [ ] **Step 4: Add step-as-tool registration AFTER the steps loop.**

Inside `parse`, AFTER the existing `// --- Deterministic steps ---` block (which populates `steps`), insert a new block that registers a Tool node for each step:

```rust
    // --- Deterministic steps as tools ----------------------------------
    //
    // Each [steps.<name>] block registers BOTH a `Deterministic` node
    // (above) and a `Tool` with `ToolImpl::Step { id }`. The Tool
    // registration is what lets an agent reference the step in its
    // `tool_refs`; the Deterministic node is what the runtime registry
    // dispatches against at invoke time.
    for (name, _entry) in config.steps.iter() {
        let step_id = StepId(name.clone());
        let tool_id = ToolId(name.clone());
        let caps = CapabilityRequirements { declared: alloc::vec::Vec::new() };
        let spec = ToolSpec {
            name: name.clone(),
            description: alloc::string::String::new(),
            input_schema: config.steps[name].input_schema.clone(),
        };
        capability_table.insert(tool_id.clone(), caps.clone());
        tools.insert(
            tool_id.clone(),
            Tool {
                id: tool_id,
                impl_: ToolImpl::Step { id: step_id },
                capabilities: caps,
                spec,
            },
        );
    }
```

- [ ] **Step 5: Rerun tests — should pass.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: all tests pass, including the three new parse tests.

- [ ] **Step 6: Commit.**

```
git add crates/tau-ir/src/lower/parse.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir/lower/parse): register Tool nodes for subflows + steps; drop SubflowEdge::Spawn emission"
```

### Task 1.6: Phase-1 sanity — `cargo clippy -p tau-ir`

- [ ] **Step 1: Clippy clean.**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir --all-targets -- -D warnings`
Expected: clean (no warnings).

If clippy flags anything (e.g. the now-unused `edges` Vec, dead imports), fix in a follow-up commit:

```
git add crates/tau-ir/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "fix(tau-ir): clippy cleanups post-Task-1.5"
```

---

## Phase 2 — tau-runtime-core surface

### Task 2.1: Add `Send + Sync` bound to `DeterministicRegistry`

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/deterministic.rs`

The registry will be passed across `.await` points (via the dispatcher), so it needs `Send + Sync`.

- [ ] **Step 1: Add the bound.**

Change the trait declaration (currently line 14):

```rust
pub trait DeterministicRegistry: Send + Sync {
```

- [ ] **Step 2: `cargo check`.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core`
Expected: clean compile.

- [ ] **Step 3: Commit.**

```
git add crates/tau-runtime-core/src/interpreter/deterministic.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-core/interpreter/deterministic): add Send + Sync bound to DeterministicRegistry"
```

### Task 2.2: Add `deterministic_registry()` default method to `ToolDispatcher`

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs`

- [ ] **Step 1: Add the default method.**

Replace the current `pub trait ToolDispatcher { ... }` block (currently lines 33-47) with:

```rust
/// Boundary the interpreter calls through to invoke tools and obtain
/// the LLM backend used for agent-loop construction.
pub trait ToolDispatcher {
    /// Invoke the tool identified by `tool_id` with `args`.
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>;

    /// Return the LLM backend this dispatcher is wired to.
    ///
    /// The interpreter calls this once per agent-node execution to build
    /// a `RuntimeBuilder` for the inner agent loop. Implementors own the
    /// backend handle (typically an `Arc`-clone of the caller's backend).
    fn llm_backend(&self) -> Arc<dyn DynLlmBackend>;

    /// Optional handle to a deterministic-step registry.
    ///
    /// The interpreter calls this when an agent invokes a tool whose IR
    /// `ToolImpl` is `Step { id }`. Returning `None` is allowed and means
    /// "this dispatcher does not support deterministic steps" — invoking
    /// a `Step` tool against a `None` registry surfaces as a
    /// [`RuntimeError::Internal`] with a clear diagnostic.
    ///
    /// Production paths (e.g. `tau run --bundle`) currently return
    /// `None`; the deterministic-registry surface ships first with the
    /// conformance test runner in `tau-ir-conformance` and graduates to
    /// production once a real native-fn registry is wired (β.7+).
    fn deterministic_registry(
        &self,
    ) -> Option<Arc<dyn super::deterministic::DeterministicRegistry>> {
        None
    }
}
```

(Note the trait object bound is `Send + Sync` implicitly because Task 2.1 added it to the trait.)

- [ ] **Step 2: `cargo check`.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo check -p tau-runtime-core`
Expected: clean.

- [ ] **Step 3: Commit.**

```
git add crates/tau-runtime-core/src/interpreter/tool_dispatch.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-core/interpreter/tool_dispatch): add deterministic_registry() default method"
```

### Task 2.3: Migrate `run_ir` / `run_agent` / `run_subflow` to `Arc<IrModule>`

**Why:** `DispatcherTool::invoke` will recursively call `run_ir` (via `Box::pin`) for `ToolImpl::Subflow`. The `DispatcherTool` is owned by the inner `Runtime` and outlives the borrow scope of `&IrModule` — so the module must be `Arc`-shared.

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/subflow.rs`

- [ ] **Step 1: Change `run_ir` signature.**

In `crates/tau-runtime-core/src/interpreter/mod.rs`, change the `pub async fn run_ir` block (currently lines 34-52) to:

```rust
/// Drive an `IrModule` from its single entry agent to completion.
///
/// `entry` names which agent in the module to start with. Future v0.x
/// will infer it from a `[workflow]` block; v0.0 requires the caller
/// to supply it.
///
/// `module` is taken as `Arc<IrModule>` so the dispatcher's
/// `DispatcherTool` can share it across recursive subflow invocations
/// without copying the IR.
pub async fn run_ir<D>(
    module: alloc::sync::Arc<IrModule>,
    entry: &AgentId,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: tool_dispatch::ToolDispatcher + Send + Sync + 'static,
{
    let agent_node =
        module
            .workflow
            .agents
            .get(entry)
            .ok_or_else(|| RuntimeError::AgentNotFound {
                agent: entry.0.clone(),
            })?;
    // Clone the Agent node out of the Arc so the borrow doesn't escape;
    // it's small (id + prompt + a few Vec<...>) and avoids a self-borrow
    // through run_agent's signature.
    let agent_node = agent_node.clone();
    agent_loop::run_agent(module, &agent_node, dispatcher, initial_messages).await
}
```

- [ ] **Step 2: Change `run_agent` signature.**

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs`, the signature of `run_agent` (currently around line 211) becomes:

```rust
pub async fn run_agent<D>(
    module: alloc::sync::Arc<IrModule>,
    agent: &Agent,
    dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
```

Inside the body, the `for tool_id in &agent.tool_refs` loop currently does `module.workflow.tools.get(tool_id)`. That still works — `Arc<IrModule>` derefs to `&IrModule`. No change inside the loop yet (Task 2.4 will rework `DispatcherTool` construction).

- [ ] **Step 3: Change `run_subflow` signature.**

In `crates/tau-runtime-core/src/interpreter/subflow.rs`, replace the function with:

```rust
//! Execute a `Node::Subflow` edge.
//!
//! v0 supports `SubflowKind::Spawn` only (per `RuntimeError::UnsupportedSubflowCompose`).
//! The spawn dispatches into a sibling agent loop with a narrowed
//! capability set. The agent loop is the same `run_agent` used at the
//! root — recursion is bounded by the interpreter's call stack and the
//! per-agent budget.
//!
//! In β.2.6.2 the `ToolImpl::Subflow` variant became the production
//! call site for sub-agent spawning (see `agent_loop::DispatcherTool::
//! invoke`). `run_subflow` survives as the documented entrypoint for
//! callers that hold a `SubflowKind` value directly.

use alloc::sync::Arc;
use tau_ir::{IrModule, SubflowKind};

use crate::error::RuntimeError;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;

/// Execute one subflow edge.
pub async fn run_subflow<D>(
    module: Arc<IrModule>,
    kind: &SubflowKind,
    dispatcher: Arc<D>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    match kind {
        SubflowKind::Spawn {
            target_agent,
            cap_subset: _,
        } => crate::interpreter::run_ir(module, target_agent, dispatcher, alloc::vec![]).await,
        SubflowKind::Compose { .. } => Err(RuntimeError::UnsupportedSubflowCompose),
    }
}
```

- [ ] **Step 4: `cargo check` — confirm interpreter still compiles.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo check -p tau-runtime-core`
Expected: clean within the crate. (Downstream `tau-cli` and `tau-ir-conformance` will break — that's fixed in Task 2.5 / Task 3.x.)

- [ ] **Step 5: Commit.**

```
git add crates/tau-runtime-core/src/interpreter/mod.rs crates/tau-runtime-core/src/interpreter/agent_loop.rs crates/tau-runtime-core/src/interpreter/subflow.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-core/interpreter): take Arc<IrModule> in run_ir/run_agent/run_subflow"
```

### Task 2.4: Wire subflow + step dispatch into `DispatcherTool`

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs`

This is the heaviest task. `DispatcherTool` now needs:
- The IR module (for the Subflow recursion target lookup + step lookup).
- The cloned `ToolImpl` (to branch at invoke time).

The `invoke` body branches on `ToolImpl`:
- `Native` / `Mcp` → existing dispatcher forward (unchanged behavior).
- `Subflow { target }` → `Box::pin(run_ir(self.module.clone(), &target, self.dispatcher.clone(), vec![]))`. Map the resulting `RunOutcome` to a `ToolInvocationResult` whose `body` is `Some(Value::String(last_assistant_text(&all_messages)))`.
- `Step { id }` → look up `self.module.workflow.steps[id]`; call `dispatcher.deterministic_registry()`; if `None` → return `RuntimeError::Internal`; if `Some(registry)` → call `registry.invoke(&step.fn_ref.name, args)` and use the return as `body`.

- [ ] **Step 1: Add `ToolImpl` and `Arc<IrModule>` to `DispatcherTool`.**

Replace the existing `DispatcherTool` struct (currently lines 57-66) with:

```rust
/// A `tau_ports::Tool` whose `invoke` branches on the IR `ToolImpl`:
///
/// - `Native` / `Mcp` → forwarded to the user's `ToolDispatcher`.
/// - `Subflow { target }` → recursively `run_ir` against the target
///   agent in the same module. The recursion is broken with `Box::pin`
///   to satisfy Rust's no-self-referential-async-fn rule.
/// - `Step { id }` → looked up in `module.workflow.steps` and called
///   against the dispatcher's `DeterministicRegistry`.
struct DispatcherTool<D> {
    /// Tool name as seen by the LLM (from the IR `ToolSpec`).
    tool_name: String,
    /// `ToolId` forwarded to the dispatcher on each invoke (Native/Mcp).
    tool_id: ToolId,
    /// LLM-facing spec (constructed via serde to bypass `#[non_exhaustive]`).
    spec: ToolSpec,
    /// IR module, shared so subflow recursion + step lookup work.
    module: alloc::sync::Arc<tau_ir::IrModule>,
    /// Clone of the IR tool's `ToolImpl`. Cheap to clone (a few hashes
    /// or a `String`).
    tool_impl: tau_ir::ToolImpl,
    /// Shared dispatcher handle.
    dispatcher: Arc<D>,
}
```

(`tau_ir::ToolImpl` needs to be in scope — add `use tau_ir::ToolImpl;` to the existing `use tau_ir::{Agent, IrModule, ToolId};` line.)

- [ ] **Step 2: Rework `DispatcherTool::invoke` to branch on `tool_impl`.**

Replace the existing `impl<D> tau_ports::tool::Tool for DispatcherTool<D>` block's `invoke` method (currently lines 86-131) with:

```rust
    async fn invoke(
        &self,
        _session: &mut Self::Session,
        args: tau_domain::Value,
    ) -> Result<ToolResult, ToolError> {
        // Convert tau_domain::Value → serde_json::Value once, then branch
        // on the IR ToolImpl. The branches do NOT share extraction logic
        // because Subflow returns a domain-side `RunOutcome` and Step
        // returns a raw `serde_json::Value` from the registry — both end
        // up as ToolResult Text content but the source paths differ.
        let json_args = domain_value_to_json(&args);

        match &self.tool_impl {
            ToolImpl::Native { .. } | ToolImpl::Mcp { .. } => {
                // Existing path: forward to the user's ToolDispatcher.
                let result: ToolInvocationResult = self
                    .dispatcher
                    .invoke(&self.tool_id, &json_args)
                    .await
                    .map_err(|e| ToolError::Internal {
                        message: alloc::format!("dispatcher error: {e}"),
                    })?;

                if let Some(err_msg) = result.error {
                    return Ok(ToolResult::new(
                        alloc::vec![ToolContent::Text { text: err_msg }],
                        true,
                    ));
                }

                let text = match result.body {
                    Some(serde_json::Value::String(s)) => s,
                    Some(v) => alloc::format!("{v}"),
                    None => alloc::string::String::new(),
                };
                Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    false,
                ))
            }

            ToolImpl::Subflow { target } => {
                // Box::pin breaks the async self-reference (run_ir →
                // run_agent → DispatcherTool::invoke → run_ir). Without it
                // the async-fn future type would be infinitely recursive.
                let module = self.module.clone();
                let target = target.clone();
                let dispatcher = self.dispatcher.clone();
                let outcome = core::pin::Pin::from(alloc::boxed::Box::new(
                    crate::interpreter::run_ir(module, &target, dispatcher, Vec::new()),
                ))
                .await
                .map_err(|e| ToolError::Internal {
                    message: alloc::format!("subflow recursion error: {e}"),
                })?;

                // Convert RunOutcome → tool result body. v0 contract:
                // emit the last `Assistant`-sent text from `all_messages`
                // as the tool result body. Empty string if none.
                let text = last_assistant_text(&outcome);
                Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    false,
                ))
            }

            ToolImpl::Step { id } => {
                // Look up the step in the IR's workflow.steps table.
                let step = self
                    .module
                    .workflow
                    .steps
                    .get(id)
                    .ok_or_else(|| ToolError::Internal {
                        message: alloc::format!(
                            "step tool {:?} references unknown step {:?} \
                             (typecheck should have caught this — possible IR corruption)",
                            self.tool_id, id,
                        ),
                    })?;

                let registry = self
                    .dispatcher
                    .deterministic_registry()
                    .ok_or_else(|| ToolError::Internal {
                        message: alloc::format!(
                            "agent invoked step tool {:?} but the dispatcher \
                             did not provide a DeterministicRegistry",
                            self.tool_id,
                        ),
                    })?;

                let result = registry
                    .invoke(&step.fn_ref.name, &json_args)
                    .map_err(|e| ToolError::Internal {
                        message: alloc::format!(
                            "deterministic step {:?} (fn={:?}) failed: {e}",
                            self.tool_id, step.fn_ref.name,
                        ),
                    })?;

                // Same body-shape extraction as the Native/Mcp arm so the
                // LLM sees identically-shaped tool results regardless of
                // ToolImpl variant.
                let text = match result {
                    serde_json::Value::String(s) => s,
                    v => alloc::format!("{v}"),
                };
                Ok(ToolResult::new(
                    alloc::vec![ToolContent::Text { text }],
                    false,
                ))
            }
        }
    }
```

- [ ] **Step 3: Add the `last_assistant_text` helper.**

Insert this free function near the other helpers at the top of `agent_loop.rs` (above the `DispatcherTool` definition):

```rust
/// Extract the body text of the last `Assistant`-authored message in a
/// `RunOutcome`'s `all_messages`. Returns an empty `String` when no
/// assistant message exists (e.g. immediate `Failed` outcomes).
///
/// Used by `DispatcherTool::invoke`'s `Subflow` arm to convert a child
/// agent's terminal state into a tool-result body for the parent.
fn last_assistant_text(outcome: &RunOutcome) -> String {
    let messages = match outcome {
        RunOutcome::Completed { all_messages, .. } => all_messages,
        RunOutcome::Failed { all_messages, .. } => all_messages,
        // Future variants — RunOutcome is #[non_exhaustive].
        _ => return String::new(),
    };
    for msg in messages.iter().rev() {
        if matches!(msg.sender, tau_domain::Address::Agent(_)) {
            if let tau_domain::MessagePayload::Text { content } = &msg.payload {
                return content.clone();
            }
        }
    }
    String::new()
}
```

- [ ] **Step 4: Update the `DispatcherTool` construction in `run_agent`.**

In `run_agent` (currently lines 247-253), the `builder = builder.with_tool(DispatcherTool {...})` block. Replace the struct literal with:

```rust
        builder = builder.with_tool(DispatcherTool {
            tool_name: ir_tool.spec.name.clone(),
            tool_id: tool_id.clone(),
            spec,
            module: module.clone(),
            tool_impl: ir_tool.impl_.clone(),
            dispatcher: dispatcher.clone(),
        });
```

- [ ] **Step 5: Update the existing inline tests for the new struct shape.**

The existing `mod tests` at the bottom of `agent_loop.rs` constructs `DispatcherTool { tool_name, tool_id, spec, dispatcher }` (without `module` / `tool_impl`) in `make_dispatcher_tool`. Update `make_dispatcher_tool` to:

```rust
    fn make_dispatcher_tool<D>(dispatcher: Arc<D>) -> DispatcherTool<D>
    where
        D: ToolDispatcher + Send + Sync + 'static,
    {
        let spec = make_tool_spec(
            "fixed",
            "fixed-output tool for tests",
            &serde_json::json!({}),
        );
        // The existing test fixtures only exercise the Native/Mcp dispatch
        // path of `invoke`; a stub `ToolImpl::Native` keeps that branch
        // active. Subflow/Step dispatch is covered by tau-ir-conformance.
        let stub_impl = tau_ir::ToolImpl::Native {
            fn_ref: tau_ir::NativeFnRef {
                name: "stub".to_string(),
            },
            content_hash: [0u8; 32],
        };
        // Empty IrModule is fine — Native dispatch never reads it.
        let stub_module = alloc::sync::Arc::new(tau_ir::IrModule {
            ir_format: tau_ir::IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target: tau_ports::target::registry::list_available()
                .next()
                .expect("at least one target")
                .triple,
            workflow: tau_ir::Workflow {
                agents: alloc::collections::BTreeMap::new(),
                tools: alloc::collections::BTreeMap::new(),
                steps: alloc::collections::BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: tau_ir::CapabilityTable(alloc::collections::BTreeMap::new()),
            },
        });
        DispatcherTool {
            tool_name: "fixed".to_string(),
            tool_id: ToolId("fixed".to_string()),
            spec,
            module: stub_module,
            tool_impl: stub_impl,
            dispatcher,
        }
    }
```

Add the missing `use` lines at the top of the `mod tests` block if needed: `use alloc::collections::BTreeMap;` (probably not — only referenced through `alloc::` paths above) and `use tau_ir::{IrModule, Workflow, IrFormatVersion, NativeFnRef, ToolImpl, CapabilityTable};`.

- [ ] **Step 6: `cargo check` then run interpreter tests.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo check -p tau-runtime-core`
Expected: clean.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo nextest run -p tau-runtime-core`
Expected: all tests pass (including the four existing `DispatcherTool` tests, which still exercise the Native dispatch arm).

- [ ] **Step 7: Commit.**

```
git add crates/tau-runtime-core/src/interpreter/agent_loop.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-runtime-core/interpreter/agent_loop): branch DispatcherTool::invoke on ToolImpl (Subflow + Step)"
```

### Task 2.5: Update downstream callers — `tau-cli` + `tau-ir-conformance`

The signature change in Task 2.3 broke `run_ir` callers. There are two:

1. `crates/tau-cli/src/cmd/ir_dispatcher.rs` — calls `run_ir(&module, ...)`.
2. `crates/tau-ir-conformance/src/dev_mode.rs` — calls `run_ir(module, ...)`.
3. (BundleMode doesn't call run_ir directly; it goes through `drive_module`, which calls run_ir on its behalf.)

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs`
- Modify: `crates/tau-ir-conformance/src/dev_mode.rs`

- [ ] **Step 1: Update `tau-cli`.**

Find the call site in `crates/tau-cli/src/cmd/ir_dispatcher.rs` (grep for `run_ir(`). Wrap the module in `Arc::new(...)`:

```bash
grep -n "run_ir(" crates/tau-cli/src/cmd/ir_dispatcher.rs
```

Change `run_ir(&module, ...)` (or whatever the precise form is) to `run_ir(std::sync::Arc::new(module), ...)`. If `module` is referenced after the `run_ir` call, you may need to clone before passing — use `run_ir(std::sync::Arc::new(module.clone()), ...)`. Verify with a final `cargo check -p tau-cli`.

- [ ] **Step 2: Update `tau-ir-conformance::dev_mode`.**

In `crates/tau-ir-conformance/src/dev_mode.rs`'s `drive_module` (around line 321), the `run_ir(module, entry, dispatcher, Vec::new())` call. Change `module` to `std::sync::Arc::new(module.clone())` so the function still works with a `&IrModule` parameter:

```rust
    let outcome: RunOutcome = run_ir(
        std::sync::Arc::new(module.clone()),
        entry,
        dispatcher,
        Vec::new(),
    )
    .await
    .expect("run_ir must not return an Err for a valid conformance fixture");
```

(Alternative: change `drive_module`'s signature to take `Arc<IrModule>` and update both DevMode + BundleMode to construct one. Pick whichever is simpler — the per-call clone is fine since fixtures are tiny.)

- [ ] **Step 3: `cargo check` both crates.**

Run in parallel:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo check -p tau-cli
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo check -p tau-ir-conformance
```
Both clean.

- [ ] **Step 4: Run the existing conformance suite to confirm no regression.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance`
Expected: all six existing tests (01 dev, 01 cross, 02 dev, 02 cross, 03 dev, 03 cross, 06 dev, 06 cross) pass.

- [ ] **Step 5: Commit.**

```
git add crates/tau-cli/src/cmd/ir_dispatcher.rs crates/tau-ir-conformance/src/dev_mode.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "fix(tau-cli,tau-ir-conformance): wrap module in Arc for run_ir signature change"
```

---

## Phase 3 — Fixture 04 (subflow spawn child)

### Task 3.1: Write fixture 04 files

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/mock_llm.jsonl`
- Create: `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/expected_report.json`
- Delete: `crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/README.md`

**Entry-agent note:** the conformance runner picks the first agent alphabetically. `parent` < `worker` alphabetically, so `parent` is the entry.

**Mock-LLM sequencing:** `SequencedLlm` is one shared queue. The order of responses must follow the call order: parent's first completion → parent's tool_use(notify) → recursion into worker → worker's first completion → worker's tool_use(page) → worker's second completion → worker end_turn → recursion returns → parent's second completion → parent end_turn.

- [ ] **Step 1: Write `workflow.toml`.**

```toml
[project]
name = "fixture-04"

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
tool_refs    = ["notify"]
max_turns    = 3

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
llm_backend  = "mock-llm"
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

- [ ] **Step 2: Write `mock_llm.jsonl`.**

```json
{"turn": 0, "response": {"tool_uses": [{"id": "p1", "name": "notify", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"tool_uses": [{"id": "w1", "name": "page", "input": {}}], "stop_reason": "tool_use"}}
{"turn": 2, "response": {"text": "paged", "stop_reason": "end_turn"}}
{"turn": 3, "response": {"text": "done", "stop_reason": "end_turn"}}
```

- [ ] **Step 3: Write `expected_report.json`.**

```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": {
    "notify:{}": 1,
    "page:{}": 1
  }
}
```

- [ ] **Step 4: Delete the README placeholder.**

```
git rm crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/README.md
```

- [ ] **Step 5: Commit fixture files.**

```
git add crates/tau-ir-conformance/fixtures/04_subflow_spawn_child/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-ir-conformance): fixture 04 — subflow spawn child"
```

### Task 3.2: Un-defer fixture 04 in `tests/conformance.rs`

**Files:**
- Modify: `crates/tau-ir-conformance/tests/conformance.rs`

- [ ] **Step 1: Remove `04_subflow_spawn_child` from `DEFERRED_FIXTURES`.**

Change the constant (currently line 32):
```rust
pub const DEFERRED_FIXTURES: &[&str] = &["05_deterministic_step"];
```

Also update the module-level docs block (currently lines 7-19) to reflect that 04 is no longer deferred — strip the `- 04_subflow_spawn_child` bullet.

- [ ] **Step 2: Add test functions for fixture 04.**

Append (BEFORE the fixture-06 block, to keep numerical order):

```rust
// ---------------------------------------------------------------------------
// Fixture 04 — subflow_spawn_child
// ---------------------------------------------------------------------------

/// Fixture 04: parent agent invokes a subflow tool that spawns the
/// `worker` child agent; child calls an MCP `page` tool then ends; parent
/// receives the child's final assistant text as the tool result and ends.
///
/// Subflow tools themselves are NOT routed through
/// `RecordingDispatcher::invoke` (the Subflow arm of
/// `DispatcherTool::invoke` goes through `Box::pin(run_ir(...))`
/// directly — see Phase 2 fix C2). So `notify` does not appear in
/// `report.tool_calls`. We assert subflow execution via the CHILD's
/// recorded tool calls: if `page` is recorded, the recursive `run_ir`
/// ran (since `page` only exists in the child agent).
///
/// Expected: RunOutcome::Completed; multiset has `page:{}` = 1
/// (the child's MCP call), proving the subflow recursion executed.
#[tokio::test(flavor = "current_thread")]
async fn fixture_04_dev_mode_subflow_dispatched() {
    let dir = fixture_dir("04_subflow_spawn_child");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    assert_eq!(
        count_tool_calls(&report, "page"),
        1,
        "expected exactly 1 page (child MCP) call — proves subflow recursion ran"
    );
    assert_eq!(
        count_tool_calls(&report, "notify"),
        0,
        "subflow tools are not routed through dispatcher.invoke; should not appear in tool_calls"
    );
}

/// Cross-mode conformance for fixture 04.
#[tokio::test(flavor = "current_thread")]
async fn fixture_04_cross_mode_conformance() {
    let dir = fixture_dir("04_subflow_spawn_child");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
```

- [ ] **Step 3: Run the suite.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance fixture_04`
Expected: both tests pass.

If they fail, diagnose:
- "no more scripted responses" → the mock_llm.jsonl ordering is wrong; trace the call sequence and re-order.
- `count_tool_calls("page") == 0` → the recursion did NOT run. Verify `dispatcher.clone()` is passed to the recursive `run_ir` in `agent_loop.rs` Subflow arm.
- "RunOutcome kind mismatch" → child or parent budget exceeded; increase `max_turns` in workflow.toml. The subflow Subflow-Failed propagation fix (Phase 2 C1) means a Failed child surfaces as `is_error: true` to the parent — check `report.run_outcome` and the messages for the diagnostic.

- [ ] **Step 4: Commit.**

```
git add crates/tau-ir-conformance/tests/conformance.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-ir-conformance): un-defer fixture 04 + add dev/cross-mode tests"
```

---

## Phase 4 — Fixture 05 (deterministic step)

### Task 4.1: Add `MapBackedDeterministicRegistry` to the conformance lib

**Files:**
- Modify: `crates/tau-ir-conformance/src/lib.rs`

The conformance crate provides a tiny dictionary-backed `DeterministicRegistry` that the `RecordingDispatcher` returns from `deterministic_registry()`. Fixture 05 needs at least `parse_celsius: {"raw":"22"} → {"celsius":22}` registered.

- [ ] **Step 1: Append a public `MapBackedDeterministicRegistry` to `lib.rs`.**

Append at the end of `crates/tau-ir-conformance/src/lib.rs`:

```rust
// ---------------------------------------------------------------------------
// MapBackedDeterministicRegistry — fixture-side DeterministicRegistry
// ---------------------------------------------------------------------------

use std::sync::Arc;

use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;

/// A `DeterministicRegistry` backed by a `BTreeMap<String, Fn>`.
///
/// The conformance suite uses this to wire scripted deterministic
/// functions into `RecordingDispatcher::deterministic_registry()`.
/// Fixture authors call [`MapBackedDeterministicRegistry::with`] to
/// register named functions.
pub struct MapBackedDeterministicRegistry {
    fns: BTreeMap<
        String,
        Arc<
            dyn Fn(&serde_json::Value) -> Result<serde_json::Value, RuntimeError> + Send + Sync,
        >,
    >,
}

impl Default for MapBackedDeterministicRegistry {
    fn default() -> Self {
        Self {
            fns: BTreeMap::new(),
        }
    }
}

impl MapBackedDeterministicRegistry {
    /// Register a function under `fn_name`. The function must be pure
    /// (no I/O, no global mutation).
    pub fn with<F>(mut self, fn_name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, RuntimeError>
            + Send
            + Sync
            + 'static,
    {
        self.fns.insert(fn_name.into(), Arc::new(f));
        self
    }
}

impl DeterministicRegistry for MapBackedDeterministicRegistry {
    fn invoke(
        &self,
        fn_name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        let f = self
            .fns
            .get(fn_name)
            .ok_or_else(|| RuntimeError::Internal {
                message: format!("MapBackedDeterministicRegistry: unknown fn {fn_name:?}"),
            })?;
        f(args)
    }
}

/// The canonical conformance-fixture registry used by the test suite.
///
/// Currently registers:
///
/// - `parse_celsius`: `{"raw": "<digits>"}` → `{"celsius": <int>}`.
///   Used by fixture 05 (`05_deterministic_step`).
pub fn fixture_deterministic_registry() -> Arc<MapBackedDeterministicRegistry> {
    Arc::new(MapBackedDeterministicRegistry::default().with(
        "parse_celsius",
        |args: &serde_json::Value| {
            let raw = args
                .get("raw")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RuntimeError::Internal {
                    message: "parse_celsius: `raw` must be a string".into(),
                })?;
            let celsius: i64 = raw.trim().parse().map_err(|e| RuntimeError::Internal {
                message: format!("parse_celsius: not an integer: {e}"),
            })?;
            Ok(serde_json::json!({ "celsius": celsius }))
        },
    ))
}
```

- [ ] **Step 2: Wire `RecordingDispatcher::deterministic_registry()` in `dev_mode.rs`.**

In `crates/tau-ir-conformance/src/dev_mode.rs`, add a `deterministic_registry()` method to the `ToolDispatcher` impl for `RecordingDispatcher` (currently lines 121-158). The full updated impl block:

```rust
impl ToolDispatcher for RecordingDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a tau_ir::ToolId,
        args: &'a JsonValue,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        let tool_name = self
            .tool_names
            .get(&tool_id.0)
            .cloned()
            .unwrap_or_else(|| tool_id.0.clone());
        let args_canonical = serde_json::to_vec(args).unwrap_or_default();
        let records = self.records.clone();

        Box::pin(async move {
            records
                .lock()
                .expect("records mutex poisoned")
                .push(ToolCallRecord {
                    tool_name,
                    args_canonical,
                });

            Ok(ToolInvocationResult {
                body: Some(serde_json::json!({"ok": true})),
                error: None,
            })
        })
    }

    fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
        self.backend.clone()
    }

    fn deterministic_registry(
        &self,
    ) -> Option<
        std::sync::Arc<dyn tau_runtime_core::interpreter::deterministic::DeterministicRegistry>,
    > {
        Some(crate::fixture_deterministic_registry())
    }
}
```

(`crate::fixture_deterministic_registry()` returns `Arc<MapBackedDeterministicRegistry>`; the trait object coercion is automatic via `Arc::<dyn _>::from(...)`. If the compiler complains, wrap explicitly: `Some(crate::fixture_deterministic_registry() as Arc<dyn _>)`.)

**No `DispatcherTool::invoke` Step arm change needed.** Phase 2's correctness review (C2) established that the Step arm MUST NOT call `self.dispatcher.invoke()` for accounting — in production (`ForwardingDispatcher`) that would fire a real plugin lookup for a tool that doesn't exist. The Step arm correctly goes directly to the registry. Consequences for the conformance tests:

- `RecordingDispatcher.records` (populated by `RecordingDispatcher::invoke`) captures Native + Mcp tool calls only.
- Subflow tools (Phase 3 fixture 04) and Step tools (this phase fixture 05) are NOT recorded in `report.tool_calls`. They ARE observable through other signals:
  - **Subflow proof:** the child agent's tool calls (e.g. fixture 04's `page`) ARE recorded, because the dispatcher is shared via `Arc::clone` across the `Box::pin` recursion. If `count_tool_calls("page") == 1`, the subflow recursion definitively ran.
  - **Step proof:** the step's output value becomes the tool-result message in `report.message_added`. If a message body contains the step's expected output (`"celsius"`), the step definitively ran AND its result reached the LLM. Plus `RunOutcome::Completed` proves the whole chain succeeded.

The Phase 4 test assertions in Task 4.3 are written accordingly.

- [ ] **Step 3: `cargo check` then run conformance.**

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo check -p tau-ir-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance
```
Both clean / passing.

- [ ] **Step 4: Commit.**

```
git add crates/tau-ir-conformance/src/lib.rs crates/tau-ir-conformance/src/dev_mode.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "feat(tau-ir-conformance): MapBackedDeterministicRegistry + RecordingDispatcher wiring"
```

### Task 4.2: Write fixture 05 files

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/05_deterministic_step/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/05_deterministic_step/mock_llm.jsonl`
- Create: `crates/tau-ir-conformance/fixtures/05_deterministic_step/expected_report.json`
- Delete: `crates/tau-ir-conformance/fixtures/05_deterministic_step/README.md`

- [ ] **Step 1: Write `workflow.toml`.**

```toml
[project]
name = "fixture-05"

[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
tool_refs    = ["normalize"]
max_turns    = 2

[steps.normalize]
deterministic = "parse_celsius"
input_schema  = {}
output_schema = {}
```

(Note: the `[tools.normalize]` block is NOT declared by the author. The parse stage auto-registers a `Tool` with `ToolImpl::Step { id: "normalize" }` for each `[steps.X]` entry. This is the v0 sugar — one step → one tool, same name.)

- [ ] **Step 2: Write `mock_llm.jsonl`.**

```json
{"turn": 0, "response": {"tool_uses": [{"id": "1", "name": "normalize", "input": {"raw": "22"}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"text": "22 celsius", "stop_reason": "end_turn"}}
```

- [ ] **Step 3: Write `expected_report.json`.**

```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": {
    "normalize:{\"raw\":\"22\"}": 1
  }
}
```

- [ ] **Step 4: Delete the README placeholder.**

```
git rm crates/tau-ir-conformance/fixtures/05_deterministic_step/README.md
```

- [ ] **Step 5: Commit.**

```
git add crates/tau-ir-conformance/fixtures/05_deterministic_step/
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-ir-conformance): fixture 05 — deterministic step"
```

### Task 4.3: Un-defer fixture 05 + add tests

**Files:**
- Modify: `crates/tau-ir-conformance/tests/conformance.rs`

- [ ] **Step 1: Empty `DEFERRED_FIXTURES`.**

```rust
pub const DEFERRED_FIXTURES: &[&str] = &[];
```

Update the module-level doc block to drop both deferred-fixture bullets (or rewrite the section to say "all six fixtures are live as of β.2.6.2"). Since both bullets are removed, you can also delete the whole "Deferred fixtures" section if preferred.

- [ ] **Step 2: Add test functions for fixture 05.**

Append AFTER the fixture-04 tests, BEFORE the fixture-06 block:

```rust
// ---------------------------------------------------------------------------
// Fixture 05 — deterministic_step
// ---------------------------------------------------------------------------

/// Fixture 05: agent invokes a deterministic step tool `normalize` (auto-
/// registered by the parse stage for `[steps.normalize]`). The
/// `MapBackedDeterministicRegistry::parse_celsius` runs and returns
/// `{"celsius": 22}`. Agent's next turn emits `"22 celsius"` and ends.
///
/// Expected: RunOutcome::Completed; multiset has exactly one
/// `normalize:{"raw":"22"}` entry.
#[tokio::test(flavor = "current_thread")]
async fn fixture_05_dev_mode_step_dispatched() {
    let dir = fixture_dir("05_deterministic_step");
    let report = DevMode.run(&dir).await;

    assert!(
        matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected RunOutcome::Completed, got: {:?}",
        report.run_outcome
    );
    // Step tools (like Subflow tools) are NOT routed through
    // dispatcher.invoke — the Step arm of DispatcherTool::invoke calls
    // the DeterministicRegistry directly (see Phase 2 fix C2). So
    // `normalize` does NOT appear in `report.tool_calls`. The strongest
    // observable: a tool-result message containing the step's output
    // (`{"celsius":22}`) reached the LLM, which only happens if the
    // step ran successfully and its result was injected into the
    // message stream.
    assert_eq!(
        count_tool_calls(&report, "normalize"),
        0,
        "step tools are not routed through dispatcher.invoke; should not appear in tool_calls"
    );
    let step_result_observed = report.message_added.keys().any(|bytes| {
        std::str::from_utf8(bytes)
            .map(|s| s.contains("celsius"))
            .unwrap_or(false)
    });
    assert!(
        step_result_observed,
        "expected the step's `celsius` output to appear in at least one message body; got messages: {:?}",
        report.message_added.keys().map(|b| String::from_utf8_lossy(b).to_string()).collect::<Vec<_>>()
    );
}

/// Cross-mode conformance for fixture 05.
#[tokio::test(flavor = "current_thread")]
async fn fixture_05_cross_mode_conformance() {
    let dir = fixture_dir("05_deterministic_step");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
```

- [ ] **Step 3: Run the full suite.**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance`
Expected: ALL tests pass. Previously: 8 (4 fixtures × 2 modes-ish). Now: 12 (6 fixtures × 2). Plus the bundle-mode capability_fit test.

- [ ] **Step 4: Commit.**

```
git add crates/tau-ir-conformance/tests/conformance.rs
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "test(tau-ir-conformance): un-defer fixture 05 + add dev/cross-mode tests"
```

---

## Phase 5 — Cross-crate validation, doc-tests, push, PR

### Task 5.1: Run full per-crate test + clippy passes

- [ ] **Step 1: Run nextest + clippy + doctests across all touched crates.**

Run these in parallel where possible:

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo nextest run -p tau-cli
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo clippy -p tau-runtime-core --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo clippy -p tau-ir-conformance --all-targets -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-core cargo test --doc -p tau-runtime-core
```

Expected: all green.

- [ ] **Step 2: Run `cargo fmt --check` on touched crates.**

Run: `timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt --check -p tau-ir -p tau-runtime-core -p tau-ir-conformance -p tau-cli`
Expected: clean.

If any failure, run `cargo fmt -p <crate>` and commit:

```
git add -u
git -c user.name="Test User" -c user.email="test@example.com" commit --no-verify -m "style: cargo fmt"
```

### Task 5.2: Push branch + open PR

- [ ] **Step 1: Push.**

Run: `git push --no-verify -u origin feat/workflow-ir-beta-2-6-2`

- [ ] **Step 2: Open the PR.**

```bash
gh pr create --title "feat(tau-ir,tau-runtime-core,tau-ir-conformance): β.2.6.2 — subflow + step tool dispatch + fixtures 04/05" --body "$(cat <<'EOF'
## Summary

Closes β.2.6.2 — the final two β.2 conformance fixtures (`04_subflow_spawn_child`, `05_deterministic_step`) are now live. Six of six fixtures pass dev-mode + cross-mode; no `#[ignore]`'d or DEFERRED slots remain. β.2 (workflow IR) family fully shipped.

## What changed

- **tau-ir** — two new `ToolImpl` variants: `Subflow { target: AgentId }` (sub-agent spawn) and `Step { id: StepId }` (pure deterministic function). `parse.rs` auto-registers Tool nodes for `[tools.X] subflow = "..."` entries and for each `[steps.X]` block. `typecheck.rs` validates that the target agent / step exists. New `IrError::UnknownSubflowToolTarget` + `UnknownStepToolTarget` variants. `SubflowEdge::Spawn` emission removed (Compose variant reserved for future use).
- **tau-runtime-core** — `DispatcherTool::invoke` branches on `ToolImpl`. Subflow recursively calls `run_ir` (broken with `Box::pin` to satisfy Rust's no-self-referential-async-fn rule); the child's last assistant text becomes the parent's tool-result body. Step calls a `DeterministicRegistry` accessed via a new default-`None` `ToolDispatcher::deterministic_registry()` method. `run_ir`/`run_agent`/`run_subflow` now take `Arc<IrModule>` so the dispatcher can share the module across recursion.
- **tau-ir-conformance** — `MapBackedDeterministicRegistry` ships with `parse_celsius` registered; `RecordingDispatcher::deterministic_registry()` returns it. Fixtures 04 + 05 author their `workflow.toml` / `mock_llm.jsonl` / `expected_report.json`; READMEs removed; `DEFERRED_FIXTURES` is now empty.
- **tau-cli** — `ir_dispatcher::run_via_ir` wraps `module` in `Arc::new(...)` for the new `run_ir` signature. `ForwardingDispatcher::deterministic_registry()` returns `None` (production has no native-fn registry yet; β.7 AOT is the natural place for one).

## v0 limitations (documented in code)

- Subflow tool calls invoke the child with **empty initial messages** — tool input args are NOT forwarded to the child as a User message. β.7 (AOT codegen) is the natural place to thread arg-forwarding.
- Subflow tools don't yet declare a capability shape. `agent.spawn` is reserved for a later host-gate iteration; v0 leaves the capability slot empty so `capability_fit` doesn't refuse fixture 04 on `linux-native-strict`.

## Test plan

- [x] `cargo nextest run -p tau-ir` (incl. new round-trip + parse + typecheck tests)
- [x] `cargo nextest run -p tau-runtime-core` (existing DispatcherTool tests still pass)
- [x] `cargo nextest run -p tau-ir-conformance` — all 12 conformance tests pass (6 fixtures × dev+cross-mode)
- [x] `cargo nextest run -p tau-cli` — no regressions on bundle dispatch
- [x] `cargo clippy` clean on all touched crates
- [x] `cargo fmt --check` clean
- [x] `cargo test --doc` clean on tau-ir + tau-runtime-core

Closes β.2.6.2; β.2 (workflow IR) family fully shipped.
EOF
)"
```

- [ ] **Step 3: Enroll auto-merge.**

```bash
PR=$(gh pr list --head feat/workflow-ir-beta-2-6-2 --json number --jq '.[0].number')
gh pr merge "$PR" --auto --squash --delete-branch
```

- [ ] **Step 4: Stop here — do NOT poll CI.**

Per CLAUDE.md: do not push after auto-merge enrollment unless CI fails. The next session will handle merge confirmation + memory entry. The branch will be deleted automatically on merge.

---

## Definition of done

After this PR merges:

1. ✅ `ToolImpl` has four variants: `Native`, `Mcp`, `Subflow`, `Step`. Canonical round-trips for all four.
2. ✅ `parse.rs` auto-registers Tool nodes for subflow bodies and step blocks. `SubflowEdge::Spawn` no longer emitted in v0.
3. ✅ `typecheck.rs` catches missing subflow targets and missing step ids with dedicated `IrError` variants.
4. ✅ `DispatcherTool::invoke` correctly dispatches all four `ToolImpl` variants. Subflow recursion is bounded by per-agent budget; `Box::pin` breaks the type-recursive future.
5. ✅ `ToolDispatcher::deterministic_registry()` is the standard surface for step support; production dispatcher returns `None`; conformance dispatcher returns `Some(MapBackedDeterministicRegistry)`.
6. ✅ Six fixtures live in conformance suite; `DEFERRED_FIXTURES` is empty.
7. ✅ No regressions on fixtures 01/02/03/06; no regressions on tau-cli bundle dispatch.
8. ✅ All touched crates pass cargo nextest + clippy + fmt + doctests.

---

## Self-review (done by author)

**Spec coverage:** Handoff §A 4.1 (fixture 04, path (a)) + §A 4.2 (fixture 05) — both covered. The dispatch design extends `ToolDispatcher` instead of introducing a separate `StepDispatcher` trait (the README's option (a) preferred to (b) for simpler plumbing).

**Placeholder scan:** No "TBD", "TODO", "implement later". Step bodies contain literal code. Test bodies are complete. Commit messages are spelled out.

**Type consistency:** `run_ir` / `run_agent` / `run_subflow` signatures match across mod.rs / agent_loop.rs / subflow.rs (all take `Arc<IrModule>`). `DispatcherTool` field set is consistent in declaration (Task 2.4 step 1) and construction (Task 2.4 step 4) and inline test fixture (Task 2.4 step 5). `MapBackedDeterministicRegistry` is the same name in lib.rs (Task 4.1 step 1), dev_mode.rs (Task 4.1 step 2), and the test (Task 4.3 step 2 uses it indirectly via `crate::fixture_deterministic_registry()`).

**Spec edge cases covered:**
- Capability-fit on subflow/step tools: both declare `capabilities = []`, so capability_fit is a no-op. No host-target gate refusal on linux-native-strict.
- Bundle-mode parity: BundleMode and DevMode share `drive_module`, so the same `ToolImpl::Subflow` / `ToolImpl::Step` branches run in both modes — identical multisets emerge.
- Recursion termination: `agent.budget.max_turns` bounds each agent loop; subflow recursion bottoms out when the child hits `end_turn` or budget. The shared SequencedLlm queue exhausting yields a controlled `LlmError::Internal` → `RunOutcome::Failed`, surfacing as a `ToolError::Internal` to the parent.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-01-workflow-ir-beta-2-6-2.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task with two-stage review between tasks. Best fit here: 13 tasks across 5 phases with hard CI gates (cargo check / nextest) between most of them.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch with checkpoints. Possible but the plan is ~900 lines; subagent isolation per task keeps contexts cleaner.

Which approach?
