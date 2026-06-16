# Per-agent / per-judge model resolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `agent.model` and a deliverable's `judge_model` real — resolved at build time through a declared `[models]` table and driven into the actual LLM request at runtime — replacing today's single-ambient-backend behaviour.

**Architecture:** A `[models]` table maps an author alias → concrete `{ backend, model }`. Lowering resolves every alias into a `ModelRef` baked into the IR (`tau is a compiler`). The dispatcher becomes multi-backend (`llm_backend_for(name)`), and the synthesized `AgentDefinition` carries the resolved model id so the `CompletionRequest` gets a real model string. The builtin deliverable judge becomes the implicit `JudgeRef::Default`, defaulting to its producer agent's model.

**Tech Stack:** Rust workspace (8 crates), `serde`, `toml`, `serde_json`. Spec: `docs/superpowers/specs/2026-06-16-per-agent-model-resolution-design.md` (decisions D1–D7).

> **CARGO RULES (from CLAUDE.md — non-negotiable):** every cargo command is
> `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`.
> Never bare `cargo`, always `-p` a single crate, always `timeout`
> (test 300 / build·check 180 / clippy 240). Prefer `cargo nextest run`;
> use `cargo test --doc` for doctests. Branch off `main`, PR to `main`
> (branch protection on — never push to main).

> **IR FORMAT VERSION:** this is a **breaking** IR change → bump
> `IrFormatVersion::CURRENT` **v1.2.0 → v2.0.0** (Task 7). A drift test in
> `tau-runtime-tokio` mirrors the version; update it too (Task 18).

> **TRACK 2 COORDINATION:** the concurrent "agent `output_schema`" track also
> edits `ir::Agent`, `lower/parse.rs`, and conformance fixtures. No logic
> conflict (`output_schema` ⟂ `model_ref`). If Track 2 has already landed when
> you start, rebase: keep its `output_schema` field on `Agent`, add `model_ref`
> alongside, and the version is already ≥ v1.3.0 → make it v2.0.0.

---

## File Structure (what changes, by responsibility)

| File | Responsibility | Change |
|---|---|---|
| `crates/tau-pkg/src/project/project.rs` | config surface + validation | add `ModelEntry` + `ProjectConfig.models`; **remove** `AgentEntry.llm_backend`; rename `JudgeConfig::Builtin`→`Default`; new error variants |
| `crates/tau-pkg/src/project/*` (parser) | TOML → `UncheckedProjectConfig` | parse `[models]`; drop `llm_backend` parse |
| `crates/tau-domain/src/agent.rs` | runtime agent definition | add `AgentDefinition.model` + `with_model` |
| `crates/tau-ir/src/model_ref.rs` (new) | resolved model type | `ModelRef { backend, model_id }` |
| `crates/tau-ir/src/node.rs` | IR Agent node | `model: String` → `model_ref: ModelRef` |
| `crates/tau-ir/src/check.rs` | IR judge ref | `JudgeRef::Builtin`→`Default { model_ref }` |
| `crates/tau-ir/src/module.rs` | format version | bump to `v2.0.0` |
| `crates/tau-ir/src/lower/parse.rs` | lowering | resolve alias → `ModelRef`; judge default → producer model_ref |
| `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` | dispatcher trait | `llm_backend()` → `llm_backend_for(name)` |
| `crates/tau-runtime-core/src/interpreter/agent_loop.rs` | agent prep | resolve via `llm_backend_for`, bake model id |
| `crates/tau-runtime-core/src/stream.rs` | request build | use `agent_def.model` |
| `crates/tau-runtime-core/src/interpreter/check.rs` | judge synth | `JudgeRef::Default { model_ref }` |
| `crates/tau-cli/src/cmd/ir_dispatcher.rs` | host dispatcher | multi-backend `ForwardingDispatcher` |
| `crates/tau-cli/src/cmd/check*.rs` | `tau check` | `BackendNotLlmCapable` finding |
| `crates/tau-ts-extract/src/*` | TS authoring parity | extract `[models]` + agent `model` alias |
| `crates/tau-ir-conformance/fixtures/*` | conformance | new fixture + migrate existing |
| `docs/decisions/00NN-*.md` | ADR | record D1–D7 |

---

## Phase A — tau-pkg config surface + validation

### Task 1: Add the `[models]` table type and `ProjectConfig.models`

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (near `ProjectConfig`, line ~662)
- Test: same file's `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn model_entry_holds_backend_and_model() {
    let m = ModelEntry { backend: "anthropic".into(), model: "claude-haiku-4-5".into() };
    assert_eq!(m.backend, "anthropic");
    assert_eq!(m.model, "claude-haiku-4-5");
}
```

- [ ] **Step 2: Run it (fails — type missing)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg model_entry_holds`
Expected: compile error `cannot find type ModelEntry`.

