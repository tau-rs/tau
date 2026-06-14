# β.4 Context Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, per-agent context pipeline — a declarative sequence of pure transformers (`trim_old`, `compact_tool_outputs`, `fit_budget`) applied to the conversation on every inference turn, lowered from `tau.toml` into the IR and executed by the runtime, with a public extension contract for custom nodes.

**Architecture:** Layered hybrid. The deterministic transformer pipeline is the β.6-conformance-gated core (Layer 1); agent-driven memory tools (β.4.4) and retrieval MCP (γ.6) are deferred additive layers. v1 transformers are `Pure`; the trait carries a `DeterminismClass` and capability declarations so `LlmBacked`/`Stateful` tiers slot in without a trait break. Transformers run on a per-turn **copy** of `Vec<Message>` at the existing projection seam (`stream.rs:293`), leaving stored history intact.

**Tech Stack:** Rust (workspace; `tau-domain`, `tau-ports`, `tau-ir`, `tau-runtime-core` `no_std`+`alloc`, `tau-pkg`, `tau-observe`, `tau-ir-conformance`, `tau-cli`). Spec: `docs/superpowers/specs/2026-06-14-beta-4-context-manager-design.md`.

**CARGO RULES (every command):** prefix `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl`, scope with `-p <crate>`. Use `cargo nextest run` for tests, `cargo test --doc` for doctests. Test=300s, build/check=180s, clippy=240s.

---

## File structure

| File | Responsibility | New/Modify |
|---|---|---|
| `crates/tau-ir/src/context.rs` | Extend `ContextConfig` with `pipeline: Vec<ContextStep>`; add `ContextStep`, `ContextNodeKind`, `DeterminismClass` | Modify |
| `crates/tau-ir/src/error.rs` | 3 IR check error variants | Modify |
| `crates/tau-ir/src/lower/typecheck.rs` | `check_context` structural validation | Modify |
| `crates/tau-ir/src/lower/parse.rs` | Lower `[agents.<id>.context]` → `ContextConfig` | Modify |
| `crates/tau-runtime-core/src/context/mod.rs` | `ContextTransformer` trait, `DeterminismClass` re-export, `TransformCx`, `CapabilityNeed`, `ContextError`, `ContextTransformerRegistry` | New |
| `crates/tau-runtime-core/src/context/estimator.rs` | `TokenEstimator` trait + `HeuristicEstimator` | New |
| `crates/tau-runtime-core/src/context/transformers.rs` | `TrimOld`, `CompactToolOutputs`, `FitBudget` | New |
| `crates/tau-runtime-core/src/context/build.rs` | `build_context_pipeline` (IR `ContextConfig` → `Vec<Arc<dyn ContextTransformer>>`) | New |
| `crates/tau-runtime-core/src/options.rs` | `RunOptions.context_pipeline` + `token_estimator` | Modify |
| `crates/tau-runtime-core/src/stream.rs` | Per-turn pipeline application at the projection seam | Modify |
| `crates/tau-runtime-core/src/error.rs` | `RuntimeError::ContextPipeline` variant | Modify |
| `crates/tau-runtime-core/src/interpreter/agent_loop.rs` | Build pipeline from `ir_agent.context`, put in `RunOptions` | Modify |
| `crates/tau-runtime-core/src/vocabulary.rs` | Mirror `EV_CONTEXT_STEP_RAN` | Modify |
| `crates/tau-observe/src/vocabulary.rs` | `EV_CONTEXT_STEP_RAN` constant + drift count | Modify |
| `crates/tau-pkg/src/project/project.rs` | `UncheckedContext`/`UncheckedContextStep` + `AgentEntry.context` + validation | Modify |
| `crates/tau-pkg/src/project/agent.rs` | thread context into lowering input | Modify |
| `crates/tau-ir-conformance/fixtures/13_context_pipeline/` | dev/bundle conformance fixture | New |
| `crates/tau-ir-conformance/tests/conformance.rs` | fixture 13 tests | Modify |
| `docs/decisions/0045-context-manager.md` | ADR | New |

---

## Phase A — IR types, checks, lowering (`tau-ir`)

### Task 1: Extend `ContextConfig` with the pipeline + shared `DeterminismClass`

**Files:**
- Modify: `crates/tau-ir/src/context.rs`
- Test: `crates/tau-ir/src/context.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Append to `crates/tau-ir/src/context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn empty_context_config_serializes_to_empty_object() {
        // Backward-compat: a ContextConfig with no steps must serialize
        // identically to the pre-β.4 empty placeholder ({}), so existing
        // bundles hash unchanged.
        let cfg = ContextConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn pipeline_roundtrips() {
        let cfg = ContextConfig {
            pipeline: alloc::vec![ContextStep {
                transformer: "fit_budget".to_string(),
                determinism: DeterminismClass::Pure,
                kind: ContextNodeKind::Builtin,
                config: Default::default(),
            }],
        };
        let json = serde_json::to_vec(&cfg).unwrap();
        let back: ContextConfig = serde_json::from_slice(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --lib context::tests 2>&1 | tail -20`
Expected: FAIL — `ContextStep`, `ContextNodeKind`, `DeterminismClass`, and the `pipeline` field don't exist.

- [ ] **Step 3: Replace the placeholder with the real types**

Replace the body of `crates/tau-ir/src/context.rs` (keep the file's existing module doc comment) with:

```rust
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use tau_domain::Value;

/// β.4 context-manager configuration attached to an [`crate::node::Agent`].
///
/// `None` on the agent means "no context management" (full history every
/// turn — pre-β.4 behavior). An empty `pipeline` serializes to `{}` so a
/// `Some(ContextConfig::default())` is byte-identical to the legacy empty
/// placeholder.
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Ordered transformers, applied top-to-bottom each turn. The last
    /// step must be the builtin `fit_budget` (typecheck-enforced).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<ContextStep>,
}

/// One node in a context pipeline.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextStep {
    /// Transformer name. For builtins: `trim_old`, `compact_tool_outputs`,
    /// `fit_budget`. For custom nodes: the user-chosen step name.
    pub transformer: String,
    /// Author-declared determinism class. Gates β.6 conformance and what
    /// `TransformCx` exposes at runtime.
    pub determinism: DeterminismClass,
    /// Whether this is a builtin or a user-supplied custom node.
    #[serde(default)]
    pub kind: ContextNodeKind,
    /// Per-node config (e.g. `keep_last_turns`, `max_bytes`, `max_tokens`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
}

/// Determinism class shared by the IR (this enum) and the runtime trait
/// (`tau_runtime_core::context::ContextTransformer::determinism`).
/// Defined here so both crates use one definition (no drift).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum DeterminismClass {
    /// Pure function of (messages, config); v1's three transformers.
    Pure,
    /// Calls an `LlmBackend` (β.4.3); conformance-gated via cassette replay.
    LlmBacked,
    /// Reads/writes a memory store (β.4.4); excluded from the conformance gate.
    Stateful,
}

