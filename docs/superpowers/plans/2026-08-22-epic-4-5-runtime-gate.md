# EPIC 4.5 — Runtime Gate for Dynamic Regions: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `StepRun::Dynamic` execute: a coordinator agent spawns store-backed kinds
via `agent.<kind>.spawn` registry tools, each spawn gated by membership, pooled bounds
counters, and meet-attenuation.

**Architecture:** Spawn-as-tool (Claude Code `Task`-tool shape). Authoring gains runnable
kinds (`[agent.kinds.*]` += description/prompt/model/tools) and a required region owner;
lowering bakes the resolved templates into a self-contained IR (`v2.7.0`); the runtime
gate lives in a new `SpawnTool`'s `invoke()`, reusing `AttenuatedDispatcher` for the
capability clamp. Soft-deny: denials are `is_error` tool results plus named trace events.

**Tech Stack:** Rust workspace (no_std-compatible tau-runtime-core), thiserror, serde,
schemars, nextest, MockLlmBackend-style scripted LLM fixtures.

**Spec:** `docs/superpowers/specs/2026-08-22-epic-4-5-runtime-gate-design.md`

## Global Constraints

- CARGO RULES (workspace `CLAUDE.md`) are mandatory. Template:
  `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate>`
  Main-agent runs use `target/main`; subagents use `target/agent-impl` (or
  `target/agent-<role>`). Never bare `cargo`, never workspace-wide, always `-p`.
- Commits: run the task's gate manually first, then
  `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."`
  (lefthook pre-commit outlives the 2-min Bash timeout; the manual gate replaces it).
  Conventional commits, imperative, scoped. End commit messages with the
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` trailer.
- `#![forbid(unsafe_code)]` everywhere (already set); thiserror at crate boundaries.
- tau-domain is READ-ONLY this slice (consume `meet`/`capability_subset`; change nothing).
- Do NOT touch `crates/tau-runtime-core/tests/ir_dispatch_gate_inert.rs` (#582 pinned) or
  add any general dispatch-site / in-guest gate.
- All crates use `version.workspace = true` — no per-crate version bumps.
- ir_format goes `v2.6.0 → v2.7.0` (MINOR). Called-out decision: the new `DynamicSpawn` /
  `Dynamic` fields are **required** (no serde defaults). A v2.6 module containing
  `Dynamic` fails decode under v2.7 — accepted because v2.6 `Dynamic` was never
  executable (the interpreter unconditionally errored), so no functional bundle regresses.
  Record this rationale in the `module.rs` version comment (Task 2).
- New trace event names (exact): `runtime.dynamic.spawned`,
  `runtime.dynamic.spawn_denied`, `runtime.dynamic.attenuation_denied`.

---

### Task 1: tau-pkg — store-backed kind authoring + region owner/spawns rules

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (structs ~137–150, ~372–386, validated
  `Dynamic` ~491–503, validation ~2129–2154, `agent_kinds` conversion ~1533–1556)
- Modify: `crates/tau-cli/src/cmd/check/categories/governance.rs` (~345, ~410–476 —
  mechanical follow-through of type changes)
- Modify: every fixture `tau.toml` using `[pipeline.steps.dynamic]` or `[agent.kinds.*]`
  (find with `grep -rl "steps.dynamic\|agent.kinds" crates --include=tau.toml`; known:
  `crates/tau-cli/tests/fixtures/wasm-build/needs-dynamic-region/tau.toml` and the
  `cmd_build_dynamic_region.rs` fixtures)
- Test: `crates/tau-pkg/src/project/project.rs` (inline `#[cfg(test)]`, near existing
  `agent_kinds_table_parses_with_capabilities` ~3009)

**Interfaces:**
- Produces: `pub struct ProjectAgentKind { pub name: String, pub capabilities:
  Vec<Capability>, pub description: String, pub prompt: Option<String>, pub model:
  Option<String>, pub tools: Vec<String> }` (tau-pkg), replacing
  `tau_domain::AgentKind` as the value of `ProjectConfig.agent_kinds`.
- Produces: validated `PipelineRunRef::Dynamic { spawns: Vec<String> /* EXPANDED,
  non-empty */, ceiling: Vec<Capability>, max_spawns: u64, max_concurrency: u64,
  agent: String /* now required */ }`.
- Consumed by: Task 2 (lowering reads `ProjectAgentKind` fields + required `agent`),
  governance L4 (reads `.capabilities`, `.get(&agent)`).

- [ ] **Step 1: Write failing parse/validation tests** (inline tests in `project.rs`):

```rust
#[test]
fn agent_kind_parses_runnable_fields() {
    let cfg = parse(r#"
[package]
name = "p"
version = "0.0.1"

[allow]

[agent.kinds.researcher]
description  = "Deep-dives one topic."
prompt       = "Research one topic."
model        = "fast"
tools        = ["probe"]
capabilities = { "net.http" = { hosts = ["crates.io"] } }
"#).expect("parses");
    let k = cfg.agent_kinds.get("researcher").expect("kind present");
    assert_eq!(k.description, "Deep-dives one topic.");
    assert_eq!(k.prompt.as_deref(), Some("Research one topic."));
    assert_eq!(k.model.as_deref(), Some("fast"));
    assert_eq!(k.tools, vec!["probe".to_string()]);
}

#[test]
fn dynamic_region_without_agent_is_rejected() {
    let err = parse(/* minimal project + [pipeline.steps.dynamic] WITHOUT `agent`,
        with [agent.kinds.researcher] and spawns = ["researcher"] */).unwrap_err();
    assert!(err.to_string().contains("dynamic region"), "{err}");
    assert!(err.to_string().contains("agent"), "{err}");
}

#[test]
fn dynamic_region_spawns_omitted_expands_to_whole_store() {
    // Two kinds declared, `spawns` key ABSENT on the region.
    let cfg = parse(/* project with [agent.kinds.a], [agent.kinds.b], region with
        agent = "coord" (declare [agents.coord]) and no spawns key */).expect("parses");
    // Find the validated Dynamic ref and assert spawns == ["a", "b"] (BTreeMap order).
}

#[test]
fn dynamic_region_empty_spawns_is_rejected() {
    // spawns = [] explicitly → error mentioning "no spawnable kinds".
}
```

Use the file's existing `parse(toml) -> Result<ProjectConfig, _>` test helper (see
`agent_kinds_table_parses_with_capabilities` at ~3009 for the exact helper name and the
minimal-project preamble it expects; reuse its preamble verbatim).

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg agent_kind_parses_runnable_fields`
Expected: FAIL (unknown field `description` — `deny_unknown_fields`).

- [ ] **Step 3: Implement authoring changes** in `project.rs`:

```rust
// UncheckedAgentKind (replaces the current 1-field struct at ~137):
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedAgentKind {
    /// The kind's capability grant (kind-as-key raw caps, same shape as `[allow]`).
    #[serde(default)]
    pub capabilities: BTreeMap<String, toml::Value>,
    /// LLM-visible spawn-tool description (EPIC 4.5).
    #[serde(default)]
    pub description: Option<String>,
    /// Child system prompt (inline only in 4.5).
    #[serde(default)]
    pub prompt: Option<String>,
    /// `[models]` alias, resolved at lowering like `[agents.*].model`.
    #[serde(default)]
    pub model: Option<String>,
    /// Tool ids the kind may call (each must exist in `[tools.*]`, typechecked).
    #[serde(default)]
    pub tools: Vec<String>,
}

