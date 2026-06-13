# IR Sequential Pipeline Executor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the canonical IR a declarative, engine-sequenced multi-step pipeline (`[[pipeline.steps]]`) with a run-scoped `steps.<id>.output` store, input templating, build-time checks, and step trace events — additive, so single-entry `tau run` is byte-for-byte unchanged when no pipeline is declared.

**Architecture:** A new `Pipeline` value rides on `IrModule.workflow` (`Option<Pipeline>`, `None` today). A new `run_pipeline` in `tau-runtime-core` sequences the *existing* agent/tool/deterministic executors, threading each step's output through a run-scoped `OutputStore`. Templating and static reference-extraction live in `tau-ir` (`no_std`) so both lowering (build-time checks) and runtime share one implementation.

**Tech Stack:** Rust workspace, `tau-ir` (`no_std`+`alloc`), `tau-runtime-core`, `tau-pkg` (project config), `tau-cli`, `tau-ts-extract`, `tau-ir-conformance`. Tests via `cargo nextest` + doctests.

**Spec:** `docs/superpowers/specs/2026-06-13-ir-pipeline-executor-design.md`

---

## Conventions for every task (read once)

- **Cargo (per repo CLAUDE.md):** never run bare `cargo`. Always:
  `timeout <secs> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>`
  (test=300s, build/check=180s, clippy=240s). Doctests: `... cargo test -p <crate> --doc`. Pick a fresh `target/agent-impl-N` if another build holds the lock.
- **Commits (per repo CLAUDE.md):** `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "..."`. Conventional commits, imperative. Commit after each task's tests pass.
- **`tau-ir` is `#![no_std]` + `#![deny(missing_docs)]`:** new code uses `alloc::` (not `std::`); every public item needs a doc comment.
- **`#[non_exhaustive]` types** (`ProjectConfig`, `RunOutcome`, `Message`) cannot be struct-literal-constructed outside their crate — construct via the crate's own constructor/validator, and match with a `_ => {}` arm.

---

## File map

| File | Change | Task |
|------|--------|------|
| `crates/tau-ir/src/ids.rs` | add `PipelineStepId` | 1 |
| `crates/tau-ir/src/pipeline.rs` | **new** — `Pipeline`, `PipelineStep`, `StepRun` | 1 |
| `crates/tau-ir/src/module.rs` | `Workflow.pipeline: Option<Pipeline>` | 1 |
| `crates/tau-ir/src/lib.rs` | exports + module decl | 1 |
| `crates/tau-ir/src/module.rs` | `IrFormatVersion::CURRENT` → `v1.1.0` | 2 |
| `crates/tau-ir/src/template.rs` | **new** — `extract_refs`, `resolve`, `TemplateError` | 3 |
| `crates/tau-pkg/src/project/project.rs` | `Unchecked`/`Checked` pipeline config + validation | 4 |
| `crates/tau-ir/src/lower/parse.rs` | populate `workflow.pipeline` | 5 |
| `crates/tau-ir/src/error.rs` | new pipeline `IrError` variants | 6 |
| `crates/tau-ir/src/lower/typecheck.rs` | pipeline build-time checks | 6 |
| `crates/tau-runtime-core/src/interpreter/output_store.rs` | **new** — `OutputStore` | 7 |
| `crates/tau-runtime-core/src/interpreter/pipeline.rs` | **new** — `run_pipeline` (agent steps) | 8 |
| `crates/tau-runtime-core/src/interpreter/pipeline.rs` | tool + deterministic steps | 9 |
| `crates/tau-runtime-core/src/vocabulary.rs` | step trace constants | 10 |
| `crates/tau-runtime-core/src/interpreter/pipeline.rs` | emit trace events | 10 |
| `crates/tau-cli/src/cmd/run.rs` | branch to `run_pipeline` | 11 |
| `crates/tau-ts-extract/src/*` | `pipeline([...])` factory + TOML emission | 12 |
| `crates/tau-ir-conformance/fixtures/08_pipeline_sequence/` | **new** fixture | 13 |
| `crates/tau-ir-conformance/tests/conformance.rs` | fixture-08 tests | 13 |

---

## Task 1: IR pipeline data types

**Files:**
- Modify: `crates/tau-ir/src/ids.rs`
- Create: `crates/tau-ir/src/pipeline.rs`
- Modify: `crates/tau-ir/src/module.rs` (add `Workflow.pipeline`)
- Modify: `crates/tau-ir/src/lib.rs` (module decl + re-exports)

- [ ] **Step 1: Add `PipelineStepId` to `ids.rs`**

Append to `crates/tau-ir/src/ids.rs`:

```rust
/// Identifier for a [`crate::pipeline::PipelineStep`] within a
/// [`crate::Workflow`]'s pipeline. Addressable as `steps.<id>.output`.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct PipelineStepId(pub String);
```

- [ ] **Step 2: Create `pipeline.rs`**

Create `crates/tau-ir/src/pipeline.rs`:

```rust
//! Sequential pipeline: an ordered list of steps the engine executes
//! top-to-bottom, threading each step's output to later steps via
//! `${steps.<id>.output}` templating.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, PipelineStepId, StepId, ToolId};

/// An ordered, engine-sequenced pipeline of steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    /// Steps, executed top-to-bottom in this order.
    pub steps: Vec<PipelineStep>,
}

/// One step in a [`Pipeline`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStep {
    /// Handle for this step; its output is addressable as
    /// `steps.<id>.output` by later steps.
    pub id: PipelineStepId,
    /// What this step runs.
    pub run: StepRun,
    /// Input template (`${input}`, `${steps.<id>.output}`).
    pub input: String,
}

/// What a [`PipelineStep`] executes — a reference to an existing node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepRun {
    /// Run an agent node by id.
    Agent(AgentId),
    /// Invoke a tool node by id.
    Tool(ToolId),
    /// Run a deterministic step node by id.
    Deterministic(StepId),
}
```

- [ ] **Step 3: Add `pipeline` field to `Workflow`**

In `crates/tau-ir/src/module.rs`, add `use crate::pipeline::Pipeline;` to the imports, then add this field as the **last** field of `struct Workflow` (after `capability_table`):

```rust
    /// Optional engine-sequenced pipeline. `None` preserves single-entry
    /// behavior (run the named entry agent). `Some` => `run_pipeline`.
    pub pipeline: Option<Pipeline>,
```