/// Delivery vehicle for a context node.
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
pub enum ContextNodeKind {
    /// A tau-provided builtin transformer.
    #[default]
    Builtin,
    /// A user-supplied node resolved at runtime. `source` selects the lane.
    Custom {
        /// `native` (v1) | `wasm` (later) | `mcp` (later).
        source: String,
        /// Package reference providing the node (e.g. `my-nodes@^0.1`).
        package: String,
    },
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --lib context::tests 2>&1 | tail -20`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/context.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-ir): extend ContextConfig with the β.4 pipeline shape"
```

---

### Task 2: IR error variants for context checks

**Files:**
- Modify: `crates/tau-ir/src/error.rs`

- [ ] **Step 1: Add the variants** (no separate test; covered by Task 3)

In `crates/tau-ir/src/error.rs`, inside the `IrError` enum (alongside `UnknownCheckRef`), add:

```rust
    /// A context pipeline names a transformer that is neither a known
    /// builtin nor a declared custom node.
    #[error("agent '{agent}': context transformer '{transformer}' is not a known builtin or custom node")]
    UnknownContextTransformer {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The offending transformer name.
        transformer: String,
    },

    /// A context pipeline's last step is not the builtin `fit_budget`.
    #[error("agent '{agent}': the last context step must be `fit_budget` (found '{last}')")]
    ContextFitBudgetNotLast {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The actual last transformer name.
        last: String,
    },

    /// A context pipeline repeats a transformer name.
    #[error("agent '{agent}': duplicate context transformer '{transformer}'")]
    DuplicateContextTransformer {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The repeated transformer name.
        transformer: String,
    },
```

- [ ] **Step 2: Compile-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir 2>&1 | tail -20`
Expected: PASS (compiles).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-ir/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-ir): add context-pipeline check error variants"
```

---

### Task 3: `check_context` structural typecheck

**Files:**
- Modify: `crates/tau-ir/src/lower/typecheck.rs`
- Test: `crates/tau-ir/src/lower/typecheck.rs` (inline tests)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/tau-ir/src/lower/typecheck.rs` (use the existing module's helpers if present; otherwise construct a `Workflow` directly):

```rust
    use crate::context::{ContextConfig, ContextNodeKind, ContextStep, DeterminismClass};
    use crate::node::Agent;
    use crate::ids::AgentId;

    fn agent_with_context(steps: alloc::vec::Vec<&str>) -> crate::module::Workflow {
        let pipeline = steps
            .into_iter()
            .map(|t| ContextStep {
                transformer: t.into(),
                determinism: DeterminismClass::Pure,
                kind: ContextNodeKind::Builtin,
                config: Default::default(),
            })
            .collect();
        let mut wf = crate::module::Workflow::default();
        wf.agents.insert(
            AgentId("a".into()),
            Agent {
                id: AgentId("a".into()),
                prompt: "p".into(),
                model: "m".into(),
                tool_refs: alloc::vec![],
                context: Some(ContextConfig { pipeline }),
                budget: Default::default(),
                produces: alloc::vec![],
            },
        );
        wf
    }

    #[test]
    fn context_ok_when_fit_budget_last() {
        let wf = agent_with_context(alloc::vec!["trim_old", "fit_budget"]);
        assert!(check_context(&wf).is_ok());
    }

    #[test]
    fn context_rejects_unknown_transformer() {
        let wf = agent_with_context(alloc::vec!["bogus", "fit_budget"]);
        assert!(matches!(
            check_context(&wf),
            Err(crate::error::IrError::UnknownContextTransformer { .. })
        ));
    }

    #[test]
    fn context_rejects_fit_budget_not_last() {
        let wf = agent_with_context(alloc::vec!["fit_budget", "trim_old"]);
        assert!(matches!(
            check_context(&wf),
            Err(crate::error::IrError::ContextFitBudgetNotLast { .. })
        ));
    }