// New validated type (near ProjectConfig, ~930). ProjectConfig.agent_kinds becomes
// BTreeMap<String, ProjectAgentKind>; tau_domain::AgentKind import stays only if
// still used elsewhere (grep; remove if dead).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectAgentKind {
    pub name: String,
    pub capabilities: Vec<Capability>,
    pub description: String,        // default ""
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<String>,
}
```

Conversion at ~1533: keep the existing raw-caps bridging (same helper the current code
calls), then build `ProjectAgentKind { name, capabilities, description:
uk.description.clone().unwrap_or_default(), prompt: uk.prompt.clone(), model:
uk.model.clone(), tools: uk.tools.clone() }`.

`UncheckedDynamic` at ~372: change `pub spawns: Vec<String>` to
`pub spawns: Option<Vec<String>>` (`#[serde(default)]`). Validation at ~2129:

```rust
// agent now required:
let agent = d.agent.clone().ok_or_else(|| bad(
    id,
    "dynamic region requires `agent` — name the [agents.<id>] coordinator that runs it",
))?;
// store-default expansion (single expansion point; governance + lowering see the
// expanded list):
let spawns: Vec<String> = match &d.spawns {
    Some(list) => list.clone(),
    None => agent_kinds.keys().cloned().collect(), // whole store, BTreeMap order
};
if spawns.is_empty() {
    return Err(bad(
        id,
        "dynamic region has no spawnable kinds — declare [agent.kinds.*] or list `spawns`",
    ));
}
```

(Use the file's existing `bad(...)` error helper — see the current bounds checks at
~2129–2154 for its exact signature — and keep those bounds checks unchanged.)
Validated `PipelineRunRef::Dynamic`: `agent: Option<String>` → `agent: String`.

