# EPIC 4.2a — Branch authoring end-to-end Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user author a conditional `Branch` in `tau.toml` that lowers to `StepRun::Branch`, typechecks, and runs on both the native interpreter and the wasm guest.

**Architecture:** Add a `branch` form to the `[[pipeline.steps]]` authoring model (tau-pkg) and a recursive lowering arm (tau-ir-lower). The IR typecheck and interpreter already handle `Branch` (4.1 #444 / 4.2 #454) — this slice makes them *reachable* from authored input. The branch condition reuses the existing `[goals.*]` predicate vocabulary verbatim (no new IR/contract surface). wasm parity is proven, not built: the guest shares the same load gate + interpreter; the one missing piece is a guest-side deterministic registry so `Branch` condition dispatch succeeds in-wasm.

**Tech Stack:** Rust (no_std IR core), serde/TOML authoring, thiserror at boundaries, `cargo nextest`. Design spec: `docs/superpowers/specs/2026-07-23-epic-4-2a-branch-authoring-design.md`.

## Global Constraints

- **CARGO (CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p <crate>`. Always `-p`, always `timeout`, never bare cargo. Test=300s, build/check=180s. Doctests use `cargo test --doc`.
- **Branch/PR:** work on `epic-4-2a-branch-authoring`; PR base `main`; never push to main. Merge queue ON — enroll with bare `gh pr merge <N> --squash --auto` (NO `--delete-branch`).
- **Boundaries:** `thiserror` at crate boundaries, `anyhow` internally; `forbid(unsafe_code)`. TDD — failing test first.
- **Scope guard (spec):** NO compound conditions / NO expression DSL in TOML. NO Parallel/Loop/Suspend authoring. Do NOT import #494 feature-fit. No `ir_format` bump (Branch is within v2.5.0).
- **Conflict note:** this slice owns the tau-pkg pipeline authoring model + the tau-ir-lower pipeline lowering match. 4.2b/4.2c are blocked behind it — no other lane touches `project.rs`/`parse.rs` pipeline code while open.

## File map

- `crates/tau-pkg/src/project/project.rs` — extend `UncheckedPipelineStep`, add `UncheckedCondition`/`ConditionConfig`, add `Branch` to `PipelineRunRef`, extract shared `parse_predicate`, make `validate_pipeline` recursive. (Task 1)
- `crates/tau-ir-lower/src/lower/parse.rs` — recursive `lower_step` + `lower_condition`, replacing the flat `.map`. (Task 2)
- `crates/tau-ir-lower/tests/lower_e2e.rs` — authored-branch e2e lowering/typecheck tests. (Task 2)
- `crates/tau-ir-conformance/fixtures/20_branch_route/{workflow.toml,mock_llm.jsonl,expected_report.json}` + `tests/conformance.rs` wiring. (Task 3)
- `crates/tau-wasm-guest/src/dispatcher.rs` — add `deterministic_registry()` override (empty registry). (Task 4)
- `crates/tau-wasm-host/tests/roundtrip.rs` — Branch parity test. (Task 4)
- `docs/how-to/authoring-a-branch.md` + `docs/SUMMARY.md`. (Task 5)

---

### Task 1: Authoring a Branch in tau-pkg

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`UncheckedPipelineStep` ~304-314; `PipelineRunRef` ~336-347; `validate_pipeline` ~1821-1856; `validate_goal` ~1874-1963; tests ~3529+)

**Interfaces:**
- Produces:
  - `PipelineRunRef::Branch { on: ConditionConfig, then: Vec<PipelineStepConfig>, otherwise: Vec<PipelineStepConfig> }`
  - `pub struct ConditionConfig { pub evaluates: LocusConfig, pub predicate: GoalPredicateConfig }`
  - shared `fn parse_predicate(check: Option<&str>, pattern: Option<String>, equals: Option<String>, min_count: Option<u64>, schema: Option<serde_json::Value>, r#fn: Option<String>) -> Result<GoalPredicateConfig, PredicateParseError>` with `enum PredicateParseError { Invalid(String), BadRegex(String) }`
- Consumes: existing `parse_locus`, `LocusConfig`, `GoalPredicateConfig`, `ProjectConfigError::{PipelineValidation, GoalValidation, BadGoalRegex}`.

- [ ] **Step 1: Write failing parse tests**

Add to the `tests` module in `project.rs` (after `parses_check_pipeline_step`, ~3564):

```rust
#[test]
fn parses_branch_pipeline_step() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "triage"
        run = "agent:triage"
        input = "${input}"
        [[pipeline.steps]]
        id = "route"
        branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }
        [[pipeline.steps.then]]
        id = "escalate"
        run = "agent:oncall"
        input = "${steps.triage.output}"
        [[pipeline.steps.otherwise]]
        id = "ack"
        run = "agent:writer"
        input = "${steps.triage.output}"
    "#;
    let cfg = ProjectConfig::parse_str(toml).expect("parses");
    let pipe = cfg.pipeline.expect("pipeline present");
    assert_eq!(pipe.steps.len(), 2);
    match &pipe.steps[1].run {
        PipelineRunRef::Branch { on, then, otherwise } => {
            assert_eq!(on.evaluates, LocusConfig::Output("triage".into()));
            assert_eq!(on.predicate, GoalPredicateConfig::Matches("(?i)urgent".into()));
            assert_eq!(then.len(), 1);
            assert_eq!(then[0].run, PipelineRunRef::Agent("oncall".into()));
            assert_eq!(otherwise.len(), 1);
            assert_eq!(otherwise[0].run, PipelineRunRef::Agent("writer".into()));
        }
        other => panic!("expected Branch, got {other:?}"),
    }
}

#[test]
fn parses_one_armed_branch_defaults_otherwise_empty() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "route"
        branch = { evaluates = "steps.x.output", check = "non_empty" }
        [[pipeline.steps.then]]
        id = "go"
        run = "agent:go"
    "#;
    let cfg = ProjectConfig::parse_str(toml).expect("parses");
    let pipe = cfg.pipeline.expect("pipeline present");
    match &pipe.steps[0].run {
        PipelineRunRef::Branch { otherwise, .. } => assert!(otherwise.is_empty()),
        other => panic!("expected Branch, got {other:?}"),
    }
}

#[test]
fn rejects_step_with_both_run_and_branch() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "bad"
        run = "agent:x"
        branch = { evaluates = "steps.x.output", check = "exists" }
    "#;
    assert!(ProjectConfig::parse_str(toml).is_err());
}

#[test]
fn rejects_then_without_branch() {
    let toml = r#"
        [project]
        name = "demo"
        [[pipeline.steps]]
        id = "leaf"
        run = "agent:x"
        [[pipeline.steps.then]]
        id = "orphan"
        run = "agent:y"
    "#;
    assert!(ProjectConfig::parse_str(toml).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-pkg parses_branch_pipeline_step rejects_step_with_both_run_and_branch`
Expected: FAIL to compile (`branch` unknown field / `PipelineRunRef::Branch` missing).

- [ ] **Step 3: Extend the raw + validated model**

Replace `UncheckedPipelineStep` (~304-314) with:

```rust
/// Raw `[[pipeline.steps]]` entry (pre-validation).
///
/// A step is either a **leaf** (`run = "<kind>:<id>"`) or a **branch**
/// (`branch = { <condition> }` + nested `then`/`otherwise` step arrays).
/// The two forms are mutually exclusive; `validate_pipeline` enforces this.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedPipelineStep {
    /// Step handle.
    pub id: String,
    /// Leaf form: `"agent:<id>"` | `"tool:<id>"` | `"deterministic:<id>"` | `"check:<id>"`.
    #[serde(default)]
    pub run: Option<String>,
    /// Input template; defaults to `"${input}"` when omitted.
    pub input: Option<String>,
    /// Branch form: the condition (mirrors the `[goals.*]` field-set).
    #[serde(default)]
    pub branch: Option<UncheckedCondition>,
    /// Branch form: steps run when the condition holds (recursive).
    #[serde(default)]
    pub then: Vec<UncheckedPipelineStep>,
    /// Branch form: steps run when it does not hold (recursive; may be empty).
    #[serde(default)]
    pub otherwise: Vec<UncheckedPipelineStep>,
}

/// Raw branch condition — the exact field-set of `[goals.*]` minus the
/// table-key id. Reuses the goal predicate menu verbatim (no new grammar).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedCondition {
    /// Read locus: a filesystem path or `steps.<id>.output`.
    pub evaluates: String,
    /// Menu predicate name (mutually exclusive with `fn`).
    #[serde(default)]
    pub check: Option<String>,
    /// Regex for `check = "matches"`.
    #[serde(default)]
    pub pattern: Option<String>,
    /// Expected value for `check = "equals"`.
    #[serde(default)]
    pub equals: Option<String>,
    /// Threshold for `check = "min_count"`.
    #[serde(default)]
    pub min_count: Option<u64>,
    /// JSON schema for `check = "schema_valid"`.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    /// Native-fn escape hatch (`<crate>::<path>`), mutually exclusive with `check`.
    #[serde(default, rename = "fn")]
    pub r#fn: Option<String>,
}
```

Add the `Branch` variant to `PipelineRunRef` (~336-347):

```rust
pub enum PipelineRunRef {
    /// `agent:<id>`
    Agent(String),
    /// `tool:<id>`
    Tool(String),
    /// `deterministic:<id>`
    Deterministic(String),
    /// `check:<id>` — explicitly position a postcondition check.
    Check(String),
    /// A conditional branch: run `then` if `on` holds, else `otherwise`.
    Branch {
        /// Branch condition (locus + predicate).
        on: ConditionConfig,
        /// Steps run when `on` holds.
        then: Vec<PipelineStepConfig>,
        /// Steps run when `on` does not hold (may be empty).
        otherwise: Vec<PipelineStepConfig>,
    },
}

/// A validated branch condition — mirrors `tau_ir::check::Condition`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionConfig {
    /// Read locus.
    pub evaluates: LocusConfig,
    /// Predicate applied to the locus.
    pub predicate: GoalPredicateConfig,
}
```

- [ ] **Step 4: Extract the shared predicate parser**

Add above `validate_goal` (~1874):

```rust
/// Error from parsing a predicate menu shared by `[goals.*]` and branch
/// conditions. Callers map these onto their own `ProjectConfigError` variant.
pub(crate) enum PredicateParseError {
    /// Structural problem (bad/missing selector or companion field).
    Invalid(String),
    /// `check = "matches"` pattern failed to compile.
    BadRegex(String),
}

/// Parse the predicate menu (`check`/`pattern`/`equals`/`min_count`/`schema`)
/// or the `fn` escape hatch into a [`GoalPredicateConfig`]. `check` and `fn`
/// are mutually exclusive and exactly one must be present. Shared by goals
/// and branch conditions so the vocabulary can never drift between them.
pub(crate) fn parse_predicate(
    check: Option<&str>,
    pattern: Option<String>,
    equals: Option<String>,
    min_count: Option<u64>,
    schema: Option<serde_json::Value>,
    r#fn: Option<String>,
) -> Result<GoalPredicateConfig, PredicateParseError> {
    match (r#fn, check) {
        (Some(_), Some(_)) => {
            return Err(PredicateParseError::Invalid(
                "only one of `fn` or `check` may be set".into(),
            ))
        }
        (None, None) => {
            return Err(PredicateParseError::Invalid(
                "one of `fn` or `check` must be set".into(),
            ))
        }
        (Some(fn_name), None) => return Ok(GoalPredicateConfig::NativeFn(fn_name)),
        (None, Some(_)) => {}
    }
    let predicate = match check.unwrap() {
        "exists" => GoalPredicateConfig::Exists,
        "non_empty" => GoalPredicateConfig::NonEmpty,
        "equals" => match equals {
            Some(v) => GoalPredicateConfig::Equals(v),
            None => {
                return Err(PredicateParseError::Invalid(
                    "check = \"equals\" requires the `equals` field".into(),
                ))
            }
        },
        "matches" => match pattern {
            Some(p) => {
                if let Err(e) = regex::Regex::new(&p) {
                    return Err(PredicateParseError::BadRegex(e.to_string()));
                }
                GoalPredicateConfig::Matches(p)
            }
            None => {
                return Err(PredicateParseError::Invalid(
                    "check = \"matches\" requires the `pattern` field".into(),
                ))
            }
        },
        "min_count" => match min_count {
            Some(n) => GoalPredicateConfig::MinCount(n),
            None => {
                return Err(PredicateParseError::Invalid(
                    "check = \"min_count\" requires the `min_count` field".into(),
                ))
            }
        },
        "schema_valid" => match schema {
            Some(s) => GoalPredicateConfig::SchemaValid(s),
            None => {
                return Err(PredicateParseError::Invalid(
                    "check = \"schema_valid\" requires the `schema` field".into(),
                ))
            }
        },
        other => {
            return Err(PredicateParseError::Invalid(format!(
                "unknown check {other:?}; valid values: exists, non_empty, equals, matches, min_count, schema_valid"
            )))
        }
    };
    Ok(predicate)
}
```

Then rewrite `validate_goal`'s body (~1874-1963) to delegate, preserving its existing error variants:

```rust
fn validate_goal(id: String, raw: UncheckedGoal) -> Result<GoalEntry, ProjectConfigError> {
    let evaluates = parse_locus(&raw.evaluates);
    let predicate = parse_predicate(
        raw.check.as_deref(),
        raw.pattern,
        raw.equals,
        raw.min_count,
        raw.schema,
        raw.r#fn,
    )
    .map_err(|e| match e {
        PredicateParseError::BadRegex(message) => {
            ProjectConfigError::BadGoalRegex { id: id.clone(), message }
        }
        PredicateParseError::Invalid(message) => {
            ProjectConfigError::GoalValidation { id: id.clone(), message }
        }
    })?;
    Ok(GoalEntry { id, evaluates, predicate })
}
```

- [ ] **Step 5: Make `validate_pipeline` recursive (leaf vs branch)**

Replace `validate_pipeline` (~1821-1856) with a top-level function (keeps the empty-pipeline + top-level dup-id checks) delegating to a per-step `validate_pipeline_step`:

```rust
fn validate_pipeline(raw: &UncheckedPipeline) -> Result<PipelineConfig, ProjectConfigError> {
    if raw.steps.is_empty() {
        return Err(ProjectConfigError::EmptyPipeline);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut steps = Vec::with_capacity(raw.steps.len());
    for s in &raw.steps {
        if !seen.insert(s.id.clone()) {
            return Err(ProjectConfigError::PipelineValidation {
                id: s.id.clone(),
                message: format!("step id {:?} declared more than once", s.id),
            });
        }
        steps.push(validate_pipeline_step(s)?);
    }
    Ok(PipelineConfig { steps })
}

/// Validate one step — a leaf (`run`) or a branch (`branch` + arms). Nested
/// arm steps recurse through this same function. Tree-wide id uniqueness and
/// output-scope integrity are enforced later by the IR typecheck
/// (`check_pipeline` / `validate_step_run`), which already walks nested blocks.
fn validate_pipeline_step(
    s: &UncheckedPipelineStep,
) -> Result<PipelineStepConfig, ProjectConfigError> {
    let has_branch = s.branch.is_some();
    let has_run = s.run.is_some();
    let has_arms = !s.then.is_empty() || !s.otherwise.is_empty();

    let run = match (has_run, has_branch) {
        (true, true) => {
            return Err(ProjectConfigError::PipelineValidation {
                id: s.id.clone(),
                message: "a step has both `run` and `branch`; exactly one is allowed".into(),
            })
        }
        (false, false) => {
            return Err(ProjectConfigError::PipelineValidation {
                id: s.id.clone(),
                message: "a step must set either `run` or `branch`".into(),
            })
        }
        (true, false) => {
            if has_arms {
                return Err(ProjectConfigError::PipelineValidation {
                    id: s.id.clone(),
                    message: "`then`/`otherwise` are only valid on a `branch` step".into(),
                });
            }
            let run_str = s.run.as_deref().unwrap();
            match run_str.split_once(':') {
                Some(("agent", id)) => PipelineRunRef::Agent(id.to_string()),
                Some(("tool", id)) => PipelineRunRef::Tool(id.to_string()),
                Some(("deterministic", id)) => PipelineRunRef::Deterministic(id.to_string()),
                Some(("check", id)) => PipelineRunRef::Check(id.to_string()),
                _ => {
                    return Err(ProjectConfigError::PipelineValidation {
                        id: s.id.clone(),
                        message: format!(
                            "run must be \"agent:<id>\" | \"tool:<id>\" | \"deterministic:<id>\" | \"check:<id>\", got {run_str:?}"
                        ),
                    })
                }
            }
        }
        (false, true) => {
            let cond = s.branch.as_ref().unwrap();
            let predicate = parse_predicate(
                cond.check.as_deref(),
                cond.pattern.clone(),
                cond.equals.clone(),
                cond.min_count,
                cond.schema.clone(),
                cond.r#fn.clone(),
            )
            .map_err(|e| ProjectConfigError::PipelineValidation {
                id: s.id.clone(),
                message: match e {
                    PredicateParseError::BadRegex(m) => format!("branch condition: {m}"),
                    PredicateParseError::Invalid(m) => format!("branch condition: {m}"),
                },
            })?;
            let on = ConditionConfig {
                evaluates: parse_locus(&cond.evaluates),
                predicate,
            };
            let then = s
                .then
                .iter()
                .map(validate_pipeline_step)
                .collect::<Result<Vec<_>, _>>()?;
            let otherwise = s
                .otherwise
                .iter()
                .map(validate_pipeline_step)
                .collect::<Result<Vec<_>, _>>()?;
            PipelineRunRef::Branch { on, then, otherwise }
        }
    };

    Ok(PipelineStepConfig {
        id: s.id.clone(),
        run,
        input: s.input.clone().unwrap_or_else(|| "${input}".to_string()),
    })
}
```

- [ ] **Step 6: Run the new + existing tau-pkg tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-pkg`
Expected: PASS — the four new branch tests plus all existing pipeline/goal tests (`parses_pipeline_steps`, `parses_check_pipeline_step`, `rejects_unknown_run_kind`, `rejects_empty_pipeline`, goal validation tests) stay green (the `parse_predicate` refactor is behavior-preserving).

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-pkg): author a conditional branch in [[pipeline.steps]]"
```

---

### Task 2: Lower an authored Branch to `StepRun::Branch`

**Files:**
- Modify: `crates/tau-ir-lower/src/lower/parse.rs` (pipeline `.map`, ~272-288; near `lower_locus`/`lower_predicate` ~538-561)
- Test: `crates/tau-ir-lower/tests/lower_e2e.rs`

**Interfaces:**
- Consumes: `PipelineRunRef::Branch`, `ConditionConfig` (Task 1); existing `lower_locus`, `lower_predicate`.
- Produces: `StepRun::Branch { on: tau_ir::check::Condition, then, otherwise }` in the lowered pipeline; recursive `fn lower_step(&PipelineStepConfig) -> PipelineStep`.

- [ ] **Step 1: Write the failing e2e test**

Add to `crates/tau-ir-lower/tests/lower_e2e.rs` (model on `lowers_goals_and_deliverables_into_checks`):

```rust
#[test]
fn lowers_authored_branch_into_steprun_branch() {
    use tau_ir::check::{GoalPredicate, Locus};
    use tau_ir::pipeline::StepRun;

    let toml = r#"
packages = ["mock-llm"]
[project]
name = "branch-demo"
[models.m]
backend = "mock-llm"
model = "m"
[agents.triage]
display_name = "Triage"
package = "branch-demo@^0.1"
model = "m"
max_turns = 1
[agents.oncall]
display_name = "Oncall"
package = "branch-demo@^0.1"
model = "m"
max_turns = 1
[agents.writer]
display_name = "Writer"
package = "branch-demo@^0.1"
model = "m"
max_turns = 1
[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }
[[pipeline.steps.then]]
id = "escalate"
run = "agent:oncall"
input = "${steps.triage.output}"
[[pipeline.steps.otherwise]]
id = "ack"
run = "agent:writer"
input = "${steps.triage.output}"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let module = lower_project(&config, &target, &caches).expect("lower").module;

    let pipe = module.workflow.pipeline.as_ref().expect("pipeline present");
    assert_eq!(pipe.steps.len(), 2);
    match &pipe.steps[1].run {
        StepRun::Branch { on, then, otherwise } => {
            assert!(matches!(&on.evaluates, Locus::Output(s) if s.0 == "triage"));
            assert!(matches!(&on.predicate, GoalPredicate::Matches(p) if p == "(?i)urgent"));
            assert_eq!(then.len(), 1);
            assert!(matches!(&then[0].run, StepRun::Agent(a) if a.0 == "oncall"));
            assert_eq!(otherwise.len(), 1);
            assert!(matches!(&otherwise[0].run, StepRun::Agent(a) if a.0 == "writer"));
        }
        other => panic!("expected Branch, got {other:?}"),
    }
}

#[test]
fn branch_arm_referencing_ghost_agent_is_rejected() {
    let toml = r#"
packages = ["mock-llm"]
[project]
name = "branch-ghost"
[models.m]
backend = "mock-llm"
model = "m"
[agents.triage]
display_name = "Triage"
package = "branch-ghost@^0.1"
model = "m"
max_turns = 1
[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "non_empty" }
[[pipeline.steps.then]]
id = "boom"
run = "agent:ghost"
input = "${input}"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let err = lower_project(&config, &target, &caches).unwrap_err();
    assert!(
        matches!(err, LowerError::UnknownPipelineRun { .. }),
        "expected UnknownPipelineRun for ghost arm agent, got {err:?}"
    );
}

#[test]
fn branch_condition_reading_out_of_scope_output_is_rejected() {
    // `route` reads `steps.later.output`, but `later` runs AFTER `route`.
    let toml = r#"
packages = ["mock-llm"]
[project]
name = "branch-scope"
[models.m]
backend = "mock-llm"
model = "m"
[agents.triage]
display_name = "Triage"
package = "branch-scope@^0.1"
model = "m"
max_turns = 1
[agents.later]
display_name = "Later"
package = "branch-scope@^0.1"
model = "m"
max_turns = 1
[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.later.output", check = "non_empty" }
[[pipeline.steps.then]]
id = "arm"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "later"
run = "agent:later"
input = "${input}"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let err = lower_project(&config, &target, &caches).unwrap_err();
    assert!(
        matches!(err, LowerError::ConditionUnknownOutput { .. }),
        "expected ConditionUnknownOutput, got {err:?}"
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-ir-lower lowers_authored_branch_into_steprun_branch`
Expected: FAIL to compile — the lowering match is non-exhaustive (missing `PipelineRunRef::Branch`).

- [ ] **Step 3: Add recursive lowering**

In `parse.rs`, replace the pipeline `.map` block (~273-288) with a call to a recursive helper:

```rust
    // --- Pipeline ---------------------------------------------------------
    let mut pipeline = config.pipeline.as_ref().map(|p| Pipeline {
        steps: p.steps.iter().map(lower_step).collect(),
    });
```

Add these free functions near `lower_locus`/`lower_predicate` (~538):

```rust
/// Lower one validated pipeline step to an IR [`PipelineStep`], recursing
/// into `Branch` arms. Reference/scope integrity is validated separately by
/// the IR typecheck (`check_pipeline`), so this mapping stays infallible.
fn lower_step(s: &tau_pkg::project::PipelineStepConfig) -> PipelineStep {
    use tau_pkg::project::PipelineRunRef;
    let run = match &s.run {
        PipelineRunRef::Agent(id) => StepRun::Agent(AgentId(id.clone())),
        PipelineRunRef::Tool(id) => StepRun::Tool(ToolId(id.clone())),
        PipelineRunRef::Deterministic(id) => StepRun::Deterministic(StepId(id.clone())),
        PipelineRunRef::Check(id) => StepRun::Check(CheckId(id.clone())),
        PipelineRunRef::Branch { on, then, otherwise } => StepRun::Branch {
            on: lower_condition(on),
            then: then.iter().map(lower_step).collect(),
            otherwise: otherwise.iter().map(lower_step).collect(),
        },
    };
    PipelineStep {
        id: PipelineStepId(s.id.clone()),
        run,
        input: s.input.clone(),
    }
}

/// Lower a tau-pkg [`ConditionConfig`] to an IR [`Condition`], reusing the
/// same locus/predicate mappings as `[goals.*]`.
fn lower_condition(c: &tau_pkg::project::ConditionConfig) -> tau_ir::check::Condition {
    tau_ir::check::Condition {
        evaluates: lower_locus(&c.evaluates),
        predicate: lower_predicate(&c.predicate),
    }
}
```

Ensure `use tau_pkg::project::PipelineRunRef;` at the top of `parse.rs` (line ~10) still compiles — it is already imported; the `lower_step` `use` is a local shadow and can be dropped if the module import suffices. Confirm `CheckId` is in scope (already used at line 283).

- [ ] **Step 4: Run to verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-ir-lower`
Expected: PASS — the three new tests plus all existing lower_e2e/typecheck tests.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ir-lower/src/lower/parse.rs crates/tau-ir-lower/tests/lower_e2e.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-ir-lower): lower authored branch to StepRun::Branch"
```

---

### Task 3: End-to-end conformance fixture (native, DevMode + BundleMode)

**Files:**
- Create: `crates/tau-ir-conformance/fixtures/20_branch_route/workflow.toml`
- Create: `crates/tau-ir-conformance/fixtures/20_branch_route/mock_llm.jsonl`
- Create: `crates/tau-ir-conformance/fixtures/20_branch_route/expected_report.json`
- Modify: `crates/tau-ir-conformance/tests/conformance.rs`

**Interfaces:**
- Consumes: authored-branch support (Tasks 1–2) + existing interpreter Branch execution (#454). The DevMode dispatcher already supplies a deterministic registry (`dev_mode.rs:166`) and mock LLM.
- Produces: a CI fixture proving author→lower→typecheck→interpret for a Branch under both execution modes.

- [ ] **Step 1: Write the fixture `workflow.toml`**

Model on `fixtures/08_pipeline_sequence/workflow.toml`. Condition uses `matches` so the mock output deterministically selects the `then` arm:

```toml
packages = ["mock-llm"]

[project]
name = "fixture-20"

[models.mock-1]
backend = "mock-llm"
model = "mock-1"

[agents.triage]
display_name = "Triage"
package      = "demo@^0.1"
model        = "mock-1"
max_turns    = 1

[agents.oncall]
display_name = "Oncall"
package      = "demo@^0.1"
model        = "mock-1"
max_turns    = 1

[agents.writer]
display_name = "Writer"
package      = "demo@^0.1"
model        = "mock-1"
max_turns    = 1

[[pipeline.steps]]
id    = "triage"
run   = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id     = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "urgent" }

  [[pipeline.steps.then]]
  id    = "escalate"
  run   = "agent:oncall"
  input = "${steps.triage.output}"

  [[pipeline.steps.otherwise]]
  id    = "ack"
  run   = "agent:writer"
  input = "${steps.triage.output}"
```

- [ ] **Step 2: Write `mock_llm.jsonl`**

`triage` (turn 0) emits text containing `urgent` → the `then` arm (`escalate`, turn 1) runs. Two turns total (only one arm executes):

```
{"turn": 0, "response": {"text": "urgent: reactor overheating", "stop_reason": "end_turn"}}
{"turn": 1, "response": {"text": "escalated to oncall", "stop_reason": "end_turn"}}
```

- [ ] **Step 3: Write `expected_report.json`**

Mirror `08_pipeline_sequence/expected_report.json`:

```json
{
  "run_outcome_kind": "Completed",
  "tool_calls": {},
  "message_added_count": 0
}
```

- [ ] **Step 4: Wire the fixture into the harness**

In `crates/tau-ir-conformance/tests/conformance.rs`, add a test alongside the existing per-fixture tests (find how `08_pipeline_sequence` is invoked — the file drives `assert_conform` per fixture). Add:

```rust
#[test]
fn fixture_20_branch_route() {
    assert_conform(&fixture_dir("20_branch_route"));
}
```

(If fixtures are enumerated by a directory scan + `DEFERRED_FIXTURES` list rather than one test each, instead confirm `20_branch_route` is picked up by the scan and is not in `DEFERRED_FIXTURES`. Match the file's actual pattern — read it before editing.)

- [ ] **Step 5: Run the conformance suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-ir-conformance`
Expected: PASS — `fixture_20_branch_route` green under DevMode AND BundleMode (BundleMode serializes → `from_canonical_bytes` → interprets, the same path wasm uses).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir-conformance/fixtures/20_branch_route crates/tau-ir-conformance/tests/conformance.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(tau-ir-conformance): end-to-end authored-branch fixture"
```

---

### Task 4: wasm parity — guest deterministic registry + roundtrip test

**Files:**
- Modify: `crates/tau-wasm-guest/src/dispatcher.rs` (`impl ToolDispatcher for GuestDispatcher`, ~41)
- Modify: `crates/tau-wasm-host/tests/roundtrip.rs`

**Interfaces:**
- Consumes: authored-branch support (Tasks 1–2); `DeterministicRegistry` trait (`invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError>`); existing `build_guest_component`, `run_component`, `end_turn_response`, `trivial_ir_bytes` helpers.
- Produces: `GuestDispatcher::deterministic_registry()` returning an empty registry so `Branch`/`Check` condition dispatch succeeds in-wasm; an `#[ignore]` parity test proving a Branch IR runs in the guest.

**Rationale:** The `Branch` interpreter arm calls `dispatcher.deterministic_registry().ok_or_else(|| Internal)`. `GuestDispatcher` currently inherits the trait default (`None`), so a Branch would fail *only* in wasm. Menu predicates (`matches`/`non_empty`/…) never call `invoke`; an empty registry is sufficient and honest for this slice.

- [ ] **Step 1: Write the failing parity test**

Add to `crates/tau-wasm-host/tests/roundtrip.rs`. First a branch-IR helper (mirrors `trivial_ir_bytes`, but authored with a branch whose `then` arm always runs via `non_empty`):

```rust
/// Lower a branch fixture to canonical IR bytes (`then` arm always taken).
fn branch_ir_bytes() -> Vec<u8> {
    let toml = r#"
packages = ["anthropic"]

[project]
name = "branch-wasm"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.triage]
display_name = "Triage"
package = "branch-wasm@^0.1"
model = "claude"
[agents.triage.prompt]
system = "Reply and stop."

[agents.arm]
display_name = "Arm"
package = "branch-wasm@^0.1"
model = "claude"
[agents.arm.prompt]
system = "Reply and stop."

[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "non_empty" }

  [[pipeline.steps.then]]
  id = "arm"
  run = "agent:arm"
  input = "${steps.triage.output}"
"#;
    let config = tau_pkg::project::ProjectConfig::parse_str(toml).expect("fixture parses");
    let target: tau_ports::target::TargetTriple = "any-wasi-strict".parse().unwrap();
    let caches = tau_ir_lower::Caches {
        native_tool: &|_| Some([0u8; 32]),
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|_| Ok(Vec::new()),
    };
    let module = tau_ir_lower::lower_project(&config, &target, &caches)
        .expect("lowers")
        .module;
    tau_ir::to_canonical_bytes(&module)
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn guest_runs_authored_branch() {
    // Two agents run: `triage` then the `then` arm `arm` (non_empty is always
    // true on triage's non-empty reply). Feed one end-turn response per agent.
    let component = build_guest_component(Some(&branch_ir_bytes()));
    let out = run_component(&component, "hi", vec![end_turn_response(), end_turn_response()])
        .expect("guest runs the baked branch IR");
    let events: Vec<tau_runtime_core::stream::RunEvent> =
        serde_json::from_str(&out).expect("guest output is a RunEvent array");
    assert!(
        matches!(events.first(), Some(tau_runtime_core::stream::RunEvent::RunStarted)),
        "stream must start with RunStarted; got {:?}", events.first()
    );
    assert!(
        matches!(events.last(), Some(tau_runtime_core::stream::RunEvent::RunCompleted { .. })),
        "branch must complete in-wasm; got {:?}", events.last()
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `rustup target add wasm32-wasip2` (once), then
`timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-wasm-host --run-ignored all guest_runs_authored_branch`
Expected: FAIL — guest returns a `RunFailed`/error event: the Branch dispatch hits `Internal { "branch route needs a deterministic registry" }` because `GuestDispatcher::deterministic_registry()` is `None`.

- [ ] **Step 3: Add the empty registry to the guest dispatcher**

In `crates/tau-wasm-guest/src/dispatcher.rs`, add a tiny registry type and override the method. Place the struct near the top of the module and the method inside `impl ToolDispatcher for GuestDispatcher`:

```rust
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;

/// The guest ships no registered deterministic native fns. Menu predicates
/// (`matches`/`non_empty`/`equals`/…) evaluate without a registry; only a
/// `NativeFn` predicate would consult one, and that is out of scope for the
/// authored-branch slice — surface it as a clear error rather than a panic.
struct EmptyDeterministicRegistry;

impl DeterministicRegistry for EmptyDeterministicRegistry {
    fn invoke(&self, fn_name: &str, _args: &serde_json::Value) -> Result<serde_json::Value, RuntimeError> {
        Err(RuntimeError::Internal {
            message: format!("tau-wasm-guest: no deterministic fn `{fn_name}` registered"),
        })
    }
}
```

Add inside the `impl ToolDispatcher for GuestDispatcher` block:

```rust
    fn deterministic_registry(&self) -> Option<Arc<dyn DeterministicRegistry>> {
        Some(Arc::new(EmptyDeterministicRegistry))
    }
```

Confirm `Arc`, `RuntimeError`, and `serde_json::Value` are already imported in the file (they are used by `invoke`); add `use` lines if the compiler flags them.

- [ ] **Step 4: Run to verify pass**

Run: `timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-wasm-host --run-ignored all guest_runs_authored_branch`
Expected: PASS — the guest decodes the branch IR, evaluates the condition, runs the `then` arm, and returns `RunStarted … RunCompleted`. This proves native/wasm parity: same load gate, same interpreter, same result.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-wasm-guest/src/dispatcher.rs crates/tau-wasm-host/tests/roundtrip.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-wasm-guest): deterministic registry so branches run in-wasm"
```

---

### Task 5: Docs example

**Files:**
- Create: `docs/how-to/authoring-a-branch.md`
- Modify: `docs/SUMMARY.md`

**Interfaces:**
- Consumes: the final authored-branch syntax (Task 1). Docs must match the shipped syntax exactly.

- [ ] **Step 1: Write the how-to page**

Create `docs/how-to/authoring-a-branch.md`:

````markdown
# Authoring a conditional branch

A `[[pipeline.steps]]` entry is normally a **leaf** — it runs one agent, tool,
deterministic step, or check:

```toml
[[pipeline.steps]]
id  = "triage"
run = "agent:triage"
```

To route between two paths, make the step a **branch** instead. A branch has a
condition (`branch = { … }`) and two nested step arrays, `then` and
`otherwise`:

```toml
[[pipeline.steps]]
id    = "triage"
run   = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id     = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }

  [[pipeline.steps.then]]
  id    = "escalate"
  run   = "agent:oncall"
  input = "${steps.triage.output}"

  [[pipeline.steps.otherwise]]
  id    = "ack"
  run   = "agent:writer"
  input = "${steps.triage.output}"
```

If `steps.triage.output` matches the pattern, the `then` arm runs; otherwise
the `otherwise` arm runs.

## The condition

`branch` reuses the same predicate vocabulary as `[goals.*]`. `evaluates` names
the value to test — a `steps.<id>.output` reference or a filesystem path — and
one predicate selector decides the verdict:

| `check`        | companion field | holds when …                       |
| -------------- | --------------- | ---------------------------------- |
| `exists`       | —               | the locus resolves                 |
| `non_empty`    | —               | it resolves and is non-empty       |
| `equals`       | `equals`        | it equals the literal              |
| `matches`      | `pattern`       | it matches the regex               |
| `min_count`    | `min_count`     | it has at least N items            |
| `schema_valid` | `schema`        | it validates against the schema    |

Or use `fn = "<crate>::<path>"` for a registered native predicate instead of
`check`.

## One-armed branches

Omit `otherwise` to do nothing when the condition is false:

```toml
[[pipeline.steps]]
id     = "maybe-review"
branch = { evaluates = "steps.draft.output", check = "non_empty" }

  [[pipeline.steps.then]]
  id  = "review"
  run = "agent:reviewer"
```

## Scope

Branch arms share the pipeline's flat output namespace: an arm step may read
any earlier step's `${steps.<id>.output}`, and later steps may read an arm's
output by id. A condition may only read outputs produced **before** the branch.
These rules are checked at build time — a forward or out-of-scope reference
fails `tau build`.

> Deeply nested or expression-heavy control flow is better authored in
> TypeScript (`tau-ts-extract`), which lowers to the same IR. TOML branches are
> intended for shallow, declarative routing.
````

- [ ] **Step 2: Register in `SUMMARY.md`**

Add a line under the existing How-to section of `docs/SUMMARY.md` (match the surrounding indentation/format — read the file first):

```markdown
  - [Authoring a conditional branch](how-to/authoring-a-branch.md)
```

- [ ] **Step 3: Build the book locally**

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines, no linkcheck errors. Then `rm -rf docs/book`.

- [ ] **Step 4: Commit**

```bash
git add docs/how-to/authoring-a-branch.md docs/SUMMARY.md
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(how-to): authoring a conditional branch"
```

---

### Task 6: Final verification + PR

- [ ] **Step 1: Full-crate gate for every touched crate**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-ir-lower
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo nextest run -p tau-ir-conformance
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e42a cargo clippy -p tau-pkg -p tau-ir-lower --all-targets
```
Expected: all green; clippy clean (workspace treats warnings as deny).

- [ ] **Step 2: Push and open PR**

```bash
git push -u origin epic-4-2a-branch-authoring
gh pr create --base main --title "feat(EPIC 4.2a): branch authoring end-to-end" \
  --body "Authors a conditional Branch in tau.toml → lowers to StepRun::Branch → typechecks → runs (native + wasm parity). Conformance fixture 20_branch_route (DevMode+BundleMode) + docs how-to. Scope: drops the #494 IrFeature flip (not on main; wasm gating is major-version-based and Branch is v2.5.0-compatible) — see docs/superpowers/specs/2026-07-23-epic-4-2a-branch-authoring-design.md. Does NOT include Parallel (4.2b) or Loop (4.2c). 🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

- [ ] **Step 3: Enroll in the merge queue**

```bash
gh pr merge <N> --squash --auto
```
(NO `--delete-branch` — it conflicts with the queue. Poke `gh pr update-branch <N>` if the PR goes BEHIND.)

## Self-review notes

- **Spec coverage:** syntax → T1; lowering → T2; typecheck reachability → proven by T2 (ghost/scope rejections) + T3 (conformance); interpreter → verified by T3; IrFeature flip → intentionally dropped (spec); wasm parity → T4 (incl. the real guest-registry gap); conformance fixture → T3; docs → T5.
- **Type consistency:** `ConditionConfig`/`parse_predicate`/`PredicateParseError` defined in T1 and consumed by name in T2; `lower_step`/`lower_condition` names stable across T2/T4; fixture name `20_branch_route` stable T3.
- **No `ir_format` bump** and **no #494 import**, per Global Constraints.