    #[test]
    fn context_rejects_duplicate() {
        let wf = agent_with_context(alloc::vec!["trim_old", "trim_old", "fit_budget"]);
        assert!(matches!(
            check_context(&wf),
            Err(crate::error::IrError::DuplicateContextTransformer { .. })
        ));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --lib typecheck 2>&1 | tail -20`
Expected: FAIL — `check_context` not defined.

- [ ] **Step 3: Implement `check_context` and call it from the typecheck entry point**

Add to `crates/tau-ir/src/lower/typecheck.rs`:

```rust
/// Structural validation of every agent's context pipeline.
///
/// Builtins: `trim_old`, `compact_tool_outputs`, `fit_budget`. Custom nodes
/// (`ContextNodeKind::Custom`) are accepted structurally here; their
/// capability grants are checked in tau-pkg. Rules:
/// - the last step must be the builtin `fit_budget` (guarantees a ceiling);
/// - no transformer name repeats;
/// - a `Builtin`-kind step must be a known builtin name.
fn check_context(wf: &crate::module::Workflow) -> Result<(), crate::error::IrError> {
    use crate::context::ContextNodeKind;
    use alloc::collections::BTreeSet;

    const BUILTINS: [&str; 3] = ["trim_old", "compact_tool_outputs", "fit_budget"];

    for (id, agent) in wf.agents.iter() {
        let Some(ctx) = &agent.context else { continue };
        if ctx.pipeline.is_empty() {
            continue;
        }

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for step in &ctx.pipeline {
            if !seen.insert(step.transformer.as_str()) {
                return Err(crate::error::IrError::DuplicateContextTransformer {
                    agent: id.0.clone(),
                    transformer: step.transformer.clone(),
                });
            }
            if matches!(step.kind, ContextNodeKind::Builtin)
                && !BUILTINS.contains(&step.transformer.as_str())
            {
                return Err(crate::error::IrError::UnknownContextTransformer {
                    agent: id.0.clone(),
                    transformer: step.transformer.clone(),
                });
            }
        }

        let last = ctx.pipeline.last().expect("non-empty checked above");
        if last.transformer != "fit_budget" {
            return Err(crate::error::IrError::ContextFitBudgetNotLast {
                agent: id.0.clone(),
                last: last.transformer.clone(),
            });
        }
    }
    Ok(())
}
```

Then call it from the public typecheck entry (find the fn that calls `check_pipeline(...)`, e.g. `typecheck(module)`), adding `check_context(&module.workflow)?;` next to the existing `check_pipeline` call.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --lib typecheck 2>&1 | tail -20`
Expected: PASS (4 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/lower/typecheck.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-ir): structural typecheck for context pipelines"
```

---

### Task 4: Lower `[agents.<id>.context]` from config into `ContextConfig`

**Note:** This depends on tau-pkg exposing the parsed context on its lowering input. The lowering input is the `ProjectConfig`/agent struct tau-ir reads (see `lower/parse.rs`). Implement Task 8 (tau-pkg parse) first if lowering reads tau-pkg types directly; otherwise tau-ir reads its own `ProjectConfig`. This task lowers whatever the parse stage produced into `ContextConfig` and attaches it to the IR `Agent`.

**Files:**
- Modify: `crates/tau-ir/src/lower/parse.rs`
- Test: `crates/tau-ir/tests/lower_e2e.rs`

- [ ] **Step 1: Write the failing e2e test**

Add to `crates/tau-ir/tests/lower_e2e.rs`:

```rust
#[test]
fn lowers_context_pipeline_onto_agent() {
    let toml = r#"
[project]
name = "ctx-lower"

[agents.a]
display_name = "A"
package      = "demo@^0.1"
llm_backend  = "mock-llm"
model        = "m"

[[agents.a.context.pipeline]]
transformer = "trim_old"
[agents.a.context.steps.trim_old]
keep_last_turns = 4

[[agents.a.context.pipeline]]
transformer = "fit_budget"
[agents.a.context.steps.fit_budget]
max_tokens = 4000
"#;
    let module = tau_ir::lower_str(toml).expect("lowering must succeed");
    let agent = module
        .workflow
        .agents
        .get(&tau_ir::AgentId("a".into()))
        .unwrap();
    let ctx = agent.context.as_ref().expect("context present");
    assert_eq!(ctx.pipeline.len(), 2);
    assert_eq!(ctx.pipeline[0].transformer, "trim_old");
    assert_eq!(ctx.pipeline[1].transformer, "fit_budget");
}
```

(Use whatever the crate's lowering entrypoint is — `lower_str`, `lower`, or via `tau_pkg`. Match the existing tests in this file.)

- [ ] **Step 2: Run to verify failure**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --test lower_e2e lowers_context 2>&1 | tail -20`
Expected: FAIL — `agent.context` is `None`.

- [ ] **Step 3: Implement lowering**

In `crates/tau-ir/src/lower/parse.rs`, where the IR `Agent` is constructed from the config agent entry, set `context` from the parsed context. Add a helper near `lower_checks`:

```rust
fn lower_context(entry: &<agent entry type>) -> Option<crate::context::ContextConfig> {
    use crate::context::{ContextConfig, ContextNodeKind, ContextStep, DeterminismClass};
    let steps = entry.context_pipeline(); // accessor returning the parsed steps + per-node config
    if steps.is_empty() {
        return None;
    }
    let pipeline = steps
        .iter()
        .map(|s| ContextStep {
            transformer: s.transformer.clone(),
            determinism: match s.determinism.as_str() {
                "llm_backed" => DeterminismClass::LlmBacked,
                "stateful" => DeterminismClass::Stateful,
                _ => DeterminismClass::Pure,
            },
            kind: match &s.custom {
                Some((source, package)) => ContextNodeKind::Custom {
                    source: source.clone(),
                    package: package.clone(),
                },
                None => ContextNodeKind::Builtin,
            },
            config: s.config.clone(), // BTreeMap<String, tau_domain::Value>
        })
        .collect();
    Some(ContextConfig { pipeline })
}
```

Replace `<agent entry type>` and `context_pipeline()`/field names with the concrete shapes produced by Task 8. Set `context: lower_context(entry)` in the `Agent { .. }` literal.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --test lower_e2e lowers_context 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/lower/parse.rs crates/tau-ir/tests/lower_e2e.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-ir): lower [agents.*.context] into the IR Agent"
```

---

## Phase B — Runtime engine (`tau-runtime-core`)

### Task 5: `TokenEstimator` + `HeuristicEstimator`

**Files:**
- Create: `crates/tau-runtime-core/src/context/estimator.rs`
- Create: `crates/tau-runtime-core/src/context/mod.rs` (module stub for now)
- Modify: `crates/tau-runtime-core/src/lib.rs` (add `pub mod context;`)

- [ ] **Step 1: Add module wiring + failing test**

In `crates/tau-runtime-core/src/lib.rs` add (near the other `mod` lines): `pub mod context;`

Create `crates/tau-runtime-core/src/context/mod.rs`:

```rust
//! β.4 context-manager primitive: per-turn transformers over the
//! conversation, applied before history is projected to the LLM.

pub mod estimator;

pub use estimator::{HeuristicEstimator, TokenEstimator};
```

Create `crates/tau-runtime-core/src/context/estimator.rs`:

```rust
use tau_domain::{Message, MessagePayload};

/// Estimates the token cost of a message. v1 ships [`HeuristicEstimator`];
/// a real per-model tokenizer can replace it behind this trait without
/// changing the transformer contract.
pub trait TokenEstimator: Send + Sync {
    /// Approximate token count for one message.
    fn estimate(&self, msg: &Message) -> u32;
}

/// Deterministic `ceil(bytes / 4)` heuristic plus a fixed per-message
/// structural overhead. Pure arithmetic — identical on every platform, so
/// it is conformance-stable (β.6) and portable (wasm/MCU).
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicEstimator;

/// Per-message structural overhead (role tag, delimiters), in tokens.
const MESSAGE_OVERHEAD: u32 = 4;

impl HeuristicEstimator {
    fn payload_bytes(payload: &MessagePayload) -> usize {
        match payload {
            MessagePayload::Text { content } => content.len(),
            MessagePayload::ToolCall { args } => args.to_string().len(),
            MessagePayload::ToolResult { body } => body.to_string().len(),
            MessagePayload::ToolError { kind, message, details } => {
                kind.len()
                    + message.len()
                    + details.as_ref().map(|d| d.to_string().len()).unwrap_or(0)
            }
            MessagePayload::Lifecycle(_) => 0,
            MessagePayload::Custom { kind, body } => kind.len() + body.len(),
            _ => 0,
        }
    }
}

impl TokenEstimator for HeuristicEstimator {
    fn estimate(&self, msg: &Message) -> u32 {
        let bytes = Self::payload_bytes(&msg.payload);
        // ceil(bytes / 4)
        let approx = u32::try_from(bytes.div_ceil(4)).unwrap_or(u32::MAX);
        approx.saturating_add(MESSAGE_OVERHEAD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::{Address, Message, MessagePayload};

    fn text(content: &str) -> Message {
        Message::new(
            Address::User,
            Address::System,
            MessagePayload::Text { content: content.into() },
        )
    }

    #[test]
    fn estimate_is_bytes_over_four_plus_overhead() {
        // 8 bytes -> ceil(8/4)=2, +4 overhead = 6
        assert_eq!(HeuristicEstimator.estimate(&text("12345678")), 6);
    }

    #[test]
    fn estimate_is_deterministic() {
        let m = text("the fan-monitor reads temperature");
        assert_eq!(HeuristicEstimator.estimate(&m), HeuristicEstimator.estimate(&m));
    }
}
```

- [ ] **Step 2: Run to verify it compiles + passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --lib context::estimator 2>&1 | tail -20`
Expected: PASS (2 tests). (If `div_ceil` is unstable on the MSRV, use `(bytes + 3) / 4`.)

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/lib.rs crates/tau-runtime-core/src/context/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): heuristic token estimator behind a swappable port"
```

---

### Task 6: The `ContextTransformer` trait, `TransformCx`, `ContextError`, `CapabilityNeed`

**Files:**
- Modify: `crates/tau-runtime-core/src/context/mod.rs`

- [ ] **Step 1: Add the contract types**

Append to `crates/tau-runtime-core/src/context/mod.rs`:

```rust
pub mod transformers;
pub mod build;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use tau_domain::{CapabilityShape, Message};

// Re-export the IR-defined determinism class so the trait and the IR share
// one definition (no drift).
pub use tau_ir::context::DeterminismClass;

/// A capability a transformer requires (e.g. fs-write for β.4.2 offload).
/// v1's three builtins return an empty slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityNeed {
    /// The capability shape required (matches `tau_domain::CapabilityShape`).
    pub shape: CapabilityShape,
}

/// Error returned by a context transformer or the pipeline runner.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextError {
    /// Protected content (system prompt + live turn) alone exceeds the budget.
    #[error("context budget unsatisfiable: protected {protected_tokens} tokens > max {max_tokens}")]
    BudgetUnsatisfiable {
        /// Estimated tokens of protected (undroppable) content.
        protected_tokens: u32,
        /// The configured `fit_budget.max_tokens`.
        max_tokens: u32,
    },
    /// A transformer failed internally.
    #[error("context transformer '{name}' failed: {detail}")]
    Transformer {
        /// Transformer name.
        name: String,
        /// Human-readable detail.
        detail: String,
    },
}

/// Capability-scoped context handed to a transformer. The fields a class
/// may access are gated by construction: `Pure` gets the estimator only;
/// `LlmBacked`/`Stateful` (later tiers) get additional handles.
pub struct TransformCx<'a> {
    estimator: &'a dyn TokenEstimator,
    system_prompt: Option<&'a str>,
}