- [ ] **Step 3: Add the type and the field**

Add near the other validated entry structs:

```rust
/// Validated `[models.<alias>]` entry: a concrete backend + vendor model id.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Backend package name (must be a declared package).
    pub backend: String,
    /// Vendor model id (e.g. `"claude-haiku-4-5"`). Trusted; not validated offline.
    pub model: String,
}
```

In `struct ProjectConfig` (after `deliverables`, line ~680) add:

```rust
    /// Map of model alias → validated `{ backend, model }`.
    pub models: BTreeMap<String, ModelEntry>,
```

Initialise it wherever `ProjectConfig { … }` is constructed in `validate()` (search `ProjectConfig {`), e.g. `models,` after building the map (Task 3 fills the map; for now wire an empty `BTreeMap::new()` so it compiles).

- [ ] **Step 4: Run it (passes)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg model_entry_holds`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git commit -m "feat(pkg): add ModelEntry + ProjectConfig.models"
```

### Task 2: Parse `[models]` from TOML into `UncheckedProjectConfig`

**Files:**
- Modify: the parser that builds `UncheckedProjectConfig` (search `UncheckedProjectConfig` and the `[models]`-sibling tables like `tools`/`steps`; mirror their `serde`/manual parse).
- Test: `crates/tau-pkg/src/project/project.rs` tests

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn models_table_parses() {
    let toml = r#"
        [project]
        name = "p"
        [models]
        haiku = { backend = "anthropic", model = "claude-haiku-4-5" }
    "#;
    let cfg = UncheckedProjectConfig::from_toml_str(toml).unwrap().validate().unwrap();
    assert_eq!(cfg.models["haiku"].backend, "anthropic");
    assert_eq!(cfg.models["haiku"].model, "claude-haiku-4-5");
}
```
(Adjust `from_toml_str`/`validate` to the crate's real entry points — search an existing parse test and copy its shape.)

- [ ] **Step 2: Run it (fails)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg models_table_parses`
Expected: FAIL (`models` empty or field unknown).

- [ ] **Step 3: Implement parse**

Add an `Unchecked`-side raw representation mirroring `ModelEntry` (a `#[derive(Deserialize)]` struct with `backend: String, model: String`) and a `models: BTreeMap<String, RawModelEntry>` field (serde `#[serde(default)]`) on the unchecked config. In `validate()` map each raw entry into `ModelEntry` (validation of contents is Task 4).

- [ ] **Step 4: Run it (passes)**

Run: same as Step 2. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/project/
git commit -m "feat(pkg): parse [models] table"
```

### Task 3: Remove `AgentEntry.llm_backend` (hard cutover, D3)

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`AgentEntry` line ~727, `AgentEntry::new` line ~765, the `[agents]` parser, all in-crate constructions)

- [ ] **Step 1: Delete the field and its constructor parameter**

Remove `pub llm_backend: String,` (line ~727). In `AgentEntry::new` (line ~765) remove the `llm_backend: String` parameter and the `llm_backend,` initializer. Remove `llm_backend` from the `[agents]` TOML parse and from any `UncheckedAgent`/`AgentEntry` literal in the crate. Update the doctest at lines ~700-713 (drop the `"anthropic"` argument).

- [ ] **Step 2: Build to find every break**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg`
Expected: errors listing each remaining `llm_backend` reference. Fix each (tests, fixtures-in-crate, `build_agent_definition` in `crates/tau-pkg/src/project/agent.rs:259` — see Task 12 for the runtime side).

> NOTE: `build_agent_definition` currently feeds `llm_backend` into the kernel. After D4, the model+backend come from the IR/`[models]`, not from `AgentEntry`. For this task, make `build_agent_definition` stop reading `llm_backend`; the resolved backend now flows through lowering → IR (Phase B/C). If `build_agent_definition` still needs *a* backend for a legacy path, derive it from the agent's resolved `[models]` entry (look up `config.models[entry.model].backend`).

- [ ] **Step 3: Run tau-pkg tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS (after fixing in-crate fixtures to drop `llm_backend` / add `[models]`).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-pkg/
git commit -m "feat(pkg)!: remove AgentEntry.llm_backend (Schema 1 hard cutover)"
```

### Task 4: `validate_models` — Stage-1 build-time refusals (D7)

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`ProjectConfigError` enum; `validate()`; add `fn validate_models`)
- Test: same file

- [ ] **Step 1: Write failing tests (one per refusal)**

```rust
#[test]
fn unknown_model_alias_is_refused() {
    let toml = r#"
        [project]
        name="p"
        [models]
        haiku = { backend="anthropic", model="claude-haiku-4-5" }
        [packages]
        anthropic = "1.0.0"
        [agents.writer]
        model = "haiko"
        prompt = "hi"
    "#;
    let err = UncheckedProjectConfig::from_toml_str(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::UnknownModelAlias { .. }), "{err:?}");
}