`Workflow` derives `Default`; `Option<Pipeline>` defaults to `None`, so the derive still holds. **Find every struct-literal construction of `Workflow`** (`grep -rn "Workflow {" crates/tau-ir/src`) and add `pipeline: None,` to each (the parse stage in Task 5 will populate it for real).

- [ ] **Step 4: Wire `lib.rs`**

In `crates/tau-ir/src/lib.rs`: add `pub mod pipeline;` next to `pub mod node;`, add `pub use ids::PipelineStepId;` (extend the existing `pub use ids::{...}` line), and add `pub use pipeline::{Pipeline, PipelineStep, StepRun};`.

- [ ] **Step 5: Write the failing test**

Append to `crates/tau-ir/src/pipeline.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentId;

    #[test]
    fn pipeline_serde_round_trips() {
        let p = Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("writer".into()),
                run: StepRun::Agent(AgentId("writer".into())),
                input: "${steps.gather.output}".into(),
            }],
        };
        let bytes = serde_json::to_vec(&p).expect("serializes");
        let back: Pipeline = serde_json::from_slice(&bytes).expect("deserializes");
        assert_eq!(p, back);
    }
}
```

- [ ] **Step 6: Run tests (expect fail, then build green)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir`
Expected first run: FAIL if any `Workflow {` literal is missing `pipeline: None`. Fix until it builds, then:
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir pipeline`
Expected: PASS (`pipeline_serde_round_trips`).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-ir/src/ids.rs crates/tau-ir/src/pipeline.rs crates/tau-ir/src/module.rs crates/tau-ir/src/lib.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): add Pipeline/PipelineStep/StepRun types + Workflow.pipeline"
```

---

## Task 2: Bump `ir_format` (additive MINOR)

**Files:**
- Modify: `crates/tau-ir/src/module.rs`
- Test: `crates/tau-ir/src/canonical.rs` (add a round-trip test with a pipeline)

- [ ] **Step 1: Write the failing test**

Append a test module to `crates/tau-ir/src/canonical.rs` (or extend the existing one). It builds a minimal `IrModule` with a pipeline and asserts canonical round-trip + that the version is the bumped value:

```rust
#[cfg(test)]
mod pipeline_canonical_tests {
    use super::*;
    use crate::ids::{AgentId, PipelineStepId};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::pipeline::{Pipeline, PipelineStep, StepRun};
    use tau_ports::target::registry;

    #[test]
    fn module_with_pipeline_round_trips_and_reports_v1_1() {
        let target = registry::list_available().next().unwrap().triple;
        let mut wf = Workflow::default();
        wf.pipeline = Some(Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("a".into()),
                run: StepRun::Agent(AgentId("a".into())),
                input: "${input}".into(),
            }],
        });
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: wf,
        };
        assert_eq!(m.ir_format.0, "v1.1.0");
        let bytes = to_canonical_bytes(&m);
        let back = from_canonical_bytes(&bytes).expect("round-trips");
        assert_eq!(m, back);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir module_with_pipeline`
Expected: FAIL on `assert_eq!(m.ir_format.0, "v1.1.0")` (current is `v1.0.0`).

- [ ] **Step 3: Bump the version**

In `crates/tau-ir/src/module.rs`, change:

```rust
    pub const CURRENT: &'static str = "v1.0.0";
```
to
```rust
    pub const CURRENT: &'static str = "v1.1.0";
```

- [ ] **Step 4: Fix any tests that assert the old version**

Run: `grep -rn "v1.0.0" crates/ --include=*.rs` and update any assertion expecting `"v1.0.0"` as the *current* IR format to `"v1.1.0"`. (Do not touch historical/back-compat fixtures that intentionally pin an old version, if any — read the surrounding test to confirm intent.)

- [ ] **Step 5: Run tests to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS (all tau-ir tests, including the new one).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir/src/module.rs crates/tau-ir/src/canonical.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): bump ir_format to v1.1.0 for additive pipeline field"
```

---

## Task 3: Templating + static reference extraction (`tau-ir`)

**Files:**
- Create: `crates/tau-ir/src/template.rs`
- Modify: `crates/tau-ir/src/lib.rs` (module decl + re-exports)

This ports `crates/tau-workflow/src/template.rs` (read it for the proven char-scan logic and escape rules) into `no_std` and adds `extract_refs` for build-time checks.

- [ ] **Step 1: Create `template.rs` with the failing tests first**

Create `crates/tau-ir/src/template.rs`:

```rust
//! Pipeline input templating: `${input}` and `${steps.<id>.output}`.
//!
//! Two surfaces share one parser:
//! - [`extract_refs`] — static reference extraction for build-time checks
//!   (no values needed).
//! - [`resolve`] — runtime substitution against an input + prior outputs.
//!
//! Escape: `$${` yields a literal `${`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use thiserror::Error;

/// A `${...}` reference found in a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateRef {
    /// `${input}` — the run's top-level input.
    Input,
    /// `${steps.<id>.output}` — an earlier step's output, by id.
    StepOutput(String),
}

/// Template parse/resolve error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    /// A `${` was never closed by `}`.
    #[error("unterminated reference: ${{{0}")]
    Unterminated(String),
    /// A reference did not match `input` or `steps.<id>.output`.
    #[error("unrecognized reference: {0}")]
    Unrecognized(String),
    /// A `${steps.<id>.output}` named an id with no available output.
    #[error("unresolved reference: {0}")]
    Unresolved(String),
}

/// Parse a `key` (the text between `${` and `}`) into a [`TemplateRef`].
fn parse_key(key: &str) -> Result<TemplateRef, TemplateError> {
    if key == "input" {
        return Ok(TemplateRef::Input);
    }
    if let Some(stripped) = key.strip_prefix("steps.") {
        if let Some(id) = stripped.strip_suffix(".output") {
            return Ok(TemplateRef::StepOutput(id.to_string()));
        }
    }
    Err(TemplateError::Unrecognized(key.to_string()))
}

/// Walk `template`, invoking `on_ref` for each recognized reference and
/// pushing literal text (with `$${`→`${` unescaping) to `out`.
fn walk(
    template: &str,
    out: &mut String,
    mut on_ref: impl FnMut(TemplateRef) -> Result<String, TemplateError>,
) -> Result<(), TemplateError> {
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            if chars.peek() == Some(&'$') {
                chars.next();
                if chars.peek() == Some(&'{') {
                    chars.next();
                    out.push_str("${");
                } else {
                    out.push_str("$$");
                }
                continue;
            }
            if chars.peek() == Some(&'{') {
                chars.next();
                let mut key = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    key.push(ch);
                }
                if !closed {
                    return Err(TemplateError::Unterminated(key));
                }
                let r = parse_key(&key)?;
                out.push_str(&on_ref(r)?);
                continue;
            }
        }
        out.push(c);
    }
    Ok(())
}

/// Extract every `${...}` reference from `template`, in order. Used by the
/// lowering pass for forward/unknown-reference checks (no values needed).
pub fn extract_refs(template: &str) -> Result<Vec<TemplateRef>, TemplateError> {
    let mut refs = Vec::new();
    let mut sink = String::new();
    walk(template, &mut sink, |r| {
        refs.push(r.clone());
        Ok(String::new())
    })?;
    Ok(refs)
}

/// Resolve `${...}` references in `template` against `input` + `prior`
/// (step id → stringified output).
pub fn resolve(
    template: &str,
    input: &str,
    prior: &BTreeMap<String, String>,
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    walk(template, &mut out, |r| match r {
        TemplateRef::Input => Ok(input.to_string()),
        TemplateRef::StepOutput(id) => prior
            .get(&id)
            .cloned()
            .ok_or(TemplateError::Unresolved(id)),
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_input_and_step_output() {
        let mut prior = BTreeMap::new();
        prior.insert("gather".to_string(), "notes".to_string());
        let out = resolve("in=${input} g=${steps.gather.output}", "X", &prior).unwrap();
        assert_eq!(out, "in=X g=notes");
    }

    #[test]
    fn escapes_double_dollar() {
        let out = resolve("$${input}", "X", &BTreeMap::new()).unwrap();
        assert_eq!(out, "${input}");
    }

    #[test]
    fn unterminated_errors() {
        assert!(matches!(
            resolve("${input", "X", &BTreeMap::new()),
            Err(TemplateError::Unterminated(_))
        ));
    }

    #[test]
    fn unresolved_step_errors() {
        assert!(matches!(
            resolve("${steps.nope.output}", "X", &BTreeMap::new()),
            Err(TemplateError::Unresolved(ref s)) if s == "nope"
        ));
    }

    #[test]
    fn extract_refs_lists_in_order() {
        let refs = extract_refs("${input} ${steps.a.output} ${steps.b.output}").unwrap();
        assert_eq!(
            refs,
            alloc::vec![
                TemplateRef::Input,
                TemplateRef::StepOutput("a".into()),
                TemplateRef::StepOutput("b".into()),
            ]
        );
    }
}
```

- [ ] **Step 2: Wire `lib.rs`**

Add `pub mod template;` and `pub use template::{extract_refs, resolve, TemplateError, TemplateRef};`.

- [ ] **Step 3: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir template`
Expected: PASS (5 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-ir/src/template.rs crates/tau-ir/src/lib.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): add pipeline template module (extract_refs + resolve)"
```

---

## Task 4: Project config — parse `[[pipeline.steps]]`

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs`
- Test: same file's test module (it already has TOML-parse tests — follow their shape)

Read `project.rs` around lines 14–27 (`UncheckedProjectConfig`), 295–308 (`ProjectConfig`), and 589–617 (`validate`) before editing.

- [ ] **Step 1: Add unchecked + checked config types**

Add near the other `Unchecked*` / `*Entry` types in `project.rs`:

```rust
/// Raw `[pipeline]` table (pre-validation).
#[derive(Debug, Clone, Deserialize)]
pub struct UncheckedPipeline {
    /// Ordered steps from `[[pipeline.steps]]`.
    #[serde(default)]
    pub steps: Vec<UncheckedPipelineStep>,
}

/// Raw `[[pipeline.steps]]` entry (pre-validation).
#[derive(Debug, Clone, Deserialize)]
pub struct UncheckedPipelineStep {
    /// Step handle.
    pub id: String,
    /// `"agent:<id>"` | `"tool:<id>"` | `"deterministic:<id>"`.
    pub run: String,
    /// Input template; defaults to `"${input}"` when omitted.
    pub input: Option<String>,
}

/// Validated pipeline.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PipelineConfig {
    /// Ordered, validated steps.
    pub steps: Vec<PipelineStepConfig>,
}

/// Validated pipeline step.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PipelineStepConfig {
    /// Step handle.
    pub id: String,
    /// Resolved run target.
    pub run: PipelineRunRef,
    /// Input template (defaulted to `"${input}"`).
    pub input: String,
}

/// A validated `run = "<kind>:<id>"` reference.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineRunRef {
    /// `agent:<id>`
    Agent(String),
    /// `tool:<id>`
    Tool(String),
    /// `deterministic:<id>`
    Deterministic(String),
}
```

- [ ] **Step 2: Add fields to the two config structs**

- In `UncheckedProjectConfig` add: `#[serde(default)] pub pipeline: Option<UncheckedPipeline>,`
- In `ProjectConfig` add: `pub pipeline: Option<PipelineConfig>,`

- [ ] **Step 3: Add the validator + call it**

Add a function near `validate_step`:

```rust
fn validate_pipeline(raw: &UncheckedPipeline) -> Result<PipelineConfig, ProjectError> {
    let mut seen = std::collections::BTreeSet::new();
    let mut steps = Vec::with_capacity(raw.steps.len());
    for s in &raw.steps {
        if !seen.insert(s.id.clone()) {
            return Err(ProjectError::invalid(format!(
                "pipeline step id {:?} is declared more than once",
                s.id
            )));
        }
        let run = match s.run.split_once(':') {
            Some(("agent", id)) => PipelineRunRef::Agent(id.to_string()),
            Some(("tool", id)) => PipelineRunRef::Tool(id.to_string()),
            Some(("deterministic", id)) => PipelineRunRef::Deterministic(id.to_string()),
            _ => {
                return Err(ProjectError::invalid(format!(
                    "pipeline step {:?}: run must be \"agent:<id>\", \"tool:<id>\", or \
                     \"deterministic:<id>\", got {:?}",
                    s.id, s.run
                )))
            }
        };
        steps.push(PipelineStepConfig {
            id: s.id.clone(),
            run,
            input: s.input.clone().unwrap_or_else(|| "${input}".to_string()),
        });
    }
    Ok(PipelineConfig { steps })
}
```

> Match the **exact** `ProjectError` constructor used elsewhere in `validate_*` (e.g. `ProjectError::invalid(...)` may instead be a named variant — read a sibling validator and copy its error-construction shape). In `validate()` (≈ line 589), after the existing agent/tool/step validation, add:
> ```rust
> let pipeline = match &self.pipeline {
>     Some(p) => Some(validate_pipeline(p)?),
>     None => None,
> };
> ```
> and set `pipeline` in the `ProjectConfig { ... }` it constructs.

- [ ] **Step 4: Write the failing test**

Add to the `project.rs` test module (mirror an existing `parse_str` test):

```rust
#[test]
fn parses_pipeline_steps() {
    let toml = r#"
        [project]
        name = "demo"

        [[pipeline.steps]]
        id = "gather"
        run = "agent:gather"
        input = "${input}"

        [[pipeline.steps]]
        id = "writer"
        run = "agent:writer"
        input = "${steps.gather.output}"
    "#;
    let cfg = ProjectConfig::parse_str(toml).expect("parses");
    let pipe = cfg.pipeline.expect("pipeline present");
    assert_eq!(pipe.steps.len(), 2);
    assert_eq!(pipe.steps[0].id, "gather");
    assert_eq!(pipe.steps[0].run, PipelineRunRef::Agent("gather".into()));
    assert_eq!(pipe.steps[1].input, "${steps.gather.output}");
}

#[test]
fn rejects_unknown_run_kind() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "x"
        run = "wizard:x"
    "#;
    assert!(ProjectConfig::parse_str(toml).is_err());
}

#[test]
fn defaults_pipeline_input_to_top_level() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "x"
        run = "agent:x"
    "#;
    let cfg = ProjectConfig::parse_str(toml).unwrap();
    assert_eq!(cfg.pipeline.unwrap().steps[0].input, "${input}");
}
```

- [ ] **Step 5: Run tests (fail → green)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg pipeline`
Expected: PASS (3 tests). Fix any other `ProjectConfig { ... }` literal in the crate that now needs `pipeline: None`.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(pkg): parse + validate [[pipeline.steps]] in project config"
```

---

## Task 5: Lowering — populate `workflow.pipeline`

**Files:**
- Modify: `crates/tau-ir/src/lower/parse.rs`
- Test: `crates/tau-ir/src/lower/parse.rs` test module (or a `lower` integration test)

- [ ] **Step 1: Map config pipeline → IR pipeline in `parse.rs`**

Read `parse.rs` to find where the `Workflow` is assembled. After agents/tools/steps are built (and before/where `Workflow { ... }` is constructed), translate `config.pipeline`.

Imports to add at the top of `parse.rs`: `crate::pipeline::{Pipeline, PipelineStep, StepRun}`, `crate::ids::{AgentId, PipelineStepId, StepId, ToolId}`, and `tau_pkg::project::PipelineRunRef`. Then add this translation:

```rust
let pipeline = config.pipeline.as_ref().map(|p| Pipeline {
    steps: p
        .steps
        .iter()
        .map(|s| PipelineStep {
            id: PipelineStepId(s.id.clone()),
            run: match &s.run {
                PipelineRunRef::Agent(id) => StepRun::Agent(AgentId(id.clone())),
                PipelineRunRef::Tool(id) => StepRun::Tool(ToolId(id.clone())),
                PipelineRunRef::Deterministic(id) => StepRun::Deterministic(StepId(id.clone())),
            },
            input: s.input.clone(),
        })
        .collect(),
});
```

Set `pipeline` into the `Workflow` being built (replace the `pipeline: None` placeholder from Task 1 here with `pipeline`).

- [ ] **Step 2: Write the failing test**

Add to `parse.rs` tests (or extend `lower/mod.rs` doctest-style integration). Use the `Caches` stub pattern from `lower/mod.rs`:

```rust
#[test]
fn lowers_pipeline_steps_in_order() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "a"
        run = "agent:a"
        input = "${input}"
        [[pipeline.steps]]
        id = "b"
        run = "agent:b"
        input = "${steps.a.output}"
    "#;
    let config = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
    let parsed = parse(&config).expect("parses");
    let pipe = parsed.workflow.pipeline.expect("pipeline present");
    assert_eq!(pipe.steps.len(), 2);
    assert_eq!(pipe.steps[0].id.0, "a");
    assert_eq!(pipe.steps[1].input, "${steps.a.output}");
}
```

> Note: `parse` does not require agents `a`/`b` to exist (that's typecheck's job in Task 6), so this test passes with only the pipeline declared.

- [ ] **Step 3: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir lowers_pipeline`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-ir/src/lower/parse.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): lower [[pipeline.steps]] into Workflow.pipeline"
```

---

## Task 6: Build-time pipeline checks (typecheck)

**Files:**
- Modify: `crates/tau-ir/src/error.rs` (new variants)
- Modify: `crates/tau-ir/src/lower/typecheck.rs`
- Test: `crates/tau-ir/src/lower/typecheck.rs` test module

- [ ] **Step 1: Add `IrError` variants**

Add to the `IrError` enum in `error.rs` (each needs a doc comment + `#[error(...)]`):

```rust
    /// A pipeline step's `run` target does not exist in the workflow.
    #[error("pipeline step {step:?}: run target {target} not found")]
    UnknownPipelineRun {
        /// The pipeline step id.
        step: String,
        /// The unresolved `kind:id` target, e.g. `agent:writer`.
        target: String,
    },

    /// Two pipeline steps share an id.
    #[error("pipeline step id {id:?} is declared more than once")]
    DuplicatePipelineStepId {
        /// The duplicated id.
        id: String,
    },

    /// `${steps.x.output}` references a step that runs at or after this one.
    #[error("pipeline step {step:?} references output of {referenced:?}, which is not an earlier step")]
    ForwardOutputRef {
        /// The referencing step.
        step: String,
        /// The referenced (later/self) step id.
        referenced: String,
    },

    /// `${steps.x.output}` references a step id not in the pipeline.
    #[error("pipeline step {step:?} references unknown step output {referenced:?}")]
    UnknownOutputRef {
        /// The referencing step.
        step: String,
        /// The unknown referenced id.
        referenced: String,
    },

    /// A pipeline input template was malformed (unterminated/unrecognized).
    #[error("pipeline step {step:?}: bad input template: {detail}")]
    BadPipelineTemplate {
        /// The step id.
        step: String,
        /// Human-readable template error.
        detail: String,
    },
```

- [ ] **Step 2: Add the check function in `typecheck.rs`**

Add (and call it from the crate's `typecheck` entry, after the existing checks):

```rust
fn check_pipeline(wf: &crate::module::Workflow) -> Result<(), IrError> {
    use crate::pipeline::StepRun;
    use crate::template::{extract_refs, TemplateRef};
    use alloc::collections::BTreeSet;

    let Some(pipeline) = &wf.pipeline else {
        return Ok(());
    };

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    for step in &pipeline.steps {
        let sid = step.id.0.as_str();
        if !seen_ids.insert(sid) {
            return Err(IrError::DuplicatePipelineStepId { id: sid.into() });
        }

        // run target must exist
        let exists = match &step.run {
            StepRun::Agent(a) => wf.agents.contains_key(a),
            StepRun::Tool(t) => wf.tools.contains_key(t),
            StepRun::Deterministic(s) => wf.steps.contains_key(s),
        };
        if !exists {
            let target = match &step.run {
                StepRun::Agent(a) => alloc::format!("agent:{}", a.0),
                StepRun::Tool(t) => alloc::format!("tool:{}", t.0),
                StepRun::Deterministic(s) => alloc::format!("deterministic:{}", s.0),
            };
            return Err(IrError::UnknownPipelineRun { step: sid.into(), target });
        }

        // template references must be earlier steps (no forward/self/unknown)
        let refs = extract_refs(&step.input)
            .map_err(|e| IrError::BadPipelineTemplate { step: sid.into(), detail: alloc::format!("{e}") })?;
        for r in refs {
            if let TemplateRef::StepOutput(ref_id) = r {
                // `seen_ids` already contains every step strictly before this one,
                // and NOT the current step (we inserted it above, so exclude self).
                if ref_id == sid {
                    return Err(IrError::ForwardOutputRef { step: sid.into(), referenced: ref_id });
                }
                let is_earlier = seen_ids.contains(ref_id.as_str()) && ref_id != sid;
                let exists_anywhere = pipeline.steps.iter().any(|s| s.id.0 == ref_id);
                if !exists_anywhere {
                    return Err(IrError::UnknownOutputRef { step: sid.into(), referenced: ref_id });
                }
                if !is_earlier {
                    return Err(IrError::ForwardOutputRef { step: sid.into(), referenced: ref_id });
                }
            }
        }
    }
    Ok(())
}
```

> Wire `check_pipeline(&resolved.workflow)?;` (or whatever the typecheck entry's workflow handle is named — read the top of `typecheck.rs`) into the public `typecheck` function alongside the existing checks.

- [ ] **Step 3: Write the failing tests**

```rust
#[test]
fn rejects_unknown_run_target() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "a"
        run = "agent:ghost"
    "#;
    let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
    let parsed = crate::lower::parse::parse(&cfg).unwrap();
    let err = typecheck(&parsed).unwrap_err();
    assert!(matches!(err, IrError::UnknownPipelineRun { .. }), "got {err:?}");
}

#[test]
fn rejects_forward_output_reference() {
    // step "a" references "b" which comes later
    let toml = r#"
        [project]
        name = "demo"
        [agents.a]
        package = "p@^0.1"
        llm_backend = "mock-llm"
        model = "m"
        prompt.system = "x"
        [agents.b]
        package = "p@^0.1"
        llm_backend = "mock-llm"
        model = "m"
        prompt.system = "x"
        [[pipeline.steps]]
        id = "a"
        run = "agent:a"
        input = "${steps.b.output}"
        [[pipeline.steps]]
        id = "b"
        run = "agent:b"
    "#;
    let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
    let parsed = crate::lower::parse::parse(&cfg).unwrap();
    let err = typecheck(&parsed).unwrap_err();
    assert!(matches!(err, IrError::ForwardOutputRef { .. }), "got {err:?}");
}
```

> Verify the minimal valid `[agents.<id>]` TOML against `project.rs` (required fields may differ — copy from an existing passing tau-pkg/tau-ir test fixture). The point is two agents `a`,`b` exist so the *only* failure is the forward ref.

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir typecheck`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir/src/error.rs crates/tau-ir/src/lower/typecheck.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir): build-time pipeline checks (run target, dup id, forward/unknown ref)"
```

---

## Task 7: `OutputStore` (runtime)

**Files:**
- Create: `crates/tau-runtime-core/src/interpreter/output_store.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs` (add `pub mod output_store;`)

- [ ] **Step 1: Create `output_store.rs`**

```rust
//! Run-scoped store of pipeline step outputs, keyed by pipeline-step id.
//!
//! Makes `${steps.<id>.output}` addressable — the substrate the
//! single-agent interpreter lacks. Stores each step's output as a JSON
//! `Value`; `template_map` projects it to the `String` map the templater
//! consumes (string values pass through; other values are compact-JSON
//! encoded).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

use serde_json::Value;

/// Pipeline step outputs accumulated during a run.
#[derive(Debug, Default, Clone)]
pub struct OutputStore {
    map: BTreeMap<String, Value>,
}

impl OutputStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `id`'s output.
    pub fn insert(&mut self, id: impl Into<String>, value: Value) {
        self.map.insert(id.into(), value);
    }

    /// Look up a step's output value.
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.map.get(id)
    }

    /// Project to the `id → String` map the templater consumes. A
    /// `Value::String` yields its inner text; any other value is
    /// compact-JSON encoded.
    pub fn template_map(&self) -> BTreeMap<String, String> {
        self.map
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_map_unwraps_strings_and_encodes_others() {
        let mut s = OutputStore::new();
        s.insert("a", Value::String("hi".into()));
        s.insert("b", serde_json::json!({"n": 1}));
        let m = s.template_map();
        assert_eq!(m.get("a").unwrap(), "hi");
        assert_eq!(m.get("b").unwrap(), "{\"n\":1}");
    }
}
```

- [ ] **Step 2: Run test**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core output_store`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/output_store.rs crates/tau-runtime-core/src/interpreter/mod.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(runtime): add OutputStore for pipeline step outputs"
```

---

## Task 8: `run_pipeline` — agent steps

**Files:**
- Create: `crates/tau-runtime-core/src/interpreter/pipeline.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs` (`pub mod pipeline;`)
- Test: an integration test under `crates/tau-runtime-core/tests/` reusing the existing `MockLlmBackend` test dispatcher (search `tests/` and `src` for `MockLlmBackend` / the dispatcher used by `run_ir` tests, and mirror that harness).

- [ ] **Step 1: Make `last_assistant_text` reusable**

In `crates/tau-runtime-core/src/interpreter/agent_loop.rs:66`, change `fn last_assistant_text` to `pub(crate) fn last_assistant_text`.

- [ ] **Step 2: Create `pipeline.rs` (agent steps only)**

```rust
//! Engine-sequenced pipeline executor.
//!
//! Runs `IrModule.workflow.pipeline` steps in order, threading each
//! step's output through an [`OutputStore`] so `${steps.<id>.output}`
//! resolves. Agent steps run the existing agent loop; tool and
//! deterministic steps (Task 9) run the existing dispatch paths.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;

use serde_json::Value;
use tau_domain::{Address, Message, MessagePayload};
use tau_ir::pipeline::StepRun;
use tau_ir::IrModule;

use crate::error::RuntimeError;
use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::output_store::OutputStore;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;

/// Drive an `IrModule`'s pipeline to completion, returning all step
/// outputs. The module's `workflow.pipeline` must be `Some` (callers
/// branch on that — see `tau run`).
pub async fn run_pipeline<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
) -> Result<OutputStore, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let pipeline = module
        .workflow
        .pipeline
        .clone()
        .ok_or_else(|| RuntimeError::Internal {
            detail: "run_pipeline called on a module without a pipeline".to_string(),
        })?;

    let mut store = OutputStore::new();

    for step in &pipeline.steps {
        let rendered = tau_ir::template::resolve(&step.input, &input, &store.template_map())
            .map_err(|e| RuntimeError::Internal {
                detail: alloc::format!("pipeline step {}: {e}", step.id.0),
            })?;

        let output: Value = match &step.run {
            StepRun::Agent(agent_id) => {
                let agent = module
                    .workflow
                    .agents
                    .get(agent_id)
                    .ok_or_else(|| RuntimeError::AgentNotFound { agent: agent_id.0.clone() })?
                    .clone();
                let initial = vec![user_message(&rendered)];
                // Box::pin: agent loops may recurse into subflows.
                let outcome =
                    Box::pin(run_agent(module.clone(), &agent, dispatcher.clone(), initial)).await?;
                match outcome {
                    RunOutcome::Completed { .. } => Value::String(last_assistant_text(&outcome)),
                    RunOutcome::Failed { status, .. } => {
                        return Err(RuntimeError::Internal {
                            detail: alloc::format!(
                                "pipeline step {} (agent {}) failed: {status:?}",
                                step.id.0, agent_id.0
                            ),
                        })
                    }
                    _ => Value::String(last_assistant_text(&outcome)),
                }
            }
            // Tool + Deterministic added in Task 9.
            other => {
                return Err(RuntimeError::Internal {
                    detail: alloc::format!("pipeline run target not yet supported: {other:?}"),
                })
            }
        };

        store.insert(step.id.0.clone(), output);
    }

    Ok(store)
}

/// Build a user-turn [`Message`] carrying `content` as its text payload.
fn user_message(content: &str) -> Message {
    // Mirror how `tau run` builds its initial message (see
    // crates/tau-cli/src/cmd/run.rs:231) — use the same Address shape.
    Message::new(
        Address::user(),
        Address::agent("pipeline"),
        MessagePayload::Text { content: content.to_string() },
    )
}
```

> **Verify before running:** (a) the exact `Address` constructors — read `run.rs:225–235` and copy whatever it uses for sender/recipient (e.g. `Address::user()` / `Address::agent(...)` may have different names); (b) that `RuntimeError::Internal { detail }` and `RuntimeError::AgentNotFound { agent }` are the real variant shapes (read `crates/tau-runtime-core/src/error.rs`). Adjust to match.

- [ ] **Step 3: Wire `mod.rs`**

Add `pub mod pipeline;` to `crates/tau-runtime-core/src/interpreter/mod.rs` and re-export if siblings are re-exported.

- [ ] **Step 4: Write the failing integration test**

Create `crates/tau-runtime-core/tests/pipeline_executor.rs`. Reuse the mock dispatcher/backend that existing `run_ir` tests use (find it: `grep -rn "MockLlmBackend\|impl ToolDispatcher" crates/tau-runtime-core`). The test builds a 2-step agent pipeline where step `b`'s input references `${steps.a.output}`, scripts the mock LLM to echo its input, and asserts `b`'s stored output contains `a`'s output:

```rust
// Pseudocode shape — adapt to the real mock harness:
// 1. Build an IrModule with two agents "a","b" and
//    workflow.pipeline = [ {id:"a", run:Agent("a"), input:"${input}"},
//                          {id:"b", run:Agent("b"), input:"prev=${steps.a.output}"} ].
// 2. Script the mock backend so each agent returns its user input as final text.
// 3. let store = run_pipeline(Arc::new(module), "SEED".into(), dispatcher).await.unwrap();
// 4. assert store.get("a") == "SEED"
//    assert store.get("b") contains "prev=SEED"
```

Write it concretely against the real harness (the conformance crate's `DevMode` builder in `crates/tau-ir-conformance/src/` is a working reference for constructing a dispatcher around a `mock_llm.jsonl` cassette — you may build the `IrModule` directly in-test instead of from TOML).

- [ ] **Step 5: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core pipeline_executor`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/pipeline.rs crates/tau-runtime-core/src/interpreter/mod.rs crates/tau-runtime-core/src/interpreter/agent_loop.rs crates/tau-runtime-core/tests/pipeline_executor.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(runtime): run_pipeline executes agent steps with output threading"
```

---

## Task 9: `run_pipeline` — tool + deterministic steps

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/pipeline.rs`
- Test: `crates/tau-runtime-core/tests/pipeline_executor.rs`

- [ ] **Step 1: Add a parse-or-wrap helper**

Add to `pipeline.rs`:

```rust
/// Turn a rendered template string into the `Value` a tool/deterministic
/// step receives: parse it as JSON if it parses, else wrap as a string.
fn rendered_to_args(rendered: &str) -> Value {
    serde_json::from_str::<Value>(rendered).unwrap_or_else(|_| Value::String(rendered.to_string()))
}
```

- [ ] **Step 2: Replace the `other =>` arm with tool + deterministic handling**

```rust
            StepRun::Tool(tool_id) => {
                let args = rendered_to_args(&rendered);
                let result = dispatcher.invoke(tool_id, &args).await?;
                match (result.body, result.error) {
                    (Some(body), _) => body,
                    (None, Some(err)) => {
                        return Err(RuntimeError::Internal {
                            detail: alloc::format!(
                                "pipeline step {} (tool {}) errored: {err}",
                                step.id.0, tool_id.0
                            ),
                        })
                    }
                    (None, None) => Value::Null,
                }
            }
            StepRun::Deterministic(step_node_id) => {
                let registry = dispatcher.deterministic_registry().ok_or_else(|| {
                    RuntimeError::Internal {
                        detail: alloc::format!(
                            "pipeline step {} needs a deterministic registry, none provided",
                            step.id.0
                        ),
                    }
                })?;
                let node = module
                    .workflow
                    .steps
                    .get(step_node_id)
                    .ok_or_else(|| RuntimeError::Internal {
                        detail: alloc::format!("unknown deterministic step {}", step_node_id.0),
                    })?;
                let args = rendered_to_args(&rendered);
                crate::interpreter::deterministic::run_step(node, registry.as_ref(), &args)?
            }
```

> Confirm `dispatcher.invoke` returns `ToolInvocationResult { body: Option<Value>, error: Option<String> }` (it does — `tool_dispatch.rs`) and that `run_step(step, registry, args)` returns `Result<Value, RuntimeError>` (it does — `deterministic.rs:13`).

- [ ] **Step 3: Add a deterministic-step test**

Extend `pipeline_executor.rs` with a test using a dispatcher whose `deterministic_registry()` returns a registry mapping a fn name to e.g. an upper-casing fn. Build a pipeline `[ agent "a" (echo), deterministic "b" run=Deterministic(node) input="${steps.a.output}" ]` and assert `b`'s output is the transformed value. (Mirror the conformance crate's `DeterministicRegistry` impl — `grep -rn "impl DeterministicRegistry" crates/`.)

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core pipeline_executor`
Expected: PASS (agent + deterministic tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/interpreter/pipeline.rs crates/tau-runtime-core/tests/pipeline_executor.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(runtime): run_pipeline supports tool + deterministic steps"
```

---

## Task 10: Step trace events

**Files:**
- Modify: `crates/tau-runtime-core/src/vocabulary.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/pipeline.rs`
- Test: `crates/tau-runtime-core/tests/pipeline_executor.rs`

- [ ] **Step 1: Add vocabulary constants**

Append to `crates/tau-runtime-core/src/vocabulary.rs` (match the existing `pub const EV_* / SPAN_*` style + the drift test the crate keeps — read the file header; there may be a test asserting all constants are listed, which you must update):

```rust
/// Span wrapping one pipeline step's execution.
pub const SPAN_PIPELINE_STEP: &str = "pipeline.step";
/// Emitted when a pipeline step begins.
pub const EV_PIPELINE_STEP_STARTED: &str = "pipeline.step_started";
/// Emitted when a pipeline step completes successfully.
pub const EV_PIPELINE_STEP_COMPLETED: &str = "pipeline.step_completed";
```

- [ ] **Step 2: Emit events in `run_pipeline`**

At the top of the per-step loop body in `pipeline.rs`, wrap the step in a span and emit start/complete (use the crate's existing `tracing` imports — see `stream.rs:236`):

```rust
        let _span = tracing::info_span!(
            crate::vocabulary::SPAN_PIPELINE_STEP,
            id = step.id.0.as_str()
        )
        .entered();
        tracing::info!(
            name: crate::vocabulary::EV_PIPELINE_STEP_STARTED,
            id = step.id.0.as_str()
        );
```

…and after `store.insert(...)`:

```rust
        tracing::info!(
            name: crate::vocabulary::EV_PIPELINE_STEP_COMPLETED,
            id = step.id.0.as_str()
        );
```

> Match the exact `tracing` macro form the crate already uses (the `name:` field syntax vs `name =` — copy from `stream.rs:236`). Keep `_span` entered across the step body (don't drop it early).

- [ ] **Step 3: Assert events in a test**

Add a test using `tracing-test` or the crate's existing trace-capture harness (search `grep -rn "tracing_test\|with_subscriber\|trace capture" crates/tau-runtime-core`). Run a one-step pipeline and assert both `pipeline.step_started` and `pipeline.step_completed` were emitted with `id`. If no capture harness exists, assert via the JSONL run-log path the crate already tests for other events (mirror that test).

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core pipeline`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/vocabulary.rs crates/tau-runtime-core/src/interpreter/pipeline.rs crates/tau-runtime-core/tests/pipeline_executor.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(runtime): emit pipeline.step_started/completed trace events"
```

---

## Task 11: CLI wiring — `tau run` branches to the pipeline

**Files:**
- Modify: `crates/tau-cli/src/cmd/run.rs`
- Test: a CLI integration test under `crates/tau-cli/tests/` (mirror an existing `tau run` test)

- [ ] **Step 1: Branch on `pipeline.is_some()`**

Read `run.rs` around the lowering + `run_ir` call (`run_ir` is referenced; the initial `Message::new` is at line 231). After lowering to the `IrModule`, branch:

```rust
if module.workflow.pipeline.is_some() {
    let store = tau_runtime_core::interpreter::pipeline::run_pipeline(
        std::sync::Arc::new(module),
        run_input_string,            // the same top-level input run.rs feeds the single agent
        dispatcher,
    )
    .await?;
    // Render the final step's output as the run result.
    // (Pick the last pipeline step's id; print its stored output.)
    render_pipeline_result(&store);   // implement as a small helper matching run.rs's output style
} else {
    // existing single-entry path: run_ir(module, &entry_agent, dispatcher, initial) ...
}
```

> Read how `run.rs` currently obtains the input string and the `dispatcher`, and how it renders `RunOutcome` to the user; mirror that rendering for the pipeline's final-step output (look up the last `pipeline.steps` id in the `OutputStore`). Keep the single-entry branch byte-for-byte as it is today.

- [ ] **Step 2: Write a CLI integration test**

Add `crates/tau-cli/tests/run_pipeline.rs` (mirror an existing `tau run` integration test — `grep -rn "tau run\|cmd::run\|assert_cmd\|Command::cargo_bin" crates/tau-cli/tests`). Create a temp project `tau.toml` with two `mock-llm` agents and a 2-step `[[pipeline.steps]]`, run `tau run`, and assert the output reflects the threaded result. Set `$TAU_HOME` to a per-test tempdir with a pre-created `config.toml` (per the repo's Windows test pattern).

- [ ] **Step 3: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli run_pipeline`
Expected: PASS.

- [ ] **Step 4: Confirm single-agent path is unchanged**

Run the existing `tau run` tests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli run`
Expected: PASS (no regressions).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/run.rs crates/tau-cli/tests/run_pipeline.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(cli): tau run drives a pipeline when one is declared"
```

---

## Task 12: TypeScript authoring parity

**Files:**
- Modify: `crates/tau-ts-extract/src/factory.rs` (recognize a `pipeline([...])` call)
- Modify: `crates/tau-ts-extract/src/lower.rs` (build + emit `[[pipeline.steps]]` TOML)
- Test: `crates/tau-ts-extract/tests/fixtures/...` + `fan_monitor_conformance.rs`

Read `crates/tau-ts-extract/src/{factory,lower}.rs` first; the strategy is "build intermediate IR → serialize to TOML → `ProjectConfig::parse_str`", so this task only needs to **emit the same `[[pipeline.steps]]` TOML** Task 4 already parses.

- [ ] **Step 1: Recognize the factory call**

In `factory.rs`, add a `Pipeline` variant to the factory enum and recognize a top-level `pipeline([...])` (or `definePipeline([...])` — match the naming convention the other factories use, e.g. `agent(...)`/`tool(...)`). Extract each element's `id`, `run`, `input` string literals.

- [ ] **Step 2: Emit TOML**

In `lower.rs`'s TOML builder (the `build_toml`-style function), after agents/tools, emit for each pipeline step:

```text
[[pipeline.steps]]
id = "<id>"
run = "<run>"
input = "<input>"
```

(Quote/escape strings the same way the existing emitter does for prompts.)

- [ ] **Step 3: Add a parity fixture**

Add a `[[pipeline.steps]]` block to the existing conformance fixture pair (`crates/tau-ts-extract/tests/fixtures/fan_monitor_conformance/` — both `tau.toml` and `project.ts`), declaring one pipeline step. The conformance test (`fan_monitor_conformance.rs`) already asserts byte-equal canonical IR between the TOML and TS surfaces; extending the fixture exercises the new path.

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract`
Expected: PASS (byte-equal canonical IR with the pipeline present on both surfaces).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ts-extract/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ts-extract): pipeline authoring parity for [[pipeline.steps]]"
```

---

## Task 13: Conformance fixture

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/08_pipeline_sequence/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/08_pipeline_sequence/mock_llm.jsonl`
- Modify: `crates/tau-ir-conformance/tests/conformance.rs`

Read fixture `01_agent_native_tool/` (all files) and the test pattern in `conformance.rs:26–81` first.

- [ ] **Step 1: Create the fixture `workflow.toml`**

```toml
[project]
name = "fixture-08"

[agents.gather]
display_name = "Gather"
package      = "demo@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
max_turns    = 1

[agents.writer]
display_name = "Writer"
package      = "demo@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
max_turns    = 1

[[pipeline.steps]]
id    = "gather"
run   = "agent:gather"
input = "${input}"

[[pipeline.steps]]
id    = "writer"
run   = "agent:writer"
input = "${steps.gather.output}"
```

- [ ] **Step 2: Create `mock_llm.jsonl`**

Script both agents to end their turn echoing their input as final text. Copy the JSONL frame shape from `01_agent_native_tool/mock_llm.jsonl` (read it) — two agent turns, each a single end-of-turn text frame. (If the conformance harness keys cassette entries per agent/turn, ensure both `gather` and `writer` get a scripted final response.)

- [ ] **Step 3: Add tests in `conformance.rs`**

Mirror fixture-01's two tests, but assert the pipeline path. The conformance `DevMode`/`BundleMode` `run(&Path)` may currently only drive single-entry `run_ir`; if so, extend the mode runner(s) in `crates/tau-ir-conformance/src/lib.rs` to call `run_pipeline` when `module.workflow.pipeline.is_some()` (same branch as the CLI in Task 11), capturing step outputs into the `ConformanceReport`. Then:

```rust
#[tokio::test(flavor = "current_thread")]
async fn fixture_08_dev_mode_threads_outputs() {
    let dir = fixture_dir("08_pipeline_sequence");
    let report = DevMode.run(&dir).await;
    // assert the run completed and writer saw gather's output
    // (exact assertions depend on what ConformanceReport captures —
    //  follow fixture-01's assertion style).
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_08_cross_mode_conformance() {
    let dir = fixture_dir("08_pipeline_sequence");
    let dev = DevMode.run(&dir).await;
    let bundle = BundleMode.run(&dir).await;
    assert_conform(&dev, &bundle);
}
```

- [ ] **Step 4: Run tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance fixture_08`
Expected: PASS (dev-mode + cross-mode).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir-conformance/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "test(conformance): add fixture 08 pipeline sequence (dev + cross-mode)"
```

---

## Final verification

- [ ] **Workspace build + lints + full test sweep** (per-crate, per CLAUDE.md):

```bash
for c in tau-ir tau-pkg tau-runtime-core tau-cli tau-ts-extract tau-ir-conformance; do
  timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p "$c" || break
done
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ir --doc
```

- [ ] **Confirm additive invariant:** a project with no `[[pipeline.steps]]` produces an `IrModule` whose `workflow.pipeline == None` and runs via the unchanged single-entry path. (Covered by existing tau-cli `run` tests passing in Task 11 Step 4.)

- [ ] **Open the PR** once green (branch already exists; follow repo PR rules — plain `git push`, `gh pr create --base main`).