impl<'a> TransformCx<'a> {
    /// Construct a `Pure`-scoped context.
    pub fn pure(estimator: &'a dyn TokenEstimator, system_prompt: Option<&'a str>) -> Self {
        Self { estimator, system_prompt }
    }
    /// Estimate one message's token cost.
    pub fn estimate_tokens(&self, msg: &Message) -> u32 {
        self.estimator.estimate(msg)
    }
    /// The agent's system prompt, if any (counts against the budget).
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt
    }
}

/// Future returned by [`ContextTransformer::transform`]. Mirrors the
/// boxed-future idiom used by `ToolDispatcher` (no `async_trait` in core).
pub type ContextFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Message>, ContextError>> + Send + 'a>>;

/// One step in an agent's per-turn context pipeline. THE public extension
/// point (contract #5): users implement this for custom nodes.
pub trait ContextTransformer: Send + Sync {
    /// Stable name; matches the `transformer` field in the IR.
    fn name(&self) -> &str;
    /// Determinism class; gates conformance and `TransformCx` scoping.
    fn determinism(&self) -> DeterminismClass;
    /// Capabilities this node needs (empty for v1's builtins).
    fn required_capabilities(&self) -> &[CapabilityNeed];
    /// Transform the per-turn message view. Default boxes [`Self::apply`].
    fn transform<'a>(&'a self, cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a>;
}
```

- [ ] **Step 2: Compile-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core 2>&1 | tail -25`
Expected: FAIL only on the not-yet-created `transformers`/`build` submodules (next tasks). If other errors appear (e.g. missing `thiserror` import), fix imports. To compile this task alone, temporarily comment the `pub mod transformers; pub mod build;` lines, verify, then re-add in Task 7/9.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/context/mod.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): ContextTransformer trait + capability-scoped TransformCx"
```

---

### Task 7: The three v1 transformers

**Files:**
- Create: `crates/tau-runtime-core/src/context/transformers.rs`

Each transformer exposes a pure `apply()` (sync, unit-tested directly) and a trait `transform()` that boxes it. A **turn** = messages from one `Address::User` `Text` message up to (excluding) the next; dropping is turn-granular.

- [ ] **Step 1: Write the failing tests**

Create `crates/tau-runtime-core/src/context/transformers.rs`:

```rust
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use tau_domain::{Address, Message, MessagePayload, Value};

use super::{
    CapabilityNeed, ContextError, ContextFuture, ContextTransformer, DeterminismClass, TransformCx,
};

/// Index boundaries of each turn: a turn starts at every `User`-text message.
/// Returns the start index of each turn (ascending). Messages before the
/// first `User`-text message form an implicit turn 0 starting at index 0.
fn turn_starts(msgs: &[Message]) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        if matches!((&m.sender, &m.payload), (Address::User, MessagePayload::Text { .. })) {
            starts.push(i);
        }
    }
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts
}

const NO_CAPS: &[CapabilityNeed] = &[];

// ---------------------------------------------------------------------------
// trim_old
// ---------------------------------------------------------------------------

/// Keep the N most recent turns; drop older turns whole.
#[derive(Debug, Clone)]
pub struct TrimOld {
    /// Number of most-recent turns to keep.
    pub keep_last_turns: u32,
}

impl TrimOld {
    /// Read config (`keep_last_turns`, default 8).
    pub fn from_config(cfg: &alloc::collections::BTreeMap<String, Value>) -> Self {
        let keep = cfg
            .get("keep_last_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(8) as u32;
        Self { keep_last_turns: keep }
    }

    /// Pure transform.
    pub fn apply(&self, msgs: Vec<Message>) -> Vec<Message> {
        if self.keep_last_turns == 0 {
            return msgs;
        }
        let starts = turn_starts(&msgs);
        if (starts.len() as u32) <= self.keep_last_turns {
            return msgs;
        }
        let keep_from_turn = starts.len() - self.keep_last_turns as usize;
        let cut = starts[keep_from_turn];
        msgs.into_iter().skip(cut).collect()
    }
}

impl ContextTransformer for TrimOld {
    fn name(&self) -> &str { "trim_old" }
    fn determinism(&self) -> DeterminismClass { DeterminismClass::Pure }
    fn required_capabilities(&self) -> &[CapabilityNeed] { NO_CAPS }
    fn transform<'a>(&'a self, _cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move { Ok(self.apply(msgs)) })
    }
}

// ---------------------------------------------------------------------------
// compact_tool_outputs
// ---------------------------------------------------------------------------

/// Truncate large tool-result/-error bodies in PRIOR turns (not the live turn).
#[derive(Debug, Clone)]
pub struct CompactToolOutputs {
    /// Max retained bytes of a tool body before truncation.
    pub max_bytes: usize,
}

impl CompactToolOutputs {
    /// Read config (`max_bytes`, default 1024).
    pub fn from_config(cfg: &alloc::collections::BTreeMap<String, Value>) -> Self {
        let max = cfg.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(1024) as usize;
        Self { max_bytes: max }
    }

    /// Pure transform. The live (last) turn is left untouched.
    pub fn apply(&self, mut msgs: Vec<Message>) -> Vec<Message> {
        let starts = turn_starts(&msgs);
        // index where the live turn begins; messages at/after this are kept verbatim
        let live_start = *starts.last().unwrap_or(&0);
        for (i, m) in msgs.iter_mut().enumerate() {
            if i >= live_start {
                break;
            }
            if let MessagePayload::ToolResult { body } = &m.payload {
                let s = body.to_string();
                if s.len() > self.max_bytes {
                    let mut kept: String = s.chars().take(self.max_bytes).collect();
                    let dropped = s.len() - kept.len();
                    kept.push_str(&alloc::format!("…[truncated {dropped} bytes]…"));
                    m.payload = MessagePayload::ToolResult { body: Value::String(kept) };
                }
            }
        }
        msgs
    }
}

impl ContextTransformer for CompactToolOutputs {
    fn name(&self) -> &str { "compact_tool_outputs" }
    fn determinism(&self) -> DeterminismClass { DeterminismClass::Pure }
    fn required_capabilities(&self) -> &[CapabilityNeed] { NO_CAPS }
    fn transform<'a>(&'a self, _cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move { Ok(self.apply(msgs)) })
    }
}

// ---------------------------------------------------------------------------
// fit_budget (must be last)
// ---------------------------------------------------------------------------

/// Drop oldest whole turns until the estimated total fits the budget.
#[derive(Debug, Clone)]
pub struct FitBudget {
    /// Max total tokens (system prompt reserved against this).
    pub max_tokens: u32,
}

impl FitBudget {
    /// Read config (`max_tokens`, default 8192).
    pub fn from_config(cfg: &alloc::collections::BTreeMap<String, Value>) -> Self {
        let max = cfg.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(8192) as u32;
        Self { max_tokens: max }
    }