#[test]
fn model_backend_must_be_declared() {
    let toml = r#"
        [project]
        name="p"
        [models]
        gpt = { backend="openai", model="gpt-5" }
        [agents.writer]
        model = "gpt"
        prompt = "hi"
    "#;
    let err = UncheckedProjectConfig::from_toml_str(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::ModelBackendNotDeclared { .. }), "{err:?}");
}

#[test]
fn agent_without_model_is_refused() {
    let toml = r#"
        [project]
        name="p"
        [agents.writer]
        prompt = "hi"
    "#;
    let err = UncheckedProjectConfig::from_toml_str(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::MissingAgentModel { .. }), "{err:?}");
}
```
(`[packages]` must be the real package-declaration table — match how existing tests declare a package. If the project's package set is keyed differently, resolve "declared backend" against that set.)

- [ ] **Step 2: Run them (fail)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg model`
Expected: FAIL (variants missing).

- [ ] **Step 3: Add variants + the validation pass**

Add to `ProjectConfigError`:

```rust
    /// A `[models]` entry is missing `backend` or `model`.
    #[error("model alias `{alias}` is malformed: needs both `backend` and `model`")]
    MalformedModelEntry { alias: String },
    /// A `[models]` entry references a backend that is not a declared package.
    #[error("model alias `{alias}` references undeclared backend `{backend}`")]
    ModelBackendNotDeclared { alias: String, backend: String },
    /// `agent.model` / `judge_model` references an alias absent from `[models]`.
    #[error("{referrer} references unknown model alias `{alias}`")]
    UnknownModelAlias { referrer: String, alias: String },
    /// An agent declares no `model`.
    #[error("agent `{agent}` has no `model` (declare one in `[models]` and reference it)")]
    MissingAgentModel { agent: String },
```

Add `fn validate_models(&self) -> Result<(), ProjectConfigError>` and call it at the end of `validate()` (next to `validate_postconditions`). Logic:

```rust
// 1. every [models] entry's backend must be a declared package
for (alias, m) in &self.models {
    if m.backend.is_empty() || m.model.is_empty() {
        return Err(ProjectConfigError::MalformedModelEntry { alias: alias.clone() });
    }
    if !self.is_declared_package(&m.backend) {   // reuse the existing package-set lookup
        return Err(ProjectConfigError::ModelBackendNotDeclared {
            alias: alias.clone(), backend: m.backend.clone(),
        });
    }
}
// 2. every agent must declare a model that resolves
for (id, a) in &self.agents {
    if a.model.is_empty() {
        return Err(ProjectConfigError::MissingAgentModel { agent: id.clone() });
    }
    if !self.models.contains_key(&a.model) {
        return Err(ProjectConfigError::UnknownModelAlias {
            referrer: format!("agent `{id}`"), alias: a.model.clone(),
        });
    }
}
// 3. every deliverable judge_model (when present) must resolve
for (id, d) in &self.deliverables {
    if let JudgeConfig::Default { model: Some(alias) } = &d.judge {
        if !self.models.contains_key(alias) {
            return Err(ProjectConfigError::UnknownModelAlias {
                referrer: format!("deliverable `{id}` judge_model"), alias: alias.clone(),
            });
        }
    }
}
```
(`is_declared_package` / the package-set accessor: find how `validate_postconditions` or package validation checks a package reference and reuse it. Malformed-entry may already be caught by Task 2's deserialize if fields are required — keep the guard regardless.)

- [ ] **Step 4: Run them (pass)**

Run: same as Step 2. Expected: PASS. Also run full `-p tau-pkg` suite.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git commit -m "feat(pkg): validate_models build-time refusals (D7 stage 1)"
```

### Task 5: Rename `JudgeConfig::Builtin` → `Default` (D5 nomenclature)

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs:616` (`JudgeConfig`); the `[deliverables]` judge parser; `validate_postconditions` (`UnknownJudgeAgent` path)

- [ ] **Step 1: Rename the variant**

```rust
pub enum JudgeConfig {
    /// The canonical (implicit) judge, optional `judge_model` alias override.
    Default { model: Option<String> },
    /// A user `[agents.*]` used as judge.
    Agent(String),
}
```
Update the deliverable parser: a deliverable with no `judge` agent named → `JudgeConfig::Default { model: judge_model_alias }`; `judge = "<id>"` → `JudgeConfig::Agent(id)`. Update every `JudgeConfig::Builtin` match arm in the crate.

- [ ] **Step 2: Build + test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git commit -m "refactor(pkg): JudgeConfig::Builtin -> Default"
```

---

## Phase B — tau-domain + tau-ir types

### Task 6: `AgentDefinition.model` field (D4)

**Files:**
- Modify: `crates/tau-domain/src/agent.rs` (`AgentDefinition` struct ~157, `new` ~176, builders ~215)
- Test: same file

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn agent_definition_carries_model() {
    let def = AgentDefinition::new(/* …existing args… */)
        .with_model("claude-haiku-4-5".into());
    assert_eq!(def.model, "claude-haiku-4-5");
}
```
(Fill `new`'s existing args from the current signature at line 176.)

- [ ] **Step 2: Run (fails)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-domain agent_definition_carries_model`
Expected: FAIL.

- [ ] **Step 3: Add field + builder**

Add `pub model: String,` to the struct; default it to `String::new()` in `new`; add:

```rust
/// Set the resolved vendor model id used to build the LLM request.
pub fn with_model(mut self, model: String) -> Self {
    self.model = model;
    self
}
```

- [ ] **Step 4: Run (passes)**

Run: same as Step 2. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-domain/src/agent.rs
git commit -m "feat(domain): AgentDefinition.model + with_model"
```

### Task 7: `ModelRef` type + `Agent.model_ref` + `JudgeRef::Default` + version bump

**Files:**
- Create: `crates/tau-ir/src/model_ref.rs`
- Modify: `crates/tau-ir/src/lib.rs` (module decl + re-export), `node.rs:36`, `check.rs:73`, `module.rs:30`
- Test: `crates/tau-ir/src/model_ref.rs`

- [ ] **Step 1: Write the new type with a test**

`crates/tau-ir/src/model_ref.rs`:

```rust
//! Resolved model reference: the concrete `{ backend, model_id }` an alias
//! lowered to. The IR never carries the source-level alias (D2).

use alloc::string::String;
use serde::{Deserialize, Serialize};

/// A concrete, build-time-resolved model selection.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    /// Backend package name — the key the runtime resolves a backend by.
    pub backend: String,
    /// Vendor model id placed into `CompletionRequest.model`.
    pub model_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_json() {
        let m = ModelRef { backend: "anthropic".into(), model_id: "claude-haiku-4-5".into() };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<ModelRef>(&s).unwrap(), m);
    }
}
```

- [ ] **Step 2: Wire module + re-export**

In `crates/tau-ir/src/lib.rs` add `mod model_ref;` (matching the file's module style) and re-export `pub use model_ref::ModelRef;` next to the other root re-exports (the crate re-exports check types at root — see commit f15cab0 "re-export check types").

- [ ] **Step 3: Change `Agent.model` → `model_ref`**

In `node.rs`, replace field (line 35-36):

```rust
    /// Resolved model selection (backend + vendor id), baked at lowering.
    pub model_ref: crate::model_ref::ModelRef,
```

- [ ] **Step 4: Change `JudgeRef`**

In `check.rs` (line 73-81):

```rust
pub enum JudgeRef {
    /// The canonical judge, on a build-time-resolved model.
    Default {
        /// Resolved model (alias resolved + producer-default applied at lowering).
        model_ref: crate::model_ref::ModelRef,
    },
    /// A user `[agents.*]` used as judge.
    Agent(AgentId),
}
```

- [ ] **Step 5: Bump version**

In `module.rs` line 30: `pub const CURRENT: &'static str = "v2.0.0";`

- [ ] **Step 6: Build (lowering + runtime will break — expected; fixed in Tasks 8–11)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-ir`
Expected: errors in `lower/parse.rs` (Task 8) — that's the next task. The `model_ref` unit test compiles. Confirm no errors *other* than the lowering literal at `parse.rs:116` and judge lowering.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir/src/model_ref.rs crates/tau-ir/src/lib.rs crates/tau-ir/src/node.rs crates/tau-ir/src/check.rs crates/tau-ir/src/module.rs
git commit -m "feat(ir)!: ModelRef + Agent.model_ref + JudgeRef::Default; bump v2.0.0"
```

---

## Phase C — tau-ir lowering (alias resolution + judge default)

### Task 8: Resolve `agent.model` alias → `Agent.model_ref` at lowering (D2)

**Files:**
- Modify: `crates/tau-ir/src/lower/parse.rs` (agent loop, lines 98-126)
- Test: `crates/tau-ir/src/lower/parse.rs` tests (or the lower module's test file)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn agent_model_alias_lowers_to_model_ref() {
    // Build a ProjectConfig with [models] haiku -> {anthropic, claude-haiku-4-5}
    // and one agent referencing "haiku". Use the crate's existing test
    // helper for ProjectConfig construction (search lower tests for a builder).
    let cfg = /* ProjectConfig with models + agent.model = "haiku" */;
    let parsed = parse(&cfg).unwrap();
    let agent = parsed.agents.values().next().unwrap();
    assert_eq!(agent.model_ref.backend, "anthropic");
    assert_eq!(agent.model_ref.model_id, "claude-haiku-4-5");
}
```

- [ ] **Step 2: Run (fails)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir agent_model_alias_lowers`
Expected: FAIL.

- [ ] **Step 3: Resolve in the agent loop**

Replace `model: entry.model.clone(),` (parse.rs:116) with a lookup. Add a helper at the top of `parse`:

```rust
let resolve_model = |alias: &str| -> Result<crate::model_ref::ModelRef, IrError> {
    let m = config.models.get(alias).ok_or_else(|| {
        IrError::Parse(alloc::format!("model alias `{alias}` not in [models]"))
    })?;
    Ok(crate::model_ref::ModelRef { backend: m.backend.clone(), model_id: m.model.clone() })
};
```
Then in the `Agent { … }` literal: `model_ref: resolve_model(&entry.model)?,`.

> Build-time validation (Task 4) already guarantees the alias exists, so this
> lookup is infallible in practice; the `IrError` arm is defense-in-depth.

- [ ] **Step 4: Run (passes)**

Run: same as Step 2. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/lower/parse.rs
git commit -m "feat(ir): lower agent.model alias to ModelRef"
```

### Task 9: Lower the deliverable judge → `JudgeRef::Default` with producer-default (D5/Q4)

**Files:**
- Modify: `crates/tau-ir/src/lower/parse.rs` (`lower_checks`, ~line 283; judge mapping ~line 448)
- Test: lower tests

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn judge_model_alias_lowers() {
    // deliverable with judge_model = "opus"
    let cfg = /* models has opus; deliverable.judge = Default{Some("opus")} */;
    let parsed = parse(&cfg).unwrap();
    let check = parsed.checks.values().next().unwrap();
    if let CheckVerify::Deliverable { judge: JudgeRef::Default { model_ref }, .. } = &check.verify {
        assert_eq!(model_ref.model_id, "claude-opus-4-8");
    } else { panic!("expected Default judge") }
}

#[test]
fn judge_default_inherits_producer_model() {
    // deliverable with judge = Default{None}; producer agent "writer" uses "haiku"
    let cfg = /* writer.model = "haiku"; deliverable.producer = "writer"; judge Default{None} */;
    let parsed = parse(&cfg).unwrap();
    let check = parsed.checks.values().next().unwrap();
    if let CheckVerify::Deliverable { judge: JudgeRef::Default { model_ref }, .. } = &check.verify {
        assert_eq!(model_ref.model_id, "claude-haiku-4-5"); // == producer's
    } else { panic!() }
}
```

- [ ] **Step 2: Run (fail)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir judge`
Expected: FAIL.

- [ ] **Step 3: Map judge in `lower_checks`**

Where the deliverable judge is lowered, replace the `JudgeConfig::Builtin { model }` arm:

```rust
let judge = match &deliverable.judge {
    JudgeConfig::Agent(id) => JudgeRef::Agent(AgentId(id.clone())),
    JudgeConfig::Default { model } => {
        let model_ref = match model {
            Some(alias) => resolve_model(alias)?,                 // explicit judge_model
            None => {
                // Q4: inherit the producer agent's resolved model.
                // deliverable.producer is filled by validate_postconditions.
                let producer = config.agents.get(&deliverable.producer).ok_or_else(|| {
                    IrError::Parse(alloc::format!(
                        "deliverable producer `{}` not found", deliverable.producer))
                })?;
                resolve_model(&producer.model)?
            }
        };
        JudgeRef::Default { model_ref }
    }
};
```
(`resolve_model` is the Task-8 closure; if `lower_checks` is a separate fn, pass `config` + a resolver or inline the same lookup. Match the real field name for the deliverables map.)

- [ ] **Step 4: Run (pass)**

Run: same as Step 2. Expected: PASS. Then full `-p tau-ir`:
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/lower/parse.rs
git commit -m "feat(ir): lower judge to Default with producer-model default (Q4)"
```

---

## Phase D — tau-runtime-core runtime wiring

### Task 10: Dispatcher trait `llm_backend()` → `llm_backend_for(name)` (D4)

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs:46`
- Modify: every in-crate `impl ToolDispatcher` (tests) — find with `grep -rn "fn llm_backend" crates/tau-runtime-core`

- [ ] **Step 1: Change the trait method**

```rust
/// Resolve the backend a given agent/judge needs, by backend package name.
fn llm_backend_for(
    &self,
    backend: &str,
) -> Result<alloc::sync::Arc<dyn DynLlmBackend>, RuntimeError>;
```
(Remove the old `fn llm_backend(&self) -> Arc<dyn DynLlmBackend>;`.)

- [ ] **Step 2: Build to enumerate broken impls**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-runtime-core`
Expected: errors at each `impl ToolDispatcher`.

- [ ] **Step 3: Update each in-crate impl**

For single-backend test dispatchers, return the one backend and assert the name:

```rust
fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
    debug_assert!(backend == self.backend.name() || backend.is_empty());
    Ok(self.backend.clone())
}
```
(Adapt field names per dispatcher. The `check.rs` `TestDispatcher` at line ~348 is one of these.)

- [ ] **Step 4: Commit (compiles; behavior wired in Task 11)**

```bash
git add crates/tau-runtime-core/src/interpreter/tool_dispatch.rs crates/tau-runtime-core/src/interpreter/check.rs
git commit -m "feat(runtime-core)!: ToolDispatcher::llm_backend_for(name)"
```

### Task 11: Rewire `prepare_agent_run` + `stream.rs` to use the resolved model (D4)

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/agent_loop.rs:413-476`
- Modify: `crates/tau-runtime-core/src/stream.rs:292`
- Test: an agent_loop / interpreter test asserting the request model id

- [ ] **Step 1: Write the failing test**

Add a test with a mock dispatcher whose backend records the last `CompletionRequest.model`, run an `ir::Agent` with `model_ref { backend, model_id: "claude-haiku-4-5" }`, assert the recorded model id is `"claude-haiku-4-5"` (not the backend name). Model it on the existing `check.rs` `TestDispatcher` pattern.

- [ ] **Step 2: Run (fails — today it's the backend name)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core <test_name>`
Expected: FAIL (asserts model id, gets backend name).

- [ ] **Step 3: Resolve by name + bake model id**

In `prepare_agent_run`, replace lines 413-415:

```rust
    // 1. Resolve the LLM backend this agent's model_ref names.
    let backend = dispatcher.llm_backend_for(&agent.model_ref.backend)?;
    let backend_name = String::from(backend.name());
```
At the `AgentDefinition` construction (line 467-476), append `.with_model(agent.model_ref.model_id.clone())` after `.with_system_prompt(...)`. Keep `llm_backend_pkg_name` derived from `agent.model_ref.backend` (it should equal `backend_name`).

In `stream.rs:292` replace:

```rust
            let mut request = CompletionRequest::new(agent_def.model.clone().into());
```

- [ ] **Step 4: Run (passes)**

Run: same as Step 2, then full `-p tau-runtime-core`:
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/agent_loop.rs crates/tau-runtime-core/src/stream.rs
git commit -m "feat(runtime-core): resolve per-agent backend + send real model id"
```

### Task 12: Judge synthesis uses `JudgeRef::Default { model_ref }` (D5)

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/check.rs:218-239`
- Test: the existing `deliverable_builtin_judge_*` tests (line ~376) — update to `Default { model_ref }`

- [ ] **Step 1: Update the synthesis arm**

```rust
        JudgeRef::Default { model_ref } => Agent {
            id: AgentId(String::from("__judge")),
            prompt: builtin_judge_prompt(must_satisfy),
            model_ref: model_ref.clone(),
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            produces: alloc::vec::Vec::new(),
            budget: AgentBudget { max_turns: Some(1), max_tokens: None },
        },
```
Update the existing tests' `JudgeRef::Builtin { model: None }` → `JudgeRef::Default { model_ref: ModelRef { backend: "<test-backend-name>".into(), model_id: "m".into() } }` so the test dispatcher's `llm_backend_for` resolves.

- [ ] **Step 2: Run the judge tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core deliverable`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/check.rs
git commit -m "feat(runtime-core): judge synth uses JudgeRef::Default model_ref"
```

---

## Phase E — tau-cli host dispatcher + tau check

### Task 13: Multi-backend `ForwardingDispatcher`

**Files:**
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs:123-128, 190` and the `ForwardingDispatcher` struct + its `ToolDispatcher` impl
- Test: `crates/tau-cli/src/cmd/ir_dispatcher.rs` tests

- [ ] **Step 1: Hold the whole registry, select by name**

Change `ForwardingDispatcher` to store `llm_backends: BTreeMap<String, Arc<dyn DynLlmBackend>>` (keyed by `backend.name()`) instead of a single backend. Replace the `.values().next()` extraction (lines 123-128) with cloning the whole name-keyed map from `runtime.llm_backends()`:

```rust
    let llm_backends: BTreeMap<String, Arc<dyn DynLlmBackend>> = runtime
        .llm_backends()
        .iter()                       // Registry iter → (name, handle)
        .map(|(name, h)| (name.to_string(), h.clone()))
        .collect();
    if llm_backends.is_empty() {
        return Err(anyhow::anyhow!("runtime has no LLM backend after plugin load"));
    }
```
(Match the real `Registry` iteration API — see `builder.rs` `Registry`.)

Update `ForwardingDispatcher::new` (line 190) to take the map. Implement:

```rust
fn llm_backend_for(&self, backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
    self.llm_backends.get(backend).cloned().ok_or_else(|| RuntimeError::Internal {
        message: format!("no loaded LLM backend named `{backend}`"),
    })
}
```

> `setup_mcp_runtime` (line 152) still takes a single backend — keep passing a
> representative one (e.g. the first map value). MCP backend selection is out of
> scope for this track.

- [ ] **Step 2: Build + test tau-cli**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli ir_dispatcher`
Expected: PASS (update any in-crate test dispatcher impls to the new trait method).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/src/cmd/ir_dispatcher.rs
git commit -m "feat(cli): multi-backend ForwardingDispatcher (llm_backend_for)"
```

### Task 14: `tau check` — `BackendNotLlmCapable` probe finding (D7 stage 2)

**Files:**
- Modify: the `tau check` plugin/sandbox probe module (search `crates/tau-cli/src/cmd` for the check verbs / findings enum)
- Test: tau-cli check tests

- [ ] **Step 1: Write the failing test**

Add a check test: a project whose `[models]` backend resolves to a loaded package that does NOT expose LLM completion yields a `BackendNotLlmCapable` finding. (Model on existing `tau check` plugin-probe tests; if no probe harness exists for LLM capability, scope this to: for each distinct `[models].backend`, assert the probed plugin advertises an LLM-completion capability, else emit the finding.)

- [ ] **Step 2: Run (fails)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli check`
Expected: FAIL.

- [ ] **Step 3: Add the finding + probe**

Add a `BackendNotLlmCapable { backend: String }` finding variant to the check findings type and, in the plugin-probe pass, verify each `[models].backend` plugin exposes LLM completion. Register the finding code in `docs/explanation/escape-hatches.md` only if it is an escape-hatch class (it is a diagnostic finding, not an escape hatch — confirm against the doc's conventions; do NOT add a spurious entry).

- [ ] **Step 4: Run (passes)**

Run: same as Step 2. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/
git commit -m "feat(cli): tau check BackendNotLlmCapable probe (D7 stage 2)"
```

---

## Phase F — TypeScript authoring parity

### Task 15: `tau-ts-extract` — extract `[models]` + agent `model` alias

**Files:**
- Modify: `crates/tau-ts-extract/src/*` (the AST extraction + the `build_project_config` TOML bridge)
- Test: `crates/tau-ts-extract` tests + a TOML↔TS byte-equal conformance check

- [ ] **Step 1: Write the failing parity test**

Author a `.ts` fixture declaring a models map and an agent with `model: "haiku"`, plus the byte-equal `.toml`. Assert the extracted `ProjectConfig` (or emitted TOML) round-trips byte-equal to the TOML authoring, mirroring the existing TOML↔TS conformance test (ADR-0041 pattern).

- [ ] **Step 2: Run (fails)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract models`
Expected: FAIL.

- [ ] **Step 3: Implement extraction**

Extract the `models` declaration from the TS AST (mirror how `tools`/`steps`/`agents` are extracted) and emit the `[models]` TOML table; ensure the agent's `model` field passes through. Reuse the `UncheckedProjectConfig`-via-TOML serialization bridge (per the β.8 memo: `UncheckedProjectConfig` is `#[non_exhaustive]`, construct via TOML serialization, not struct literal).

- [ ] **Step 4: Run (passes)**

Run: same as Step 2. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ts-extract/
git commit -m "feat(ts-extract): [models] table + agent model alias parity"
```

---

## Phase G — conformance, migration, docs

### Task 16: Migrate existing fixtures off `llm_backend` + onto `[models]`

**Files:**
- Modify: every `crates/tau-ir-conformance/fixtures/*/workflow.toml` and any in-repo `tau.toml` test fixture that uses `llm_backend` or a raw `model`

- [ ] **Step 1: Find every fixture**

Run: `grep -rln "llm_backend" crates/ docs/ | grep -E "\.toml$"`
Then for each: add a `[models]` table, replace `llm_backend = "X"` + `model = "Y"` with a `[models]` alias and `model = "<alias>"`.

- [ ] **Step 2: Regenerate conformance snapshots**

Run the conformance suite; update `expected_report.json` snapshots for the new IR shape (`model_ref`, `JudgeRef::Default`, format `v2.0.0`):
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
(Use the suite's snapshot-update mechanism — search the conformance crate README/Makefile for how snapshots regenerate; do NOT hand-edit if a regen path exists.)

- [ ] **Step 3: Commit**

```bash
git add crates/tau-ir-conformance/ crates/  # all migrated fixtures
git commit -m "test: migrate fixtures to [models] + v2.0.0 IR"
```

### Task 17: New conformance fixture — multi-model workflow + deliverable judge

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/12_models_multi/` (workflow.toml + .ts + expected_report.json), numbering per the existing sequence

- [ ] **Step 1: Author the fixture**

A workflow with a `[models]` table (≥2 aliases on the same backend), two agents on different models, and one deliverable with an explicit `judge_model` plus one whose judge omits `judge_model` (inherits producer). Include both `.toml` and byte-equal `.ts`.

- [ ] **Step 2: Run + snapshot**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance models_multi`
Expected: PASS (TOML↔IR byte-equal + TOML↔TS parity).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-ir-conformance/fixtures/12_models_multi/
git commit -m "test(conformance): multi-model + judge fixture"
```

### Task 18: Version drift test + full-workspace green

**Files:**
- Modify: the IR-format-version drift test in `tau-runtime-tokio` (search `v1.2.0` / `IrFormatVersion` there)

- [ ] **Step 1: Update the drift test to `v2.0.0`**

Run: `grep -rn "v1.2.0\|IrFormatVersion" crates/tau-runtime-tokio` and update the asserted version constant.

- [ ] **Step 2: Per-crate test sweep (touched crates)**

Run each (separate invocations, single `-p`):
`tau-pkg`, `tau-domain`, `tau-ir`, `tau-runtime-core`, `tau-runtime-tokio`, `tau-cli`, `tau-ts-extract`, `tau-ir-conformance` — e.g.
`timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio`
Expected: all PASS.

- [ ] **Step 3: Doctests for crates with changed doc examples**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg` (and tau-ir, tau-domain if their doctests changed).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-runtime-tokio/
git commit -m "test: bump IR format-version drift assertion to v2.0.0"
```

### Task 19: ADR recording D1–D7

**Files:**
- Create: `docs/decisions/00NN-per-agent-model-resolution.md` (next free number — check `ls docs/decisions/`)

- [ ] **Step 1: Write the ADR**

Record: D1 `[models]` table chosen over self-advertising / passthrough; D2/D7 resolve-at-lowering bakes `ModelRef`; D3 Schema 1 hard cutover (`llm_backend` removed); D4 `llm_backend_for` + `AgentDefinition.model`; D5 `JudgeRef::Default` + producer-model default; D6 `MissingAgentModel`; the **MAJOR v2.0.0** IR bump and why (breaking field/variant change per `module.rs` semver rules); honest limit (vendor model string trusted, not validated offline). Note this **closes the `judge_model` runtime no-op** honest limit from ADR-0044.

- [ ] **Step 2: Commit**

```bash
git add docs/decisions/
git commit -m "docs(adr): per-agent model resolution (D1-D7)"
```

---

## Self-Review (completed by plan author)

- **Spec coverage:** D1/D2/D7 → Tasks 1,2,4,7,8,9; D3 → Task 3; D4 → Tasks 6,10,11,13; D5 → Tasks 5,9,12; D6 → Task 4; validation 3-stage → Tasks 4 (stage 1) + 14 (stage 2) + runtime (stage 3, inherent); ts parity → Task 15; conformance → Tasks 16,17; version → Tasks 7,18; ADR → Task 19. No gaps.
- **Type consistency:** `ModelRef { backend, model_id }` used identically in node.rs, check.rs, lowering, runtime synth, tests. `ModelEntry { backend, model }` (tau-pkg) maps to `ModelRef { backend, model_id }` (tau-ir) at lowering — names intentionally differ across the crate boundary; the mapping is explicit in Task 8. `JudgeConfig::Default` (pkg) ↔ `JudgeRef::Default` (ir). `with_model` / `AgentDefinition.model` consistent across Tasks 6,11.
- **Placeholders:** the few "search the crate for the real helper" notes are unavoidable cross-references to existing untouched APIs (package-set lookup, snapshot-regen path, TS extraction siblings); each names the concrete symbol/file to find. No TBD logic.