- [ ] **Step 4: Mechanical follow-through in `governance.rs`** (tau-cli): swap the
`AgentKind` import for `tau_pkg::ProjectAgentKind` (adjust path to wherever tau-pkg
re-exports it — add `pub use` in tau-pkg's lib/project mod), and at ~410–449 replace the
`Option<String>` owner handling: `agent` is now `String`, so the `None → allow.ceiling`
fallback arm is deleted and the owner lookup becomes unconditional
(`project.agents.get(agent)`; a missing owner id keeps producing the existing
finding/err path). Field accesses `.capabilities` at ~345 and ~452 are unchanged.

- [ ] **Step 5: Update all region/kind fixtures** found by the grep in **Files** to the
new syntax — every `[agent.kinds.<k>]` gains `prompt = "You are a <k>."` and
`model = "<an alias that exists in that fixture's [models]>"` (add a `[models]` table if
the fixture lacks one, mirroring the spec example); every `[pipeline.steps.dynamic]`
gains `agent = "<an [agents.*] id present in the fixture>"` (add a minimal
`[agents.coordinator]` with `model`/`prompt` if none exists). Update any test assertions
in `cmd_build_dynamic_region.rs` that snapshot error text or IR JSON.

- [ ] **Step 6: Run tau-pkg + tau-cli tests to green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
Expected: PASS (tau-cli lowering/governance tests may still fail if they consume
lowering — if a failure traces to `tau-ir-lower` reading `ProjectAgentKind`, apply the
minimal field-name fix there now; the real lowering lands in Task 2).

- [ ] **Step 7: Commit**

```bash
git add -A
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify \
  -m "feat(pkg): store-backed agent kinds + required dynamic-region owner (EPIC 4.5)" \
  -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: tau-ir + tau-ir-lower — IR v2.7.0, real kind lowering, typecheck

**Files:**
- Modify: `crates/tau-ir/src/pipeline.rs` (~34–113), `crates/tau-ir/src/module.rs`
  (~43–54, tests ~150), `crates/tau-ir/tests/schema_export.rs` (schema path)
- Create: `schemas/ir/tau-ir.v2.7.0.schema.json` (generated)
- Modify: `crates/tau-ir-lower/src/lower/parse.rs` (~595–618), `crates/tau-ir-lower/src/error.rs`,
  `crates/tau-ir-lower/src/lower/typecheck.rs`
- Modify: `crates/tau-runtime-core/tests/pipeline_control_flow.rs` (~712–773, mechanical
  fixture-field update only — behavior change is Task 4)
- Test: inline tau-ir tests + `crates/tau-ir-lower` existing test modules

**Interfaces:**
- Produces (exact, used by Tasks 3–4):

```rust
pub struct DynamicSpawn {
    pub kind: String,
    pub capabilities: CapabilityRequirements,
    pub description: String,
    pub prompt: crate::prompt::PromptSource,
    pub model_ref: crate::model_ref::ModelRef,
    pub tool_refs: Vec<ToolId>,
}
StepRun::Dynamic {
    owner: AgentId,
    envelope: CapabilityRequirements,
    spawns: Vec<DynamicSpawn>,
    max_spawns: u64,
    max_concurrency: u64,
}
```

- Produces: `LowerError::DynamicKindNotRunnable { kind: String, step: String }`.
- Consumes: Task 1's `ProjectAgentKind` + required `agent: String`.

- [ ] **Step 1: Write failing tau-ir round-trip test** (in `pipeline.rs` tests):

```rust
#[test]
fn dynamic_step_serde_round_trips_with_templates() {
    let p = Pipeline { steps: alloc::vec![PipelineStep {
        id: PipelineStepId("fanout".into()),
        run: StepRun::Dynamic {
            owner: AgentId("coordinator".into()),
            envelope: CapabilityRequirements::default(),
            spawns: alloc::vec![DynamicSpawn {
                kind: "researcher".into(),
                capabilities: CapabilityRequirements::default(),
                description: "Deep-dives one topic.".into(),
                prompt: crate::prompt::PromptSource::inline("Research one topic."),
                model_ref: crate::model_ref::ModelRef {
                    backend: "anthropic".into(),
                    model_id: "claude-haiku-4-5".into(),
                },
                tool_refs: alloc::vec![ToolId("probe".into())],
            }],
            max_spawns: 8,
            max_concurrency: 4,
        },
        input: "${input}".into(),
    }]};
    let bytes = serde_json::to_vec(&p).expect("serializes");
    let back: Pipeline = serde_json::from_slice(&bytes).expect("deserializes");
    assert_eq!(p, back);
}
```

(If `PromptSource::inline` isn't the constructor name, copy whatever
`parse.rs:119–144` uses.)

- [ ] **Step 2: Run to verify failure** (compile error: unknown fields)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir dynamic_step`

- [ ] **Step 3: Implement IR changes.** Add the fields exactly as in **Interfaces**
(doc-comment each; note on `owner`: "the coordinator agent that runs the region — must
exist in `workflow.agents` (typechecked)"). In `module.rs`: `CURRENT` → `"v2.7.0"`, add
version-history comment line:
`// MINOR v2.7.0: Dynamic gains owner + runnable spawn templates (EPIC 4.5). v2.6`
`// Dynamic-bearing modules (never executable — interpreter unconditionally errored)`
`// fail decode; rebuild.`
Update the two version assertions at ~150–151. Fix ALL in-workspace constructors of
`DynamicSpawn`/`StepRun::Dynamic` (compiler-driven): `tau-ir-lower/src/lower/parse.rs`
(real lowering, Step 4), `tau-runtime-core/tests/pipeline_control_flow.rs`
`dynamic_module()` (mechanical: `owner: AgentId("coordinator".into())`, kind fields as in
Step 1's test values — the test still expects the named error until Task 4), plus any
`feature_fit`/`typecheck` test constructors (`grep -rn "StepRun::Dynamic" crates`).

- [ ] **Step 4: Real lowering.** In `parse.rs` (~595–618), the Dynamic arm resolves each
expanded kind name via the `ProjectAgentKind` map (the map the file already builds from
`config.agent_kinds` at ~274 — extend it to carry the full `ProjectAgentKind`, not just
`Vec<Capability>`):

```rust
let mut resolved: Vec<DynamicSpawn> = Vec::new();
for kind in spawns {
    let k = kinds_map.get(kind).ok_or_else(|| LowerError::UnknownAgentKind {
        kind: kind.clone(), step: step_id_string.clone(),
    })?;
    let (Some(prompt), Some(model)) = (&k.prompt, &k.model) else {
        return Err(LowerError::DynamicKindNotRunnable {
            kind: kind.clone(), step: step_id_string.clone(),
        });
    };
    resolved.push(DynamicSpawn {
        kind: kind.clone(),
        capabilities: CapabilityRequirements { declared: k.capabilities.clone() },
        description: k.description.clone(),
        prompt: PromptSource::inline(prompt),
        model_ref: resolve_model_ref(config, model)?,   // parse.rs:335 — reuse as-is
        tool_refs: k.tools.iter().map(|t| ToolId(t.clone())).collect(),
    });
}
StepRun::Dynamic {
    owner: AgentId(agent.clone()),      // Task 1 made this a required String
    envelope: CapabilityRequirements { declared: ceiling.clone() },
    spawns: resolved,
    max_spawns: *max_spawns,
    max_concurrency: *max_concurrency,
}
```

New error variant in `error.rs`, matching the existing style at ~247:

```rust
#[error("dynamic region in step '{step}' offers kind '{kind}' which is not runnable — [agent.kinds.{kind}] must declare `prompt` and `model`")]
DynamicKindNotRunnable { kind: String, step: String },
```

- [ ] **Step 5: Typecheck additions.** In `typecheck.rs`, find the steps-walk arm that
verifies `StepRun::Agent` ids exist in `workflow.agents` (grep `StepRun::Agent`) and add,
following the identical error-construction pattern used there, a `StepRun::Dynamic` arm
checking: (a) `workflow.agents.contains_key(owner)`; (b) every `spawn.tool_refs` entry is
in `workflow.tools`. Reuse the existing "unknown agent/tool reference" error variants if
they carry a free-text location, else add sibling variants in the same style.

- [ ] **Step 6: Lowering tests** (in tau-ir-lower's existing test layout — put them
beside the current `UnknownAgentKind` test, grep for it):
  - kind missing `prompt` → `DynamicKindNotRunnable`.
  - kind `model` alias not in `[models]`/`[allow.models]` → the existing
    `resolve_model_ref` error.
  - happy path: lowered `DynamicSpawn` carries description/prompt/model_ref/tool_refs,
    `owner` set.
  - owner id not in `[agents.*]` → typecheck error; kind tool ref not in `[tools.*]` →
    typecheck error.

- [ ] **Step 7: Regenerate schema.** Update `schema_path()` in `schema_export.rs` to
`tau-ir.v2.7.0.schema.json` (keep the old v2.6.0 file — published history), then:

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl UPDATE_SCHEMA=1 cargo test -p tau-ir --features schema --test schema_export`
Then re-run WITHOUT `UPDATE_SCHEMA` and expect green (drift check passes).

- [ ] **Step 8: Green gates**

Run (each): `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir` / `-p tau-ir-lower` / `-p tau-pkg` / `-p tau-runtime-core` / `-p tau-cli`
Expected: all PASS (runtime-core's dynamic test still asserts the named error — that is
correct until Task 4).

- [ ] **Step 9: Commit** (same identity/no-verify shape as Task 1, message:
`feat(ir): v2.7.0 — runnable spawn templates + region owner; lower store-backed kinds (EPIC 4.5)`)

---

### Task 3: tau-runtime-core — `dynamic.rs`: denial types, counters, SpawnTool (TDD: denial first)

**Files:**
- Create: `crates/tau-runtime-core/src/interpreter/dynamic.rs` (+ register
  `mod dynamic;` in `crates/tau-runtime-core/src/interpreter/mod.rs`)
- Modify: `crates/tau-runtime-core/src/interpreter/attenuate.rs` (event-name field),
  `crates/tau-runtime-core/src/error.rs` (CapabilityDenial Display wording)
- Test: inline `#[cfg(test)]` in `dynamic.rs`

**Interfaces:**
- Consumes: Task 2's `DynamicSpawn` fields; `AttenuatedDispatcher::new` (attenuate.rs:48);
  `run_agent` (agent_loop.rs:673); `tau_domain::package::capability::lattice::meet`.
- Produces (used by Task 4):

```rust
pub(crate) struct RegionCounters { /* max_spawns, max_concurrency, spawned: AtomicU64, in_flight: AtomicU64 */ }
impl RegionCounters { pub(crate) fn new(max_spawns: u64, max_concurrency: u64) -> Self }
pub(crate) struct SpawnTool<D> { /* fields below */ }
impl<D: ToolDispatcher + Send + Sync + 'static> SpawnTool<D> {
    pub(crate) fn new(
        spawn: tau_ir::pipeline::DynamicSpawn,
        envelope: CapabilityRequirements,
        counters: Arc<RegionCounters>,
        region_step: String,
        module: Arc<IrModule>,
        dispatcher: Arc<D>,
    ) -> Self
}
// SpawnTool implements tau_ports::tool::Tool with Session = ()
pub(crate) fn child_grant(envelope: &CapabilityRequirements, kind_caps: &CapabilityRequirements) -> CapabilityRequirements
```

- [ ] **Step 1: Write the FIRST failing test — bounds denial** (this is the slice's TDD
anchor; write it before any implementation):

```rust
#[tokio::test]
async fn spawn_denied_when_max_spawns_exhausted() {
    // Counters pre-saturated: spawned == max_spawns, so invoke() must deny
    // BEFORE constructing any child (no LLM backend needed).
    let counters = Arc::new(RegionCounters::new(1, 1));
    counters.try_admit().expect("first admit"); // saturate: spawned = 1
    let tool = SpawnTool::new(
        test_spawn("researcher"),               // helper below
        CapabilityRequirements::default(),
        counters,
        "fanout".into(),
        test_module(),                          // helper below
        Arc::new(PanicDispatcher),              // panics if a child ever runs
    );
    let mut session = ();
    let res = tool.invoke(&mut session, serde_json::json!({"message": "go"}))
        .await.expect("soft-deny returns Ok(ToolResult)");
    assert!(res.is_error, "denial must be is_error");
    let text = flatten(&res.content);
    assert!(text.contains("spawn denied"), "{text}");
    assert!(text.contains("max_spawns exhausted (1/1)"), "{text}");
    assert!(text.contains("fanout"), "{text}");
    assert!(text.contains("researcher"), "{text}");
}
```

Test helpers in the same module: `test_spawn(kind)` builds a `DynamicSpawn` with inline
prompt `"You are a researcher."`, `ModelRef { backend: "mock", model_id: "m" }`, empty
tool_refs/caps; `test_module()` mirrors `attenuate.rs::module_with_tool`'s `IrModule`
construction (~201–239) with an empty tools map; `PanicDispatcher` implements
`ToolDispatcher` with `invoke`/`llm_backend_for` that `panic!("child must not run")`;
`flatten` maps `ToolContent::Text` to a string (copy the match from agent_loop.rs tests
~1152).

- [ ] **Step 2: Run to verify failure** (module doesn't exist yet)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core spawn_denied`

- [ ] **Step 3: Implement `dynamic.rs`** (no_std style: `alloc::` imports, mirror
attenuate.rs's header):

```rust
//! EPIC 4.5: dynamic-region spawn gate. One `SpawnTool` per offered kind is
//! registered into the coordinator's tool registry (`agent.<kind>.spawn`,
//! Task-tool shape); the admission gate lives in `invoke()`:
//! bounds counters → meet-attenuation → child agent run.

pub(crate) struct RegionCounters {
    max_spawns: u64,
    max_concurrency: u64,
    spawned: core::sync::atomic::AtomicU64,
    in_flight: core::sync::atomic::AtomicU64,
}

pub(crate) enum AdmitError {
    Bounds { spawned: u64, max: u64 },
    Concurrency { in_flight: u64, max: u64 },
}

impl RegionCounters {
    pub(crate) fn new(max_spawns: u64, max_concurrency: u64) -> Self { /* zeros */ }

    /// Admit one spawn: returns its 0-based index, or the typed refusal.
    /// compare_exchange loop on `spawned`; then in_flight increment with a
    /// saturation check (decrement + refuse on overshoot — defensive only,
    /// unreachable under today's sequential per-turn tool dispatch).
    pub(crate) fn try_admit(&self) -> Result<u64, AdmitError> { /* … */ }

    /// Paired with every successful try_admit; call when the child finishes.
    pub(crate) fn release(&self) { /* in_flight -= 1 */ }
}

pub(crate) fn child_grant(
    envelope: &CapabilityRequirements,
    kind_caps: &CapabilityRequirements,
) -> CapabilityRequirements {
    CapabilityRequirements {
        declared: tau_domain::package::capability::lattice::meet(
            &envelope.declared, &kind_caps.declared,
        ),
    }
}
```

`SpawnTool<D>` struct with the **Interfaces** fields plus a precomputed
`tool_name: String` (`alloc::format!("agent.{}.spawn", spawn.kind)`).
`impl Tool for SpawnTool<D>`: `type Session = ()`; `name()` → `&self.tool_name`;
`schema()` →

```rust
ToolSpec {
    name: self.tool_name.clone(),
    description: self.spawn.description.clone(),
    input_schema: serde_json::json!({
        "type": "object",
        "properties": { "message": { "type": "string" } },
        "required": ["message"]
    }),
}
```

(match the exact `ToolSpec` construction style of agent_loop.rs:356–374 — it may go
through serde; copy that mechanism). `init`/`teardown` trivial (mirror DispatcherTool).
`invoke(&self, _session, args)`:

1. Parse `message: String` from args (missing → `Ok(ToolResult)` with `is_error: true`,
   text `"agent.<kind>.spawn: missing required arg `message`"`).
2. `self.counters.try_admit()` — on `Err`, emit

```rust
tracing::warn!(
    name = "runtime.dynamic.spawn_denied",
    region_step = %self.region_step,
    kind = %self.spawn.kind,
    reason = %reason_str,           // "bounds" | "concurrency"
    spawned = counters_snapshot,
    max_spawns = self_max,
);
```

   and return `Ok(ToolResult { is_error: true, content: Text }))` with text
   `"spawn denied: region `{region_step}` max_spawns exhausted ({spawned}/{max}) — kind `{kind}`; proceed with the results you have"`
   (concurrency variant: `"…max_concurrency exceeded ({in_flight}/{max})…"`).
3. Index `n` from try_admit → child id `alloc::format!("{}:{}#{n}", self.region_step, self.spawn.kind)`.
4. `let grant = child_grant(&self.envelope, &self.spawn.capabilities);`
5. Build the child `Agent` value (tau_ir::node::Agent — all fields, mirroring the
   fixture at agent_loop.rs:1095–1111: `prompt: self.spawn.prompt.clone()`,
   `model_ref: self.spawn.model_ref.clone()`, `tool_refs: self.spawn.tool_refs.clone()`,
   `budget: AgentBudget { max_turns: None, max_tokens: None }`, rest default/empty).
6. Emit `tracing::info!(name = "runtime.dynamic.spawned", region_step, kind, child_id, spawned, max_spawns)`.
7. Wrap: `let att = Arc::new(AttenuatedDispatcher::new_with_event(grant,
   ToolId(self.tool_name.clone()), child_id.clone(), self.module.clone(),
   self.dispatcher.clone(), "runtime.dynamic.attenuation_denied"));`
8. `let outcome = alloc::boxed::Box::pin(run_agent(self.module.clone(), &child_agent, att, alloc::vec![user_msg])).await;`
   where `user_msg` mirrors the pipeline Agent-arm's user_message construction
   (pipeline.rs ~700–752 — copy its Message::new call). ALWAYS `self.counters.release()`
   after the await (both arms).
9. Map: run error or `RunOutcome::Failed` → `Ok(ToolResult{is_error: true, text:
   "spawn `{child_id}` failed: {detail}"})`; success → `Ok(ToolResult{is_error: false,
   content: Text(last_assistant_text(&outcome))})` (reuse the crate's existing
   `last_assistant_text` — grep it in pipeline.rs).

`attenuate.rs` change: add field `event: &'static str`; keep `new(...)` delegating to
`new_with_event(..., "runtime.subflow.attenuation_denied")`; the `tracing::warn!` at
~96 uses `name = %self.event` — n.b. `tracing` requires `name` to be a const expression
in some forms; if `name = %self.event` fails to compile, emit
`tracing::warn!(event = %self.event, tool = …, …)` with a literal
`name = "runtime.attenuation_denied"` and assert on the `event` field in tests.
`error.rs`: CapabilityDenial Display `" (narrowed by subflow `{frame}`)"` →
`" (narrowed by `{frame}`)"`; update the one asserting test
(attenuate.rs:265 `"narrowed by subflow `notify`"` → `"narrowed by `notify`"`).

- [ ] **Step 4: Run the denial test to green**, then add the remaining unit tests:

```rust
#[test]
fn child_grant_is_meet_of_envelope_and_kind() {
    // envelope = net.http hosts=["crates.io"]; kind caps = net.http hosts=any
    // (hand-crafted over-reach) → meet = hosts=["crates.io"] (clamped).
    // Reuse attenuate.rs's `cap(toml_str)` helper pattern for construction.
}

#[tokio::test]
async fn spawn_denied_when_concurrency_saturated() {
    // RegionCounters::new(4, 1) with in_flight pre-bumped via try_admit without
    // release → second invoke refused with "max_concurrency exceeded (1/1)".
}

#[tokio::test]
async fn admitted_spawn_indexes_are_sequential() {
    // try_admit → 0, 1, 2; release doesn't affect `spawned`.
}
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core dynamic`
Expected: PASS (all `dynamic.rs` unit tests + existing attenuate tests with the updated
wording).

- [ ] **Step 5: Commit** (`feat(runtime): SpawnTool gate — bounds counters + meet attenuation (EPIC 4.5)`)

---

### Task 4: interpreter wiring — Dynamic arm executes the coordinator; integration tests

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs`
  (`prepare_agent_run` ~436, `run_agent`/`run_agent_streaming` ~673/700),
  `crates/tau-runtime-core/src/interpreter/pipeline.rs` (Dynamic arm ~614–623 + module
  doc ~110–116), `crates/tau-runtime-core/src/error.rs` (delete
  `DynamicRegionRequiresRuntimeGate`)
- Test: `crates/tau-runtime-core/tests/pipeline_control_flow.rs` (~712–773 replaced)

**Interfaces:**
- Consumes: Task 3's `SpawnTool`, `RegionCounters`.
- Produces: `pub(crate) async fn run_agent_with_spawn_tools<D>(module, agent: &Agent,
  dispatcher: Arc<D>, initial_messages: Vec<Message>, spawn_tools:
  Vec<crate::interpreter::dynamic::SpawnTool<D>>) -> Result<RunOutcome, RuntimeError>`.

- [ ] **Step 1: Write failing integration tests** replacing
`dynamic_region_errors_pending_runtime_gate` in `pipeline_control_flow.rs`:

```rust
/// Scripted backend: pops queued CompletionResponses in order. Modeled on
/// tau_ports::fixtures::MockLlmBackend's impl block — copy its trait impl
/// shape and replace the response source with a Mutex<VecDeque<_>>.
struct SeqBackend { responses: std::sync::Mutex<std::collections::VecDeque<CompletionResponse>>, /* + whatever MockLlmBackend's impl needs */ }

#[tokio::test]
async fn dynamic_region_spawns_child_and_completes() {
    // Module: dynamic_module() extended with an [agents] entry "coordinator"
    // (prompt inline, model backend "mock") and owner: "coordinator".
    // Script: r1 = coordinator emits tool_use{name:"agent.researcher.spawn",
    //              input:{"message":"topic A"}} (make_tool_use / make_completion_response
    //              from tau_ports::fixtures, stop_reason ToolUse);
    //         r2 = child's answer: text "CHILD REPORT A", end_turn;
    //         r3 = coordinator final: text "SUMMARY: A", end_turn.
    let out = run_pipeline(Arc::new(module), "x".into(), dispatcher_with(seq_backend))
        .await.expect("region completes");
    // Step output == coordinator's final text:
    assert_eq!(store_output(&out, "spawn-region"), "SUMMARY: A");
    // Non-collision pin (#582-adjacent): the spawn tool_use was answered by the
    // REGISTRY SpawnTool, not the legacy kernel intercept — request 3's message
    // history must contain the child's report, and must NOT contain the
    // intercept's "no orchestration runtime" text.
    let reqs = seq_backend.requests();
    let third = render_messages(&reqs[2]);
    assert!(third.contains("CHILD REPORT A"), "{third}");
    assert!(!third.contains("no orchestration runtime"), "{third}");
}

#[tokio::test]
async fn dynamic_region_soft_denies_past_max_spawns() {
    // Module with max_spawns: 1. Script: r1 = TWO tool_uses in one turn (spawn A,
    // spawn B); r2 = child A text; r3 = coordinator final "DONE".
    // Expect: run COMPLETES (soft-deny), and the coordinator's next request sees
    // the denial text for spawn B:
    let third = render_messages(&reqs[2]);
    assert!(third.contains("spawn denied"), "{third}");
    assert!(third.contains("max_spawns exhausted (1/1)"), "{third}");
}
```

(`dispatcher_with`, `store_output`, `render_messages` are small test helpers: the
dispatcher mirrors the file's existing `dispatcher()` helper with `llm_backend_for`
returning the SeqBackend; `render_messages` flattens a `CompletionRequest`'s messages
to one string. The exact `CompletionRequest` message-walk: copy from any existing
assertion on `invocations()` in the kernel tests — grep `invocations()` in
crates/tau-runtime-core/tests.)

- [ ] **Step 2: Run to verify failure** (still hits the named error)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core dynamic_region`

- [ ] **Step 3: Implement.** (a) `prepare_agent_run` gains
`spawn_tools: Vec<crate::interpreter::dynamic::SpawnTool<D>>` (after the
`agent.tool_refs` loop: `for st in spawn_tools { builder = builder.with_tool(st); }`);
`run_agent`/`run_agent_streaming` pass `alloc::vec![]`; add
`run_agent_with_spawn_tools` per **Interfaces** delegating like `run_agent`.
(b) `pipeline.rs` Dynamic arm (replacing ~614–623; place after `rendered` is available,
mirroring the Suspend/Parallel early-dispatch style):

```rust
if let StepRun::Dynamic { owner, envelope, spawns, max_spawns, max_concurrency } = &step.run {
    let agent = module.workflow.agents.get(owner).ok_or_else(|| RuntimeError::Internal {
        message: alloc::format!(
            "dynamic region '{}' owner '{}' not in workflow.agents (typecheck should reject)",
            step.id.0, owner.0
        ),
    })?.clone();
    let counters = alloc::sync::Arc::new(
        crate::interpreter::dynamic::RegionCounters::new(*max_spawns, *max_concurrency),
    );
    let spawn_tools = spawns.iter().map(|s| crate::interpreter::dynamic::SpawnTool::new(
        s.clone(), envelope.clone(), counters.clone(), step.id.0.clone(),
        module.clone(), dispatcher.clone(),
    )).collect();
    let initial = alloc::vec![user_message(&rendered)];   // same helper as the Agent arm
    let outcome = alloc::boxed::Box::pin(run_agent_with_spawn_tools(
        module.clone(), &agent, dispatcher.clone(), initial, spawn_tools,
    )).await?;
    match outcome {
        RunOutcome::Failed { .. } => { /* same error mapping as the Agent arm */ }
        _ => { store.insert(step.id.0.clone(), Value::String(last_assistant_text(&outcome))); }
    }
    i += 1;
    continue;
}
```

(Match the Agent arm exactly for `user_message`, span instrumentation, failure mapping,
and store insertion — copy, don't improvise. No loop-feedback injection into Dynamic:
retry rewind targets gate agent steps; note that in the arm comment.)
(c) Delete `RuntimeError::DynamicRegionRequiresRuntimeGate` (error.rs ~378–384) and the
now-stale doc-comment lines in pipeline.rs (~110–116, ~614–618) — the doc now says
Dynamic runs the owner with per-kind spawn tools, soft-denying past bounds.

- [ ] **Step 4: Run to green**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS (whole crate — including untouched `ir_dispatch_gate_inert.rs` and
`subflow_attenuation.rs`).

- [ ] **Step 5: Commit** (`feat(runtime): execute dynamic regions — coordinator + spawn tools (EPIC 4.5, closes the 4.4 deferral)`)

---

### Task 5: trace-event assertions

**Files:**
- Modify: `crates/tau-runtime-core/Cargo.toml` (dev-dependency `tracing-subscriber` if
  not already present — check first)
- Test: append to `crates/tau-runtime-core/src/interpreter/dynamic.rs` tests

**Interfaces:** consumes Task 3's emissions; produces nothing new.

- [ ] **Step 1: Write the failing capture test**

```rust
#[tokio::test]
async fn denial_emits_spawn_denied_trace_event() {
    use tracing_subscriber::layer::SubscriberExt;
    let events: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    // Collecting layer: record fmt::format of each event's fields via a
    // tracing_subscriber Layer impl storing `format!("{:?}", event)` strings —
    // or use tracing_subscriber::fmt::TestWriter if the crate's existing tests
    // already have a capture pattern (grep `with_default` in crates/tau-runtime-core
    // first and reuse it verbatim if found).
    let subscriber = tracing_subscriber::registry().with(CollectingLayer(events.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    // …drive the saturated-counters denial exactly as in
    // spawn_denied_when_max_spawns_exhausted…
    let dump = events.lock().unwrap().join("\n");
    assert!(dump.contains("runtime.dynamic.spawn_denied"), "{dump}");
    assert!(dump.contains("fanout"), "{dump}");
}
```

with `CollectingLayer` (~15 lines): `impl<S: Subscriber> Layer<S> for CollectingLayer`
whose `on_event` pushes `format!("{:?}", event)` — the Debug rendering includes the
`name`/field values.

- [ ] **Step 2: Run (fail if emission missing/misnamed), fix, re-run to green, and add
the mirror assertion for `runtime.dynamic.spawned`** inside
`dynamic_region_spawns_child_and_completes` (Task 4's test file) using the same layer.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core trace`

- [ ] **Step 3: Commit** (`test(runtime): pin dynamic-region trace events`)

---

### Task 6: conformance fixture

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/<NN>_dynamic_region/` (NN = next free
  number; copy the directory layout of `01_agent_native_tool/` exactly — inspect it
  first: it contains at least `tau.toml` + `mock_llm.jsonl` + whatever
  expectation/registration file the harness uses; mirror ALL of them)
- Modify: the conformance harness's fixture registry (wherever `01_agent_native_tool` is
  listed — grep the string in `crates/tau-ir-conformance`)

**Interfaces:** consumes the full 4.5 surface end-to-end; produces the DoD's "one
conformance fixture".

- [ ] **Step 1: Author the fixture.** `tau.toml`:

```toml
[package]
name = "dynamic-region-conformance"
version = "0.0.1"

[allow]
"net.http" = { hosts = ["conformance.test"] }

[models]
fast = { backend = "mock", model = "m" }

[agents.coordinator]
model  = "fast"
prompt = "Spawn researchers for each topic, then summarize."

[agent.kinds.researcher]
description  = "Researches one topic."
prompt       = "Research the topic you are given."
model        = "fast"
capabilities = {}

[[pipeline.steps]]
id    = "fanout"
input = "${input}"

[pipeline.steps.fanout.dynamic]
agent           = "coordinator"
spawns          = ["researcher"]
ceiling         = {}
max_spawns      = 1
max_concurrency = 1
```

`mock_llm.jsonl` (harness turn format — verify field names against
`01_agent_native_tool/mock_llm.jsonl` and adjust):

```jsonl
{"turn": 0, "response": {"tool_uses": [{"id": "1", "name": "agent.researcher.spawn", "input": {"message": "topic A"}}, {"id": "2", "name": "agent.researcher.spawn", "input": {"message": "topic B"}}], "stop_reason": "tool_use"}}
{"turn": 1, "response": {"text": "REPORT A", "stop_reason": "end_turn"}}
{"turn": 2, "response": {"text": "SUMMARY", "stop_reason": "end_turn"}}
```

Expectations (in the harness's format): the run completes; the event stream contains a
`ToolCallCompleted` for `agent.researcher.spawn` with a success result carrying
`REPORT A` and a second one with an error result containing
`max_spawns exhausted (1/1)`; final output `SUMMARY`.

- [ ] **Step 2: Run to green** (fix harness-format mismatches by diffing against
fixture 01, not by weakening assertions)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`

- [ ] **Step 3: Commit** (`test(conformance): dynamic-region runtime fixture — spawn + bounds denial`)

---

### Task 7: docs + roadmap

**Files:**
- Modify: `docs/explanation/dynamic-regions.md` ("Runtime execution" section, lines
  ~124–131, plus the authoring section's kind example), `docs/superpowers/plans/vision-roadmap.md`
  (4.5 bullet ~169–173)

- [ ] **Step 1: Rewrite the "Runtime execution" section** — replace the stub with (adjust
prose freely, keep all facts):

```markdown
## Runtime execution (EPIC 4.5)

At runtime a dynamic region runs its owning **coordinator** agent (`agent = "..."`,
required). Every kind the region offers appears in the coordinator's tool list as
`agent.<kind>.spawn` — an ordinary tool (the same shape as a coding harness's
subagent/Task tool), described to the LLM by the kind's `description`. Spawning is a
tool call; the admission gate runs inside it:

1. **Membership** — by construction: only offered kinds are registered as tools.
2. **Bounds** — one pooled counter per region instance: past `max_spawns`, the call
   is **soft-denied** — an error tool-result the coordinator sees and must adapt to;
   the run does not abort. `max_concurrency` is guarded the same way.
3. **Attenuation** — the child's grant is `meet(ceiling, kind.capabilities)` (the
   sound lattice meet), enforced on every child tool call. This runtime clamp is what
   makes the build-time L1 spawn-cap deferral sound, including against hand-crafted IR.

Each admitted spawn runs the kind's own agent definition (`prompt`/`model`/`tools`)
as child `<region-step>:<kind>#<n>`; its final text returns as the tool result. The
region step's output is the coordinator's final text.

Every gate action is observable: denials surface as error `ToolCallCompleted` events
in the run stream, and as `runtime.dynamic.spawned` / `runtime.dynamic.spawn_denied` /
`runtime.dynamic.attenuation_denied` trace events (drop rows in `tau run --tui`) — a
bounded-out run is auditable without reading the coordinator's prose.

**wasm divergence (explicit):** dynamic regions are native-only. `tau build --target
wasm` rejects any workflow containing one at build time (`FeatureUnsupported`), so the
guest interpreter never sees a region and carries no gate.
```

Also update the authoring section: kind example gains
`description`/`prompt`/`model`/`tools`; region example gains required `agent` and notes
`spawns` is optional (omitted ⇒ whole store, build fails loudly if any kind exceeds the
ceiling); update the "spawns kinds on demand" intro if it references the 4.4 refusal.

- [ ] **Step 2: Roadmap** — 4.5 bullet → `✅ SHIPPED 2026-08-22` with one delta line
("a user can run a bounded dynamic region: coordinator spawns store-backed kinds via
`agent.<kind>.spawn`, gated by membership + bounds + meet-attenuation") and note the
epic DoD line is now fully met.

- [ ] **Step 3: Build the book**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && rm -rf book`
Expected: only `[INFO]` lines.

- [ ] **Step 4: Commit** (`docs: dynamic-region runtime half — store, spawn tools, gate, wasm divergence (EPIC 4.5)`)

---

### Task 8: full gate, PR, follow-ups

- [ ] **Step 1: Full verification** (all from repo root, sequentially):

```bash
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt --all --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir -p tau-pkg -p tau-ir-lower -p tau-runtime-core -p tau-cli -p tau-ir-conformance --all-targets
for c in tau-ir tau-pkg tau-ir-lower tau-runtime-core tau-cli tau-ir-conformance; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p $c || exit 1
done
for c in tau-ir tau-pkg tau-ir-lower tau-runtime-core; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p $c || exit 1
done
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --features schema --test schema_export
```

All green before proceeding. Fix regressions at the task they belong to.

- [ ] **Step 2: Push + PR**

```bash
git push -u origin feat/epic-4-5-runtime-gate
gh pr create --base main --title "feat(runtime): dynamic-region runtime gate — spawn tools + bounds + meet attenuation (EPIC 4.5)" \
  --body "Closes #402. <summary per spec> …

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge <N> --squash --auto     # BARE enroll — no --delete-branch
```

- [ ] **Step 3: Follow-up check** (session-end requirement):

```bash
gh issue view 613 --json state -q .state
```

If OPEN and unclaimed report: "READY: #613 — your runtime gate is its runtime
counterpart; a build-time L3 for skill.spawn now has a surface to check against." Else
report "no new lanes unblocked".