    /// Pure transform. System prompt + live turn are protected.
    pub fn apply(&self, cx: &TransformCx<'_>, msgs: Vec<Message>) -> Result<Vec<Message>, ContextError> {
        let sys_tokens = cx
            .system_prompt()
            .map(|s| (s.len() as u32).div_ceil(4))
            .unwrap_or(0);
        let starts = turn_starts(&msgs);
        let live_start = *starts.last().unwrap_or(&0);

        let msg_tokens = |m: &Message| cx.estimate_tokens(m);
        let live_tokens: u32 = msgs[live_start..].iter().map(msg_tokens).sum();
        let protected = sys_tokens.saturating_add(live_tokens);
        if protected > self.max_tokens {
            return Err(ContextError::BudgetUnsatisfiable {
                protected_tokens: protected,
                max_tokens: self.max_tokens,
            });
        }

        // Drop oldest whole turns (turn 0..live) until total fits.
        let mut cut_turn = 0usize; // number of leading turns to drop
        loop {
            let cut = if cut_turn == 0 { 0 } else { starts[cut_turn] };
            let total: u32 =
                sys_tokens.saturating_add(msgs[cut..].iter().map(msg_tokens).sum());
            if total <= self.max_tokens || cut >= live_start {
                return Ok(msgs.into_iter().skip(cut).collect());
            }
            cut_turn += 1;
        }
    }
}

impl ContextTransformer for FitBudget {
    fn name(&self) -> &str { "fit_budget" }
    fn determinism(&self) -> DeterminismClass { DeterminismClass::Pure }
    fn required_capabilities(&self) -> &[CapabilityNeed] { NO_CAPS }
    fn transform<'a>(&'a self, cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move { self.apply(cx, msgs) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::HeuristicEstimator;
    use tau_domain::{Address, AgentInstanceId, Message, MessagePayload, Value};

    fn user(content: &str) -> Message {
        Message::new(Address::User, Address::Agent(AgentInstanceId::new()),
            MessagePayload::Text { content: content.into() })
    }
    fn assistant(content: &str) -> Message {
        Message::new(Address::Agent(AgentInstanceId::new()), Address::User,
            MessagePayload::Text { content: content.into() })
    }
    fn tool_result(body: &str) -> Message {
        Message::new(Address::Tool("t".into()), Address::Agent(AgentInstanceId::new()),
            MessagePayload::ToolResult { body: Value::String(body.into()) })
    }

    #[test]
    fn trim_old_keeps_last_n_turns() {
        // 3 turns; keep_last_turns=2 drops turn 1.
        let msgs = alloc::vec![
            user("t1"), assistant("a1"),
            user("t2"), assistant("a2"),
            user("t3"),
        ];
        let out = TrimOld { keep_last_turns: 2 }.apply(msgs);
        // turn1 (t1,a1) dropped -> 3 messages remain
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0].payload, MessagePayload::Text { content } if content == "t2"));
    }

    #[test]
    fn compact_truncates_prior_tool_output_not_live() {
        let big = "x".repeat(100);
        let msgs = alloc::vec![
            user("t1"), tool_result(&big),
            user("t2"), tool_result(&big), // live turn — untouched
        ];
        let out = CompactToolOutputs { max_bytes: 10 }.apply(msgs);
        // first tool_result truncated, second (live) intact
        let first = out[1].payload.clone();
        let last = out[3].payload.clone();
        match first { MessagePayload::ToolResult { body } => assert!(body.to_string().contains("truncated")), _ => panic!() }
        match last { MessagePayload::ToolResult { body } => assert!(!body.to_string().contains("truncated")), _ => panic!() }
    }

    #[test]
    fn fit_budget_drops_until_fits() {
        let big = "x".repeat(400); // ~100 tokens each
        let msgs = alloc::vec![ user(&big), assistant(&big), user("live") ];
        let cx = TransformCx::pure(&HeuristicEstimator, None);
        let out = FitBudget { max_tokens: 60 }.apply(&cx, msgs).unwrap();
        // turn 0 (the two big messages) dropped; live turn kept
        assert!(matches!(&out[0].payload, MessagePayload::Text { content } if content == "live"));
    }

    #[test]
    fn fit_budget_unsatisfiable_when_live_too_big() {
        let big = "x".repeat(4000);
        let msgs = alloc::vec![ user(&big) ];
        let cx = TransformCx::pure(&HeuristicEstimator, None);
        let err = FitBudget { max_tokens: 10 }.apply(&cx, msgs).unwrap_err();
        assert!(matches!(err, ContextError::BudgetUnsatisfiable { .. }));
    }
}
```

- [ ] **Step 2: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core context::transformers 2>&1 | tail -25`
Expected: PASS (4 tests). Fix `div_ceil` to `(x + 3) / 4` if MSRV rejects it.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/context/transformers.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): trim_old, compact_tool_outputs, fit_budget"
```

---

### Task 8: tau-pkg — parse `[agents.<id>.context]` + capability check

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs`
- Test: `crates/tau-pkg/src/project/project.rs` (inline tests) or `crates/tau-pkg/tests/`

- [ ] **Step 1: Write the failing test**

Add a test that loads a `tau.toml` string with a context block and asserts the parsed `AgentEntry` carries the pipeline (mirror existing `validate_agent` tests). Example:

```rust
#[test]
fn parses_agent_context_pipeline() {
    let toml = r#"
[project]
name = "p"
[agents.a]
display_name = "A"
package = "demo@^0.1"
llm_backend = "mock-llm"
[[agents.a.context.pipeline]]
transformer = "trim_old"
[agents.a.context.steps.trim_old]
keep_last_turns = 4
[[agents.a.context.pipeline]]
transformer = "fit_budget"
[agents.a.context.steps.fit_budget]
max_tokens = 4000
"#;
    let cfg = ProjectConfig::from_toml_str(toml).expect("parse"); // use the crate's actual parse fn
    let agent = cfg.agents().get("a").unwrap();
    assert_eq!(agent.context.len(), 2);
    assert_eq!(agent.context[0].transformer, "trim_old");
    assert_eq!(agent.context[1].transformer, "fit_budget");
}
```

(Match the crate's real parse entrypoint and accessor names.)

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg parses_agent_context 2>&1 | tail -20`
Expected: FAIL — no `context` field.

- [ ] **Step 3: Implement the unchecked structs, the `AgentEntry.context` field, and validation**

In `crates/tau-pkg/src/project/project.rs`:

1. Add to `UncheckedAgent`:
```rust
    /// Optional `[agents.<id>.context]` sub-table (β.4).
    #[serde(default)]
    pub context: Option<UncheckedContext>,
```

2. Add the unchecked structs:
```rust
/// `[agents.<id>.context]` sub-table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedContext {
    /// Ordered pipeline of `[[agents.<id>.context.pipeline]]` entries.
    #[serde(default)]
    pub pipeline: Vec<UncheckedContextStep>,
    /// Per-node config tables: `[agents.<id>.context.steps.<name>]`.
    #[serde(default)]
    pub steps: Option<toml::Table>,
}

/// One `[[agents.<id>.context.pipeline]]` entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedContextStep {
    /// Transformer name (builtin or custom).
    pub transformer: String,
    /// `builtin` (default) | `custom`.
    #[serde(default)]
    pub kind: Option<String>,
    /// For custom nodes: `native` | `wasm` | `mcp`.
    #[serde(default)]
    pub source: Option<String>,
    /// For custom nodes: providing package ref.
    #[serde(default)]
    pub package: Option<String>,
    /// For custom nodes: declared determinism (`pure` default).
    #[serde(default)]
    pub determinism: Option<String>,
}
```

3. Add a validated step type + `context: Vec<ContextStepEntry>` to `AgentEntry`:
```rust
/// Validated context-pipeline step.
#[derive(Debug, Clone)]
pub struct ContextStepEntry {
    /// Transformer name.
    pub transformer: String,
    /// `pure` | `llm_backed` | `stateful`.
    pub determinism: String,
    /// `Some((source, package))` for custom nodes, else `None`.
    pub custom: Option<(String, String)>,
    /// Per-node config from `[...context.steps.<name>]`.
    pub config: std::collections::BTreeMap<String, tau_domain::Value>,
}
```
Add `pub context: Vec<ContextStepEntry>,` to `AgentEntry` and set it in the `Ok(AgentEntry { .. })` literal.

4. In `validate_agent`, build the context after `let config = ...`:
```rust
    let context: Vec<ContextStepEntry> = match raw.context {
        None => Vec::new(),
        Some(ctx) => {
            let steps_tbl = ctx.steps.unwrap_or_default();
            let mut out = Vec::with_capacity(ctx.pipeline.len());
            for s in ctx.pipeline {
                let determinism = s.determinism.unwrap_or_else(|| "pure".into());
                let custom = match s.kind.as_deref() {
                    Some("custom") => {
                        let source = s.source.clone().ok_or_else(|| {
                            ProjectConfigError::AgentValidation {
                                id: id.clone(),
                                message: format!("context custom node {:?} needs `source`", s.transformer),
                            }
                        })?;
                        let package = s.package.clone().ok_or_else(|| {
                            ProjectConfigError::AgentValidation {
                                id: id.clone(),
                                message: format!("context custom node {:?} needs `package`", s.transformer),
                            }
                        })?;
                        if source != "native" {
                            return Err(ProjectConfigError::AgentValidation {
                                id: id.clone(),
                                message: format!("context node {:?}: source {source:?} not supported in v1 (only `native`)", s.transformer),
                            });
                        }
                        Some((source, package))
                    }
                    _ => None,
                };
                let node_cfg = steps_tbl
                    .get(&s.transformer)
                    .and_then(|v| v.as_table())
                    .map(toml_table_to_value_map)
                    .unwrap_or_default();
                out.push(ContextStepEntry { transformer: s.transformer, determinism, custom, config: node_cfg });
            }
            out
        }
    };
```

5. Add the helper `toml_table_to_value_map` converting `toml::Table` → `BTreeMap<String, tau_domain::Value>` (reuse any existing toml→Value bridge in the crate; if none, convert via `serde_json`/`toml::Value` to `tau_domain::Value`).

**Capability check note:** for `custom` nodes that declare non-empty capabilities, the grant check belongs here (intersect against the agent's `capability_overrides`/package grants), mirroring the deliverables fs-write cross-check. v1 builtins declare none, so the check is a no-op for them; implement the subset check and a rejection test in Task 13.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg parses_agent_context 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/project/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-pkg): parse [agents.*.context] into AgentEntry"
```

> After this lands, return to **Task 4** and replace the `<agent entry type>`/accessor placeholders with `ContextStepEntry`/`entry.context`.

---

### Task 9: `build_context_pipeline` (IR → transformer instances)

**Files:**
- Create: `crates/tau-runtime-core/src/context/build.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/tau-runtime-core/src/context/build.rs`:

```rust
use alloc::sync::Arc;
use alloc::vec::Vec;
use tau_ir::context::{ContextConfig, ContextNodeKind};

use super::transformers::{CompactToolOutputs, FitBudget, TrimOld};
use super::ContextTransformer;
use crate::error::RuntimeError;

/// Optional registry of user-supplied (native) custom transformers.
pub trait ContextTransformerRegistry: Send + Sync {
    /// Resolve a custom node by name; `None` if unknown.
    fn resolve(&self, name: &str) -> Option<Arc<dyn ContextTransformer>>;
}

/// Build the concrete per-turn pipeline from an IR `ContextConfig`.
///
/// Builtins are constructed directly from their per-node config. Custom
/// (`ContextNodeKind::Custom`) nodes are resolved via `registry`.
pub fn build_context_pipeline(
    cfg: &ContextConfig,
    registry: Option<&dyn ContextTransformerRegistry>,
) -> Result<Vec<Arc<dyn ContextTransformer>>, RuntimeError> {
    let mut out: Vec<Arc<dyn ContextTransformer>> = Vec::with_capacity(cfg.pipeline.len());
    for step in &cfg.pipeline {
        let t: Arc<dyn ContextTransformer> = match &step.kind {
            ContextNodeKind::Builtin => match step.transformer.as_str() {
                "trim_old" => Arc::new(TrimOld::from_config(&step.config)),
                "compact_tool_outputs" => Arc::new(CompactToolOutputs::from_config(&step.config)),
                "fit_budget" => Arc::new(FitBudget::from_config(&step.config)),
                other => {
                    return Err(RuntimeError::Internal {
                        message: alloc::format!("unknown builtin context transformer '{other}'"),
                    })
                }
            },
            ContextNodeKind::Custom { .. } => {
                let reg = registry.ok_or_else(|| RuntimeError::Internal {
                    message: alloc::format!(
                        "custom context node '{}' but no registry provided",
                        step.transformer
                    ),
                })?;
                reg.resolve(&step.transformer).ok_or_else(|| RuntimeError::Internal {
                    message: alloc::format!("custom context node '{}' not registered", step.transformer),
                })?
            }
        };
        out.push(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::context::{ContextStep, DeterminismClass};

    #[test]
    fn builds_three_builtins() {
        let cfg = ContextConfig {
            pipeline: alloc::vec![
                ContextStep { transformer: "trim_old".into(), determinism: DeterminismClass::Pure, kind: ContextNodeKind::Builtin, config: Default::default() },
                ContextStep { transformer: "fit_budget".into(), determinism: DeterminismClass::Pure, kind: ContextNodeKind::Builtin, config: Default::default() },
            ],
        };
        let pipe = build_context_pipeline(&cfg, None).unwrap();
        assert_eq!(pipe.len(), 2);
        assert_eq!(pipe[0].name(), "trim_old");
        assert_eq!(pipe[1].name(), "fit_budget");
    }
}
```

Add `pub use build::{build_context_pipeline, ContextTransformerRegistry};` to `context/mod.rs`.

- [ ] **Step 2: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core context::build 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/context/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): build context pipeline from IR config"
```

---

### Task 10: `RunOptions` fields + `RuntimeError::ContextPipeline`

**Files:**
- Modify: `crates/tau-runtime-core/src/options.rs`
- Modify: `crates/tau-runtime-core/src/error.rs`

- [ ] **Step 1: Add the error variant**

In `crates/tau-runtime-core/src/error.rs`, in `RuntimeError`:

```rust
    /// A context-pipeline transformer failed or the budget was unsatisfiable.
    #[error("context pipeline failed: {detail}")]
    ContextPipeline {
        /// Human-readable detail (from `ContextError`).
        detail: String,
    },
```

- [ ] **Step 2: Add the RunOptions fields**

In `crates/tau-runtime-core/src/options.rs`, add to `RunOptions`:

```rust
    /// β.4 per-turn context pipeline. Empty = no context management
    /// (full history every turn — pre-β.4 behavior).
    pub context_pipeline: alloc::vec::Vec<alloc::sync::Arc<dyn crate::context::ContextTransformer>>,
    /// Token estimator used by `fit_budget`. Defaults to the heuristic.
    pub token_estimator: alloc::sync::Arc<dyn crate::context::TokenEstimator>,
```

In `RunOptions::default()` (or its builder), initialize:
```rust
            context_pipeline: alloc::vec::Vec::new(),
            token_estimator: alloc::sync::Arc::new(crate::context::HeuristicEstimator),
```

- [ ] **Step 3: Compile-check**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core 2>&1 | tail -25`
Expected: PASS (or surfaces the `Default` impl site to fix; fix it).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-runtime-core/src/options.rs crates/tau-runtime-core/src/error.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): RunOptions context pipeline + estimator"
```

---

### Task 11: Apply the pipeline per-turn at the projection seam + emit events

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs`
- Modify: `crates/tau-runtime-core/src/vocabulary.rs` (mirror `EV_CONTEXT_STEP_RAN`)

- [ ] **Step 1: Add the vocabulary mirror**

In `crates/tau-runtime-core/src/vocabulary.rs`, add the constant and add it to the `PAIRS` array:

```rust
/// Emitted once per context transformer per turn.
pub const EV_CONTEXT_STEP_RAN: &str = "runtime.context_step_ran";
```
And add `("EV_CONTEXT_STEP_RAN", EV_CONTEXT_STEP_RAN),` to the `PAIRS` slice.

- [ ] **Step 2: Insert the pipeline application at `stream.rs:~291`**

Replace the three lines at the request-build site:
```rust
            let mut request = CompletionRequest::new(agent_def.llm_backend.as_str().into());
            request.system = agent_def.system_prompt.clone();
            request.messages = crate::run::agent_messages_to_provider_messages(&messages);
```
with:
```rust
            let mut request = CompletionRequest::new(agent_def.llm_backend.as_str().into());
            request.system = agent_def.system_prompt.clone();
            // β.4: derive a budgeted per-turn VIEW; the stored `messages`
            // (full conversation) is never mutated.
            let provider_messages = if options.context_pipeline.is_empty() {
                crate::run::agent_messages_to_provider_messages(&messages)
            } else {
                let cx = crate::context::TransformCx::pure(
                    options.token_estimator.as_ref(),
                    agent_def.system_prompt.as_deref(),
                );
                let mut view = messages.clone();
                let mut pipeline_failed: Option<String> = None;
                for t in &options.context_pipeline {
                    let before: u32 = view.iter().map(|m| cx.estimate_tokens(m)).sum();
                    match t.transform(&cx, view).await {
                        Ok(next) => {
                            let after: u32 = next.iter().map(|m| cx.estimate_tokens(m)).sum();
                            debug!(
                                parent: &turn_span,
                                name = EV_CONTEXT_STEP_RAN,
                                step = t.name(),
                                tokens_in = before,
                                tokens_out = after,
                            );
                            view = next;
                        }
                        Err(e) => {
                            pipeline_failed = Some(alloc::format!("{e}"));
                            view = alloc::vec::Vec::new();
                            break;
                        }
                    }
                }
                if let Some(detail) = pipeline_failed {
                    yield make_failed_outcome(
                        messages,
                        total_turns,
                        aggregated_tokens,
                        crate::error::RuntimeError::ContextPipeline { detail },
                    );
                    return;
                }
                crate::run::agent_messages_to_provider_messages(&view)
            };
            request.messages = provider_messages;
```

Add `use crate::vocabulary::EV_CONTEXT_STEP_RAN;` to the imports. Use the existing failed-outcome helper — match the name used elsewhere in `stream.rs` (search for how `make_max_turns_outcome` constructs a terminal `RunEvent`; mirror it for the failure). If a `RuntimeError`→outcome helper already exists, use it; otherwise construct the same `RunOutcome::Failed { .. }` yield the file uses for other terminal errors.

- [ ] **Step 3: Run the existing core test suite (no regressions; empty pipeline path unchanged)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core 2>&1 | tail -30`
Expected: PASS (all existing tests green — empty `context_pipeline` ⇒ identical behavior).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-runtime-core/src/stream.rs crates/tau-runtime-core/src/vocabulary.rs
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): apply context pipeline per turn + emit ContextStepRan"
```

---

### Task 12: Wire the pipeline from the IR Agent into `RunOptions`

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs`

- [ ] **Step 1: Build the pipeline where per-agent `RunOptions` is assembled**

Locate where the interpreter builds `RunOptions` for an agent before calling `run_with_history`/`run_streaming_inner` (in `interpreter/agent_loop.rs`, near `split_history`/`run_with_history`). Insert:

```rust
    // β.4: build the per-turn context pipeline from the IR agent's config.
    if let Some(ctx_cfg) = ir_agent.context.as_ref() {
        let registry = dispatcher.context_transformer_registry();
        run_options.context_pipeline =
            crate::context::build_context_pipeline(ctx_cfg, registry.as_deref())?;
    }
```

Where `ir_agent` is the `tau_ir::node::Agent` for the agent being run (thread it in from the `IrModule` if not already in scope), and `run_options` is the mutable `RunOptions` being assembled. Add the `context_transformer_registry()` accessor to the `ToolDispatcher` trait (default `None`), mirroring `deterministic_registry()`:

```rust
    /// Optional registry of user-supplied native context nodes.
    fn context_transformer_registry(
        &self,
    ) -> Option<Arc<dyn crate::context::ContextTransformerRegistry>> {
        None
    }
```

- [ ] **Step 2: Compile-check + core suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core 2>&1 | tail -30`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-runtime-core): wire IR context config into the agent loop"
```

---

## Phase C — Observability, conformance, custom node

### Task 13: `tau-observe` vocabulary + custom-node capability rejection test

**Files:**
- Modify: `crates/tau-observe/src/vocabulary.rs`
- Modify: `crates/tau-runtime-tokio/tests/vocabulary_drift.rs` (bump `OBSERVE_TOTAL_EXPECTED`)
- Test: `crates/tau-pkg/...` (capability rejection)

- [ ] **Step 1: Add the observe constant + its assertion**

In `crates/tau-observe/src/vocabulary.rs` add:
```rust
/// Emitted once per context transformer per turn (β.4).
pub const EV_CONTEXT_STEP_RAN: &str = "runtime.context_step_ran";
```
Add an assertion in the matching `#[test]` block: `assert_eq!(EV_CONTEXT_STEP_RAN, "runtime.context_step_ran");`

- [ ] **Step 2: Update the drift test**

In `crates/tau-runtime-tokio/tests/vocabulary_drift.rs`, increment `OBSERVE_TOTAL_EXPECTED` by 1 (this constant IS mirrored into the kernel — Task 11 added it to `k::PAIRS` — so it is NOT added to `OBSERVE_ONLY`).

- [ ] **Step 3: Capability rejection test (tau-pkg)**

Add a tau-pkg test: a `[agents.a.context]` custom node declaring a capability the agent isn't granted is rejected at validation. (Implement the subset check in `validate_agent` per Task 8 step 3's capability note.) Example asserts `ProjectConfigError` is returned.

- [ ] **Step 4: Run**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-observe -p tau-runtime-tokio vocabulary 2>&1 | tail -20
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg context 2>&1 | tail -20
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-observe/ crates/tau-runtime-tokio/tests/vocabulary_drift.rs crates/tau-pkg/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "feat(tau-observe): ContextStepRan vocabulary + capability rejection"
```

---

### Task 14: Conformance fixture `13_context_pipeline`

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/13_context_pipeline/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/13_context_pipeline/mock_llm.jsonl`
- Create: `crates/tau-ir-conformance/fixtures/13_context_pipeline/expected_report.json`
- Modify: `crates/tau-ir-conformance/tests/conformance.rs`

- [ ] **Step 1: Create the fixture files**

`workflow.toml`:
```toml
[project]
name = "fixture-13"

[agents.mono]
display_name = "Mono"
package      = "demo@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
max_turns    = 2

[[agents.mono.context.pipeline]]
transformer = "trim_old"
[agents.mono.context.steps.trim_old]
keep_last_turns = 4

[[agents.mono.context.pipeline]]
transformer = "compact_tool_outputs"
[agents.mono.context.steps.compact_tool_outputs]
max_bytes = 256

[[agents.mono.context.pipeline]]
transformer = "fit_budget"
[agents.mono.context.steps.fit_budget]
max_tokens = 4000
```

`mock_llm.jsonl`:
```jsonl
{"turn": 0, "response": {"text": "done", "stop_reason": "end_turn"}}
```

`expected_report.json`:
```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": {},
  "message_added_count": 0
}
```

- [ ] **Step 2: Register the test (dev + cross-mode conformance)**

Add to `crates/tau-ir-conformance/tests/conformance.rs`:
```rust
#[tokio::test(flavor = "current_thread")]
async fn fixture_13_dev_mode_completed() {
    let dir = fixture_dir("13_context_pipeline");
    let report = DevMode.run(&dir).await;
    assert!(matches!(report.run_outcome, Some(RunOutcome::Completed { .. })),
        "expected Completed, got {:?}", report.run_outcome);
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_13_cross_mode_conformance() {
    let dir = fixture_dir("13_context_pipeline");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
```

- [ ] **Step 3: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance fixture_13 2>&1 | tail -25`
Expected: PASS (dev completes; dev and bundle event streams agree).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-ir-conformance/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "test(conformance): context pipeline runs identically dev vs bundle"
```

---

### Task 15: Backward-compat + native custom-node integration tests

**Files:**
- Test: `crates/tau-runtime-core/tests/context_integration.rs` (new) or extend an existing integration test using `common::MockLlmBackend`.

- [ ] **Step 1: Backward-compat test (no context block ⇒ no events, unchanged output)**

Write a test running an agent with **no** context block and assert the run completes and emits **zero** `runtime.context_step_ran` events (use the tracing test-recorder pattern from logging §D, or assert via a `RunEvent` collection that no context step ran). Use `common::MockLlmBackend`.

- [ ] **Step 2: Native custom-node test (proves the extension point)**

Define a test-only `ContextTransformer` (e.g. `DropAll` returning only the live turn), register it via a test `ContextTransformerRegistry`, reference it from an IR `ContextConfig` with `kind = Custom { source: "native", .. }`, and assert it ran (e.g. the sent message count shrank). This exercises `build_context_pipeline` + the registry + the per-turn hook end-to-end.

- [ ] **Step 3: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core context_integration 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-runtime-core/tests/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "test(tau-runtime-core): context backward-compat + native custom node e2e"
```

---

## Phase D — CLI surface, ADR, full gate

### Task 16: `tau check`/`build` surface context errors (verify wiring)

**Files:**
- Modify: `crates/tau-cli/...` only if context errors aren't already routed through the existing IR-error renderer.

- [ ] **Step 1: Add a CLI test**

Add a `tau-cli` integration test (mirror `cmd_check_deliverable.rs`): a project with an invalid context pipeline (`fit_budget` not last) → `tau check` exits non-zero and renders `ContextFitBudgetNotLast`. Build/check/run already route IR `typecheck` through their renderers, so this is largely a verification test; add a `cmd_check_context.rs`.

- [ ] **Step 2: Run**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli cmd_check_context 2>&1 | tail -25`
Expected: PASS. If the error isn't rendered, add a match arm in the CLI's IR-error renderer for the three new `IrError` variants.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "test(tau-cli): tau check rejects invalid context pipelines"
```

---

### Task 17: ADR-0045 + docs

**Files:**
- Create: `docs/decisions/0045-context-manager.md`
- Modify: `docs/SUMMARY.md` (if ADRs are listed there)

- [ ] **Step 1: Write the ADR**

Record: layered-hybrid over pipeline-only / MemGPT-in-core; `DeterminismClass` as the conformance boundary; E1 heuristic estimator behind a swappable `TokenEstimator`; the public `ContextTransformer` extension contract + native-first custom-node lane; the four other locked contracts; and the SOTA→tier roadmap. Reference the design spec. Use the existing ADR template (`docs/decisions/template.md`). Note the pre-existing `0044` filename collision as a known follow-up.

- [ ] **Step 2: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build 2>&1 | tail -15 && cd .. && rm -rf docs/book`
Expected: only `[INFO]` lines (no broken links). Ensure the ADR is in `SUMMARY.md`.

- [ ] **Step 3: Commit**

```bash
git add docs/
git -c user.name="Test User" -c user.email="test@example.com" commit -m "docs(adr): record context-manager design (ADR-0045)"
```

---

### Task 18: Full per-crate gate before PR

- [ ] **Step 1: fmt + clippy + tests across touched crates**

```
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check
for c in tau-ir tau-runtime-core tau-pkg tau-observe tau-ir-conformance tau-cli; do
  timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p $c --all-targets 2>&1 | tail -5
done
for c in tau-ir tau-runtime-core tau-pkg tau-observe tau-ir-conformance; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p $c 2>&1 | tail -5
done
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --doc 2>&1 | tail -5
```
Expected: all green.

- [ ] **Step 2: Verify `no_std` core still builds**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core --no-default-features 2>&1 | tail -15`
Expected: PASS — the context module is `no_std`+`alloc` clean (no `std` leak).

- [ ] **Step 3: Open the PR**

```bash
git push -u origin HEAD
gh pr create --base main --title "feat: β.4 context manager (deterministic v1)" --body "Implements docs/superpowers/specs/2026-06-14-beta-4-context-manager-design.md. Opt-in per-agent context pipeline (trim_old/compact_tool_outputs/fit_budget), IR-lowered, runtime-applied per turn, with the public ContextTransformer extension contract (native custom nodes). Deterministic-only v1; LLM/offload/retrieve tiers banked. 🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Self-review

**Spec coverage:** §4 layered hybrid → architecture preserved (Layer 1 only built; 2/3 deferred). §5 trait → Task 6. §6 per-turn seam → Task 11. §7 E1 → Task 5. §8 config → Task 8. §9 five contracts: #1 determinism (Task 1/6), #2 capability decl (Task 6 + 8/13), #3 restorable-handle convention (documented in ADR Task 17; not exercised in v1 — `compact` uses a non-restorable marker per spec), #4 KV-cache invariant (documented in ADR; v1 transformers prefix-stable except fit_budget head-drop — noted), #5 public SDK + open registry + custom node (Tasks 6/9/15). §10 budget/protected/errors → Task 7 (`fit_budget`) + Task 10/11. §11 semantics → Task 7. §12 IR → Tasks 1/3/4. §13 observability → Tasks 11/13. §14 testing → Tasks 14/15. §18 DoD → Tasks 14 (dev+bundle), 15 (custom + backward-compat), 16 (build-time reject), 18 (`no_std`).

**Placeholder scan:** Task 4 and Task 8 intentionally cross-reference each other (parse produces the type lowering consumes); Task 8's note redirects back to Task 4 — this is sequencing, not a placeholder. The `make_failed_outcome` helper in Task 11 says "match the name used in stream.rs" — the implementer must use the file's existing terminal-error helper; the surrounding code is complete.

**Type consistency:** `DeterminismClass` defined once in `tau-ir::context` and re-exported by `tau-runtime-core::context` (Tasks 1/6). `ContextStepEntry` (tau-pkg, Task 8) → `lower_context` (tau-ir, Task 4) → `ContextConfig`/`ContextStep` (tau-ir, Task 1) → `build_context_pipeline` (Task 9) → `RunOptions.context_pipeline` (Task 10) → applied in `stream.rs` (Task 11). `EV_CONTEXT_STEP_RAN` mirrored in both `tau-runtime-core::vocabulary` (Task 11) and `tau-observe::vocabulary` (Task 13) with the drift count bumped.
