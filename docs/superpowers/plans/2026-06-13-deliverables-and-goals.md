# Deliverables & Goals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two build-time-checked postcondition primitives to the canonical `tau.toml` → IR path — `goal` (deterministic predicate) and `deliverable` (produced artifact + LLM-judged content) — with an opt-in rewind-to-gate retry loop, all riding on the Plan 1 pipeline executor.

**Architecture:** A new `Check` value rides on `IrModule.workflow.checks` and is positioned in the pipeline via a new `StepRun::Check(CheckId)` step (lowered from `[goals.*]`/`[deliverables.*]`; auto-appended at the pipeline tail unless explicitly placed with `run = "check:<id>"`). The **goal arm reuses the deterministic-fn machinery**: menu predicates (`exists`/`non_empty`/`equals`/`matches`/`min_count`/`schema_valid`) are host-registered built-in fns invoked through the existing `DeterministicRegistry`, and the native-fn escape hatch (`fn = "..."`) routes to the same registry. The **deliverable arm reuses the agent/judge machinery**: a deterministic existence floor, then an LLM judge (a synthesized built-in agent or a user `[agents.*]`) returning `{met, rationale}`. Build-time semantic checks (producer binding, capability coverage, gate position, retry-span-has-LLM, regex-compiles, judge resolution) live in `tau-pkg`'s project validator where `produces`, capabilities, `glob_subset`, the pipeline order, and `std` all coexist. The single genuinely new engine capability — **bounded, budget-capped rewind iteration** — lives in `run_pipeline`, which becomes an index loop that can jump back to the gate step and re-run forward with the failure rationale injected into agent steps.

**Tech Stack:** Rust workspace — `tau-ir` (`no_std`+`alloc`+`deny(missing_docs)`), `tau-runtime-core` (`no_std`+`alloc`), `tau-runtime-tokio`/`tau-cli` (host, `std`), `tau-pkg` (project config), `tau-ts-extract`, `tau-ir-conformance`. Tests via `cargo nextest` + doctests.

**Spec:** `docs/superpowers/specs/2026-06-13-deliverables-and-goals-design.md`

---

## Key implementation decisions (review before executing)

These resolve forks the spec left to the plan phase. If you disagree, raise it before Task 1 — they shape every downstream task.

1. **`Check` is NOT a `Node` enum variant.** The `Node` enum (`Agent`/`Tool`/`Deterministic`/`Subflow`) is a type abstraction that never appears in the serialized `IrModule` — only the `Workflow` `BTreeMap`s do. So a `Check` lives in a new `workflow.checks: BTreeMap<CheckId, Check>` map and is positioned by a new `StepRun::Check(CheckId)` pipeline step. This keeps `Node` untouched (no ripple through code that exhaustively matches `Node`).

2. **Checks are pipeline steps, default-appended at the tail.** The authoring surface (`[goals.*]`/`[deliverables.*]`) carries no position field, so lowering appends one `StepRun::Check` step per check at the end of `pipeline.steps`, in deterministic order (goals by id, then deliverables by id). A check may *also* be placed explicitly with a pipeline step `run = "check:<id>"`; an explicitly-placed check is not auto-appended. This satisfies the worked example (no explicit placement) while leaving the "checkpoints anywhere" door open with zero rework.

3. **All build-time *semantic* checks live in `tau-pkg`'s `validate()`.** `tau-pkg` owns `ProjectConfig`, the pipeline order, agent `produces`, tool capabilities, and `glob_subset`, and has `std` (regex). `tau-ir` only depends on `tau-pkg` (not the reverse), so doing the producer/gate/judge/regex checks in `tau-pkg` needs no new cross-crate dep. `tau-ir` lowering keeps only *structural* integrity checks (referenced ids exist), matching the existing typecheck split.

4. **Goal predicates are host-registered `DeterministicRegistry` fns.** `tau-runtime-core` (`no_std`) cannot run regex. `run_pipeline` resolves the locus to content (file via the artifact reader, or named output from the store), builds a JSON args object `{present, content, ...params}`, and calls `registry.invoke("<predicate-fn>", &args)`. The host (`tau-cli`/conformance) registers the six menu predicates under reserved fn names; the escape-hatch `fn` routes to the same registry. This is exactly "the goal arm reuses the deterministic-fn machinery."

5. **The artifact reader is a defaulted `ToolDispatcher` method.** Mirroring `deterministic_registry()`/`clock()`/`random()`, add `fn artifact_reader(&self) -> Option<Arc<dyn ArtifactReader>> { None }`. No change to `run_pipeline`'s signature or its callers. The tokio host returns a `std::fs` reader; tests return an in-memory mock.

6. **`judge_model` is parsed/validated/stored/traced but a runtime no-op in v1.** The dispatcher exposes a *single* `llm_backend()`, and `Agent.model` is *already* ignored at runtime today (the request uses `backend.name()`). Per-agent/judge model selection needs multi-backend resolution that does not exist yet. So the built-in judge and any custom judge run on the ambient backend in v1. This is the spec's own "honest limit" applied to the judge; it is stated in the docs/ADR, not papered over.

7. **Bundle format `v1.1.0` → `v1.2.0` (MINOR, additive).** `workflow.checks` serializes with `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]` and `StepRun::Check` never appears in a check-free pipeline, so every existing fixture's canonical bytes are unchanged. Task 16 asserts this.

---

## Conventions for every task (read once)

- **Cargo (per repo CLAUDE.md):** never run bare `cargo`. Always:
  `timeout <secs> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>`
  (test=300s, build/check=180s, clippy=240s, fmt=30s). Doctests: `... cargo test -p <crate> --doc`. Pick a fresh `target/agent-impl-N` if `pgrep -af cargo` shows another build on your target dir.
- **Commits (per repo CLAUDE.md):** `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "..."`. Conventional commits, imperative. Commit after each task's tests pass. `--no-verify` is sanctioned only for the pre-existing tau-pkg echo-tool fixture failure.
- **`tau-ir` is `#![no_std]` + `#![deny(missing_docs)]`:** new code uses `alloc::` (not `std::`); every public item needs a doc comment. Build-time check code under `tau-ir/src/lower/` runs with `feature = "with-std-adapters"` (so `std` is available *there*), but the crate's non-`lower` modules stay `no_std`.
- **`tau-runtime-core` is `#![no_std]` + `alloc`:** check-evaluation code uses `alloc::` and `serde_json` (which works under `alloc`). No regex, no `std::fs` here — those are host-side.
- **`#[non_exhaustive]` types** (`ProjectConfig`, `AgentEntry`, `ToolEntry`, `PipelineConfig`, `RunOutcome`, `Message`) cannot be struct-literal-constructed outside their crate — construct via the crate's own constructor/validator, and match with a `_ => {}` arm.
- **TDD rhythm per task:** write the failing test → run it, confirm it fails for the expected reason → write the minimal implementation → run, confirm pass → `cargo fmt` + clippy → commit.

---

## Canonical type vocabulary (defined once, used everywhere)

These exact names are used across tasks. Do not rename.

**`tau-pkg` (`crates/tau-pkg/src/project/project.rs`):**
```rust
// Added field on UncheckedAgent / AgentEntry
produces: Vec<String>

// Unchecked (serde) shapes
struct UncheckedGoal { evaluates: String, check: Option<String>, pattern: Option<String>,
                       equals: Option<String>, min_count: Option<u64>,
                       schema: Option<serde_json::Value>, r#fn: Option<String> }
struct UncheckedDeliverable { path: Option<String>, output: Option<String>, must_satisfy: String,
                              on_fail: Option<String>, max_attempts: Option<u32>,
                              retry_from: Option<String>, judge_model: Option<String>, judge: Option<String> }

// Validated shapes
struct GoalEntry { id: String, evaluates: LocusConfig, predicate: GoalPredicateConfig }
struct DeliverableEntry { id: String, locus: LocusConfig, must_satisfy: String,
                          on_fail: OnFailConfig, max_attempts: u32,
                          retry_from: Option<String>, judge: JudgeConfig }
enum LocusConfig { Path(String), Output(String) }
enum GoalPredicateConfig { Exists, NonEmpty, Equals(String), Matches(String),
                           MinCount(u64), SchemaValid(serde_json::Value), NativeFn(String) }
enum OnFailConfig { Abort, Retry }
enum JudgeConfig { Builtin { model: Option<String> }, Agent(String) }
// ProjectConfig gains: goals: BTreeMap<String, GoalEntry>, deliverables: BTreeMap<String, DeliverableEntry>
```

**`tau-ir` (`crates/tau-ir/src/check.rs`, `ids.rs`, `node.rs`, `pipeline.rs`, `module.rs`):**
```rust
struct CheckId(pub String);                                   // ids.rs
struct Check { id: CheckId, verify: CheckVerify, retry: RetryPolicy }
enum CheckVerify {
    Goal { evaluates: Locus, predicate: GoalPredicate },
    Deliverable { locus: Locus, must_satisfy: String, judge: JudgeRef },
}
enum Locus { Path(String), Output(PipelineStepId) }
enum GoalPredicate { Exists, NonEmpty, Equals(String), Matches(String),
                     MinCount(u64), SchemaValid(Value), NativeFn(NativeFnRef) }
enum JudgeRef { Builtin { model: Option<String> }, Agent(AgentId) }
struct RetryPolicy { on_fail: OnFail, max_attempts: u32, gate: PipelineStepId }
enum OnFail { Abort, Retry }
// Agent gains: produces: Vec<String>          (node.rs)
// Workflow gains: checks: BTreeMap<CheckId, Check>   (module.rs)
// StepRun gains: Check(CheckId)               (pipeline.rs)
```

**`tau-runtime-core` (`crates/tau-runtime-core/src/interpreter/`):**
```rust
trait ArtifactReader: Send + Sync { fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, RuntimeError>; }
struct CheckVerdict { met: bool, rationale: String }   // interpreter/check.rs
// vocabulary.rs constants:
SPAN_PIPELINE_CHECK = "pipeline.check"
EV_CHECK_EVALUATED  = "check.evaluated"
EV_CHECK_RETRY      = "check.retry"
FN_BUILTIN_EXISTS = "__tau::goal::exists"          FN_BUILTIN_NON_EMPTY = "__tau::goal::non_empty"
FN_BUILTIN_EQUALS = "__tau::goal::equals"          FN_BUILTIN_MATCHES   = "__tau::goal::matches"
FN_BUILTIN_MIN_COUNT = "__tau::goal::min_count"    FN_BUILTIN_SCHEMA_VALID = "__tau::goal::schema_valid"
// RuntimeError gains: CheckFailed { id, kind, rationale, attempt }
```

---

## File map

| File | Change | Task |
|------|--------|------|
| `crates/tau-pkg/src/project/project.rs` | `produces` on `UncheckedAgent`/`AgentEntry` | 1 |
| `crates/tau-pkg/src/project/project.rs` | `[goals.*]` parse → `GoalEntry` | 2 |
| `crates/tau-pkg/src/project/project.rs` | `[deliverables.*]` parse → `DeliverableEntry` | 3 |
| `crates/tau-pkg/src/project/project.rs` | `ProjectConfigError` variants + `validate_postconditions` (producer/capability) | 4 |
| `crates/tau-pkg/src/project/project.rs` | gate-position + retry-span + unknown-retry-from checks | 5 |
| `crates/tau-pkg/src/project/project.rs` | regex-compiles + judge-resolution checks | 6 |
| `crates/tau-pkg/src/capability_override/glob_subset.rs` | expose `is_glob_subset` to project module (vis bump if needed) | 4 |
| `crates/tau-ir/src/ids.rs` | add `CheckId` | 7 |
| `crates/tau-ir/src/check.rs` | **new** — `Check`, `CheckVerify`, `Locus`, `GoalPredicate`, `JudgeRef`, `RetryPolicy`, `OnFail` | 7 |
| `crates/tau-ir/src/lib.rs` | module decl + re-exports | 7 |
| `crates/tau-ir/src/node.rs` | `Agent.produces: Vec<String>` | 8 |
| `crates/tau-ir/src/module.rs` | `Workflow.checks` + `IrFormatVersion::CURRENT` → `v1.2.0` | 9 |
| `crates/tau-ir/src/pipeline.rs` | `StepRun::Check(CheckId)` | 10 |
| `crates/tau-ir/src/error.rs` | `UnknownCheckRef` / `UnknownCheckLocus` variants | 11 |
| `crates/tau-ir/src/lower/parse.rs` | lower `produces`, goals/deliverables → checks, position check steps, resolve gate | 12 |
| `crates/tau-ir/src/lower/typecheck.rs` | `StepRun::Check` + `Locus` integrity | 13 |
| `crates/tau-ir/src/canonical.rs` | round-trip test incl. checks | 14 |
| `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` | defaulted `artifact_reader()` | 15 |
| `crates/tau-runtime-core/src/interpreter/artifact.rs` | **new** — `ArtifactReader` trait + in-memory mock | 15 |
| `crates/tau-runtime-core/src/error.rs` | `RuntimeError::CheckFailed` | 16 |
| `crates/tau-runtime-core/src/vocabulary.rs` | check span/event + builtin-fn-name constants | 16 |
| `crates/tau-runtime-core/src/interpreter/check.rs` | **new** — `evaluate_goal`, `evaluate_deliverable`, `CheckVerdict` | 17,18 |
| `crates/tau-runtime-core/src/interpreter/pipeline.rs` | index loop + `StepRun::Check` dispatch + retry/rewind + feedback | 19,20,21 |
| `crates/tau-runtime-tokio/` (or `tau-cli`) | `StdFsArtifactReader` + register builtin predicate fns | 22 |
| `crates/tau-cli/src/cmd/run.rs`, `dev/session.rs` | dispatcher returns reader (wiring) | 22 |
| `crates/tau-ts-extract/src/{factory,lower}.rs` | `goals`/`deliverables`/`produces` parity | 23 |
| `crates/tau-ir-conformance/fixtures/09_*`,`10_*`,`11_*` | **new** fixtures | 24,25,26 |
| `crates/tau-ir-conformance/tests/conformance.rs` | fixture tests | 24,25,26 |
| `crates/tau-cli/tests/` | `tau check` build-refusal integration | 27 |
| `docs/` + ADR | how-to page + ADR-00xx | 28 |

---

## PHASE A — `tau-pkg` config surface + build-time checks

### Task 1: `produces` on agents

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`UncheckedAgent` ~lines 43-80, `AgentEntry` ~lines 389-420, `validate_agent` ~line 691, and `AgentEntry::new`)
- Test: same file's `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

Add to the test module:
```rust
#[test]
fn agent_produces_parses_and_validates() {
    let toml = r#"
[project]
name = "p"

[agents.writer]
display_name = "Writer"
package      = "demo@^0.1"
llm_backend  = "anthropic"
model        = "claude-haiku-4-5"
produces     = ["/workspace/report.md"]
"#;
    let cfg: UncheckedProjectConfig = toml::from_str(toml).expect("parse");
    let validated = cfg.validate().expect("validate");
    assert_eq!(
        validated.agents["writer"].produces,
        vec!["/workspace/report.md".to_string()]
    );
}
```

- [ ] **Step 2: Run it, confirm failure**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg agent_produces_parses_and_validates`
Expected: compile error — no field `produces` on `UncheckedAgent`/`AgentEntry`.

- [ ] **Step 3: Add the field**

In `UncheckedAgent` add:
```rust
    /// Artifact paths / named outputs this agent declares it produces.
    /// Cross-checked against `fs-write` capabilities at validation time
    /// and bound to `[deliverables.*]`/`[goals.*]` loci.
    #[serde(default)]
    pub produces: Vec<String>,
```
In `AgentEntry` add the same doc + `pub produces: Vec<String>,`. In `validate_agent`, thread it through: `produces: raw.produces,`. Update `AgentEntry::new` (and the doctest on `AgentEntry`, if it constructs all fields) to pass `Vec::new()` — or add a `with_produces` builder if `new` is field-positional; keep `new`'s signature stable by defaulting `produces` to empty inside `new` and exposing it as a public field set post-construction. (Check how `new` is written; the existing `new` does not take every field, so just initialize `produces: Vec::new()` inside it.)

- [ ] **Step 4: Run it, confirm pass**

Run: same command. Expected: PASS.

- [ ] **Step 5: fmt + commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-pkg
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  add -A && git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(pkg): add produces declaration to agent config"
```

---

### Task 2: `[goals.*]` parse → `GoalEntry`

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (add `UncheckedGoal`, `GoalEntry`, `LocusConfig`, `GoalPredicateConfig`; `UncheckedProjectConfig.goals`; `ProjectConfig.goals`; `validate_goal`; call in `validate()`)
- Test: same file

`evaluates` resolves to a `LocusConfig`: a value starting with `steps.` and ending `.output` → `Output("<id>")`; otherwise `Path(<string>)`. Predicate selection: if `r#fn` is `Some` → `NativeFn`; else dispatch on `check`: `"exists"`/`"non_empty"`/`"equals"`(needs `equals`)/`"matches"`(needs `pattern`)/`"min_count"`(needs `min_count`)/`"schema_valid"`(needs `schema`). Missing-param or unknown-`check` → `ProjectConfigError::GoalValidation`.

- [ ] **Step 1: Write the failing tests**
```rust
#[test]
fn goal_matches_parses_path_locus_and_regex_predicate() {
    let toml = r#"
[project]
name = "p"
[goals.has_sources]
evaluates = "/workspace/report.md"
check     = "matches"
pattern   = "(?m)^## Sources"
"#;
    let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
    let v = cfg.validate().unwrap();
    let g = &v.goals["has_sources"];
    assert_eq!(g.evaluates, LocusConfig::Path("/workspace/report.md".into()));
    assert_eq!(g.predicate, GoalPredicateConfig::Matches("(?m)^## Sources".into()));
}

#[test]
fn goal_fn_escape_hatch_parses_output_locus() {
    let toml = r#"
[project]
name = "p"
[goals.link_health]
evaluates = "steps.writer.output"
fn        = "research_checks::all_links_resolve"
"#;
    let v: ProjectConfig = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap();
    let g = &v.goals["link_health"];
    assert_eq!(g.evaluates, LocusConfig::Output("writer".into()));
    assert_eq!(g.predicate, GoalPredicateConfig::NativeFn("research_checks::all_links_resolve".into()));
}

#[test]
fn goal_matches_without_pattern_is_rejected() {
    let toml = r#"
[project]
name = "p"
[goals.bad]
evaluates = "/x"
check     = "matches"
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::GoalValidation { .. }));
}
```

- [ ] **Step 2: Run, confirm failure** (`... cargo nextest run -p tau-pkg goal_`) — undefined types.

- [ ] **Step 3: Implement.** Add (deriving `Debug, Clone, PartialEq` on validated types; `Deserialize` on `UncheckedGoal` with `#[serde(deny_unknown_fields)]`):
```rust
/// Raw `[goals.<id>]` table (pre-validation).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedGoal {
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

/// A read locus: a filesystem path or a named step output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocusConfig {
    /// Filesystem path.
    Path(String),
    /// `steps.<id>.output` → the step id.
    Output(String),
}

/// Validated goal predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalPredicateConfig {
    /// Locus resolves to something.
    Exists,
    /// Resolves and is non-empty.
    NonEmpty,
    /// Equals the given literal.
    Equals(String),
    /// Matches the given regex.
    Matches(String),
    /// At least N items (lines/array entries).
    MinCount(u64),
    /// Validates against the given JSON schema.
    SchemaValid(serde_json::Value),
    /// Registered native fn (`<crate>::<path>`).
    NativeFn(String),
}

/// Validated `[goals.<id>]` entry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct GoalEntry {
    /// Goal id (table key).
    pub id: String,
    /// Read locus.
    pub evaluates: LocusConfig,
    /// Verification predicate.
    pub predicate: GoalPredicateConfig,
}
```
Add a free fn `parse_locus(s: &str) -> LocusConfig` (used by goals and deliverables): strip `steps.` prefix + `.output` suffix → `Output`, else `Path`. Add `goals: BTreeMap<String, UncheckedGoal>` (with `#[serde(default)]`) to `UncheckedProjectConfig` and `goals: BTreeMap<String, GoalEntry>` to `ProjectConfig`. Add `GoalValidation { id, message }` to `ProjectConfigError`. Implement `validate_goal(id, raw) -> Result<GoalEntry, ProjectConfigError>`, and in `validate()` loop the goals map. Reject `check` and `fn` both set, neither set, or a `check` whose required param is missing.

- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: fmt + commit** `feat(pkg): parse [goals.*] into validated GoalEntry`.

---

### Task 3: `[deliverables.*]` parse → `DeliverableEntry`

**Files:** same file (add `UncheckedDeliverable`, `DeliverableEntry`, `OnFailConfig`, `JudgeConfig`; maps on both configs; `validate_deliverable`; call in `validate()`).

Rules: exactly one of `path`/`output` (locus) — both or neither → error. `on_fail` defaults `"abort"`; `"retry"`/`"abort"` only. `max_attempts` defaults `1` (abort) — when `on_fail="retry"` and absent, default `3`. Judge: `judge` and `judge_model` both set → error (checked in Task 6, not here — here just record). `judge = Some(a)` → `JudgeConfig::Agent(a)`; else `JudgeConfig::Builtin { model: judge_model }`.

- [ ] **Step 1: Failing test**
```rust
#[test]
fn deliverable_path_locus_retry_parses() {
    let toml = r#"
[project]
name = "p"
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "A coherent summary."
on_fail      = "retry"
max_attempts = 3
retry_from   = "writer"
"#;
    let v = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap();
    let d = &v.deliverables["report"];
    assert_eq!(d.locus, LocusConfig::Path("/workspace/report.md".into()));
    assert_eq!(d.on_fail, OnFailConfig::Retry);
    assert_eq!(d.max_attempts, 3);
    assert_eq!(d.retry_from.as_deref(), Some("writer"));
    assert_eq!(d.judge, JudgeConfig::Builtin { model: None });
}

#[test]
fn deliverable_rejects_both_path_and_output() {
    let toml = r#"
[project]
name = "p"
[deliverables.bad]
path         = "/x"
output       = "steps.writer.output"
must_satisfy = "x"
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::DeliverableValidation { .. }));
}
```

- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** the structs/enums (mirroring Task 2's style), `DeliverableValidation { id, message }` error, `validate_deliverable`, and the maps. `OnFailConfig`/`JudgeConfig` derive `Debug, Clone, PartialEq`.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(pkg): parse [deliverables.*] into validated DeliverableEntry`.

---

### Task 4: Build-time check — producer binding + capability coverage

**Files:** `crates/tau-pkg/src/project/project.rs` (new `validate_postconditions(&ProjectConfig) -> Result<(), ProjectConfigError>`, called at the end of `validate()` after maps are built); `crates/tau-pkg/src/capability_override/glob_subset.rs` (make `is_glob_subset` reachable from the `project` module — change `pub(crate)` to `pub(crate)` is already crate-visible; just `use crate::capability_override::glob_subset::is_glob_subset;`).

A deliverable binds to its producer: the unique agent whose `produces` contains a string resolving to the same locus. Zero producers → `DeliverableNoProducer`. The producer must hold an `fs.write` capability (from the tools it references) whose paths cover the declared `produces` path via `is_glob_subset(child = produces_path, parent = cap_path)`.

> Capability source: an agent's effective write paths = union of `fs.write` paths across `tools` it lists in `tool_refs`. Read `ToolEntry.capabilities` (`Vec<Capability>`), keep `Capability::Filesystem(FsCapability::Write { paths, .. })`.

- [ ] **Step 1: Failing tests**
```rust
#[test]
fn deliverable_without_producer_is_rejected() {
    let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"
llm_backend = "anthropic"
model = "m"
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::DeliverableNoProducer { id } if id == "report"));
}

#[test]
fn producer_lacking_fs_write_capability_is_rejected() {
    let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"
llm_backend = "anthropic"
model = "m"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/other/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::DeliverableProducerLacksCapability { .. }));
}

#[test]
fn producer_with_covering_capability_validates() {
    let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"
llm_backend = "anthropic"
model = "m"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
    assert!(toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().is_ok());
}
```

- [ ] **Step 2: Run, confirm failure** (`... cargo nextest run -p tau-pkg producer` and `deliverable_without_producer`).

- [ ] **Step 3: Implement.** Add error variants:
```rust
    /// A deliverable declares a locus no agent's `produces` covers.
    #[error("deliverable '{id}' has no producer: no step declares produces = [{locus:?}]")]
    DeliverableNoProducer { id: String, locus: String },
    /// More than one agent claims to produce the deliverable's locus.
    #[error("deliverable '{id}' is produced by multiple agents ({agents:?}); a deliverable must bind to exactly one producer")]
    DeliverableAmbiguousProducer { id: String, agents: Vec<String> },
    /// The producing agent holds no fs-write capability covering the path.
    #[error("step '{agent}' declares it produces '{path}' but holds no fs-write capability covering that path")]
    DeliverableProducerLacksCapability { id: String, agent: String, path: String },
```
Implement `validate_postconditions`: for each deliverable, find producer agents (those whose `produces`, each `parse_locus`'d, equals the deliverable locus); 0 → `DeliverableNoProducer`, >1 → `DeliverableAmbiguousProducer`. For a `LocusConfig::Path`, compute the producer's write-paths and require `paths.iter().any(|p| is_glob_subset(path, p))`, else `DeliverableProducerLacksCapability`. (`Output` loci skip the capability check — no fs-write involved.) Store the resolved producer id for Task 5 (return it via a private helper `fn producer_of(cfg, deliverable) -> Result<String, ProjectConfigError>` reused there). Call `validate_postconditions(&result)?` before `Ok(result)` in `validate()`.

- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(pkg): build-time producer binding + capability coverage for deliverables`.

---

### Task 5: Build-time check — gate position + retry-span-has-LLM + unknown retry_from

**Files:** same file (extend `validate_postconditions`).

Only applies when `on_fail == Retry`. Resolve the gate: `retry_from` (a pipeline step id) or default = the producer's *pipeline step* (the step whose `run` is `agent:<producer>`). Build-time checks:
- **Unknown gate** — `retry_from` names no pipeline step → `UnknownRetryFrom`.
- **Guarantee 1 (gate ≤ producer)** — the gate step's index must be `<=` the producer step's index → else `GateAfterProducer`.
- **Guarantee 2 (span has an LLM step)** — some pipeline step in `[gate_index ..= producer_index]` must be an `agent:` step → else `RetrySpanNoLlm`.

> Requires a pipeline. If `on_fail == Retry` but `pipeline` is `None` or the producer agent is not in any pipeline step, that's `RetrySpanNoLlm`/`UnknownRetryFrom` respectively (a retry needs a sequence to rewind). Use `PipelineConfig.steps` with `PipelineRunRef::Agent(id)` matching to locate the producer step.

- [ ] **Step 1: Failing tests**
```rust
fn cfg_with_pipeline(retry_from: &str, polish_after: bool) -> String {
    // gather -> writer (producer) -> [polish], deliverable retries from `retry_from`
    let polish_step = if polish_after {
        "[[pipeline.steps]]\nid=\"polish\"\nrun=\"agent:polish\"\ninput=\"${steps.writer.output}\"\n"
    } else { "" };
    let polish_agent = if polish_after {
        "[agents.polish]\ndisplay_name=\"P\"\npackage=\"d@^0.1\"\nllm_backend=\"anthropic\"\nmodel=\"m\"\n"
    } else { "" };
    format!(r#"
[project]
name = "p"
[agents.gather]
display_name="G"
package="d@^0.1"
llm_backend="anthropic"
model="m"
[agents.writer]
display_name="W"
package="d@^0.1"
llm_backend="anthropic"
model="m"
produces=["/workspace/report.md"]
tool_refs=["write_file"]
{polish_agent}[tools.write_file]
native="WriteFile"
capabilities=[{{ kind="fs.write", paths=["/workspace/**"] }}]
[[pipeline.steps]]
id="gather"
run="agent:gather"
input="${{input}}"
[[pipeline.steps]]
id="writer"
run="agent:writer"
input="${{steps.gather.output}}"
{polish_step}[deliverables.report]
path="/workspace/report.md"
must_satisfy="x"
on_fail="retry"
max_attempts=3
retry_from="{retry_from}"
"#)
}

#[test]
fn retry_gate_before_producer_validates() {
    assert!(toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("gather", false))
        .unwrap().validate().is_ok());
}

#[test]
fn retry_gate_after_producer_is_rejected() {
    let err = toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("polish", true))
        .unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::GateAfterProducer { .. }));
}

#[test]
fn retry_from_unknown_step_is_rejected() {
    let err = toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("nope", false))
        .unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::UnknownRetryFrom { .. }));
}
```
(A `RetrySpanNoLlm` test: build a span whose only steps are `tool:`/`deterministic:` — e.g. gate and producer are the same deterministic-only span. Add it once the deterministic-producer fixture is convenient; at minimum assert the variant exists.)

- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement.** Error variants:
```rust
    /// `retry_from` names a step that does not run before the producer.
    #[error("deliverable '{id}' has retry_from = \"{gate}\" but '{gate}' runs after producer '{producer}' — the gate must be at or before the producer")]
    GateAfterProducer { id: String, gate: String, producer: String },
    /// The retry span has no non-deterministic step, so retrying is a no-op.
    #[error("deliverable '{id}' sets on_fail = \"retry\" but the retry span contains no non-deterministic step; retrying cannot change the result")]
    RetrySpanNoLlm { id: String },
    /// `retry_from` names no pipeline step.
    #[error("deliverable '{id}' has retry_from = \"{gate}\" but no pipeline step has that id")]
    UnknownRetryFrom { id: String, gate: String },
```
Implement the three checks using `PipelineConfig.steps` indices and `PipelineRunRef::Agent` matching. Default gate = producer step id when `retry_from` is `None`.

- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(pkg): build-time gate-position + retry-span guarantees`.

---

### Task 6: Build-time check — regex compiles + judge resolution

**Files:** same file. `regex` is already a workspace dep used elsewhere; confirm `tau-pkg/Cargo.toml` has it, add `regex = { workspace = true }` if absent.

- **Regex compiles** — for every goal with `GoalPredicateConfig::Matches(p)`, `regex::Regex::new(p)` must succeed → else `BadGoalRegex`.
- **Judge mutual exclusion** — `judge` + `judge_model` both set → `JudgeAndModelConflict`. (Recorded in Task 3 as raw fields; check here against the *unchecked* deliverable, so keep the raw map around, or re-derive: a `DeliverableEntry` with `JudgeConfig::Agent` plus a non-None `judge_model` is impossible to represent — so detect the conflict in `validate_deliverable` instead and emit there. **Decision:** move this specific check into `validate_deliverable` (Task 3) and only keep agent-existence here.)
- **Judge agent exists** — `JudgeConfig::Agent(a)` where `a ∉ cfg.agents` → `UnknownJudgeAgent`.

> The spec's `output_schema` *warning* is deferred: `AgentEntry` has no `output_schema` field today (only deterministic steps do), so there is nothing to check. Note this in the ADR (Task 28).

- [ ] **Step 1: Failing tests**
```rust
#[test]
fn goal_bad_regex_is_rejected() {
    let toml = r#"
[project]
name = "p"
[goals.g]
evaluates = "/x"
check     = "matches"
pattern   = "("
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::BadGoalRegex { .. }));
}

#[test]
fn deliverable_judge_and_model_conflict_rejected() {
    let toml = r#"
[project]
name = "p"
[agents.writer]
display_name="W"
package="d@^0.1"
llm_backend="anthropic"
model="m"
produces=["/workspace/report.md"]
[deliverables.report]
path="/workspace/report.md"
must_satisfy="x"
judge="critic"
judge_model="claude-haiku-4-5"
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::JudgeAndModelConflict { .. }));
}

#[test]
fn deliverable_unknown_judge_agent_rejected() {
    let toml = r#"
[project]
name = "p"
[agents.writer]
display_name="W"
package="d@^0.1"
llm_backend="anthropic"
model="m"
produces=["/workspace/report.md"]
[deliverables.report]
path="/workspace/report.md"
must_satisfy="x"
judge="ghost"
"#;
    let err = toml::from_str::<UncheckedProjectConfig>(toml).unwrap().validate().unwrap_err();
    assert!(matches!(err, ProjectConfigError::UnknownJudgeAgent { id, judge } if id=="report" && judge=="ghost"));
}
```

- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** variants + checks:
```rust
    #[error("goal '{id}' has check = \"matches\" but its pattern is not a valid regex: {message}")]
    BadGoalRegex { id: String, message: String },
    #[error("deliverable '{id}' sets both judge_model and judge — a custom judge brings its own model")]
    JudgeAndModelConflict { id: String },
    #[error("deliverable '{id}' sets judge = \"{judge}\" but no [agents.{judge}] is defined")]
    UnknownJudgeAgent { id: String, judge: String },
```
Put `JudgeAndModelConflict` in `validate_deliverable`; `BadGoalRegex` in `validate_goal` (or `validate_postconditions`); `UnknownJudgeAgent` in `validate_postconditions` (needs the agents map).

- [ ] **Step 4: Run, confirm pass. Run the whole tau-pkg suite** to catch regressions: `... cargo nextest run -p tau-pkg`.
- [ ] **Step 5: commit** `feat(pkg): build-time regex + judge resolution checks`.

---

## PHASE B — `tau-ir` IR modeling + lowering

### Task 7: `CheckId` + `check.rs` IR types

**Files:**
- Modify: `crates/tau-ir/src/ids.rs` (add `CheckId`, mirroring `PipelineStepId` lines 10-29)
- Create: `crates/tau-ir/src/check.rs`
- Modify: `crates/tau-ir/src/lib.rs` (`pub mod check;` + re-exports next to `pub mod pipeline;`)

- [ ] **Step 1: Failing test** (in `check.rs`'s test module):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PipelineStepId;
    use alloc::string::ToString;

    #[test]
    fn goal_check_round_trips_through_serde() {
        let c = Check {
            id: CheckId("g".to_string()),
            verify: CheckVerify::Goal {
                evaluates: Locus::Path("/x".to_string()),
                predicate: GoalPredicate::Matches("^#".to_string()),
            },
            retry: RetryPolicy { on_fail: OnFail::Abort, max_attempts: 1,
                                  gate: PipelineStepId("g".to_string()) },
        };
        let bytes = serde_json::to_vec(&c).unwrap();
        let back: Check = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(c, back);
    }
}
```

- [ ] **Step 2: Run, confirm failure** (`... cargo nextest run -p tau-ir goal_check_round_trips`).

- [ ] **Step 3: Implement `ids.rs`** — add (copy `PipelineStepId`'s derives + doc):
```rust
/// Identifier for a postcondition [`Check`](crate::check::Check).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckId(pub String);
```
**Implement `check.rs`** (header `//!`, all items documented):
```rust
//! Postcondition checks: `goal` (deterministic predicate) and
//! `deliverable` (produced artifact + LLM-judged content).

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AgentId, CheckId, PipelineStepId};
use crate::tool_impl::NativeFnRef;

/// A postcondition evaluated at a point in the pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// Identifier within the workflow.
    pub id: CheckId,
    /// What is verified and how.
    pub verify: CheckVerify,
    /// Failure handling.
    pub retry: RetryPolicy,
}

/// The two postcondition kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckVerify {
    /// Deterministic predicate over a read locus.
    Goal {
        /// Read locus.
        evaluates: Locus,
        /// Predicate to apply.
        predicate: GoalPredicate,
    },
    /// Produced artifact whose content an LLM judge evaluates.
    Deliverable {
        /// Produced locus.
        locus: Locus,
        /// Natural-language acceptance criterion.
        must_satisfy: String,
        /// Who judges the content.
        judge: JudgeRef,
    },
}

/// A read/produce locus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Locus {
    /// Filesystem path.
    Path(String),
    /// Named pipeline-step output (`steps.<id>.output`).
    Output(PipelineStepId),
}

/// Deterministic goal predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GoalPredicate {
    /// Locus resolves.
    Exists,
    /// Resolves and non-empty.
    NonEmpty,
    /// Equals the literal.
    Equals(String),
    /// Matches the regex.
    Matches(String),
    /// At least N items.
    MinCount(u64),
    /// Validates against the JSON schema.
    SchemaValid(Value),
    /// Registered native fn.
    NativeFn(NativeFnRef),
}

/// Who evaluates a deliverable's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JudgeRef {
    /// tau's built-in minimalist judge, optionally on a chosen model.
    Builtin {
        /// `judge_model` override (runtime no-op in v1 — see ADR).
        model: Option<String>,
    },
    /// A user `[agents.*]` used as judge.
    Agent(AgentId),
}

/// Failure handling for a check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Abort vs rewind-and-retry.
    pub on_fail: OnFail,
    /// Maximum check evaluations (>= 1).
    pub max_attempts: u32,
    /// Rewind point — at or before the producer step.
    pub gate: PipelineStepId,
}

/// `on_fail` discriminant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnFail {
    /// Exit non-zero with the rationale.
    Abort,
    /// Rewind to the gate and re-run forward.
    Retry,
}
```
(Confirm `NativeFnRef` is exported from `tool_impl`; if not, import from wherever `Deterministic.fn_ref` is typed.)

- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(ir): add Check IR types (goal + deliverable arms)`.

---

### Task 8: `produces` on the IR `Agent` node

**Files:** `crates/tau-ir/src/node.rs` (`Agent` struct, lines 29-44).

- [ ] **Step 1: Failing test** (node.rs test module): construct an `Agent` with `produces: vec!["/x".into()]`, serde round-trip, assert equal. (The compile error is the failure.)
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** — add to `Agent`:
```rust
    /// Artifact loci this agent declares it produces (deliverable binding).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: alloc::vec::Vec<alloc::string::String>,
```
The `skip_serializing_if` keeps existing canonical bytes stable for produce-less agents (asserted in Task 16). Fix every `Agent { .. }` literal in the crate (parse.rs lowering + tests) to set `produces`.
- [ ] **Step 4: Run, confirm pass** (`... cargo nextest run -p tau-ir`).
- [ ] **Step 5: commit** `feat(ir): add produces field to Agent node`.

---

### Task 9: `Workflow.checks` + version bump

**Files:** `crates/tau-ir/src/module.rs` (`Workflow` struct lines 54-71; `IrFormatVersion::CURRENT` line 28).

- [ ] **Step 1: Failing test** — a test asserting `IrFormatVersion::CURRENT == "v1.2.0"` and that a default `Workflow` has an empty `checks` map.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** — add to `Workflow`:
```rust
    /// Postcondition checks, keyed by id. Positioned in the pipeline via
    /// `StepRun::Check`.
    #[serde(default, skip_serializing_if = "alloc::collections::BTreeMap::is_empty")]
    pub checks: alloc::collections::BTreeMap<crate::ids::CheckId, crate::check::Check>,
```
Bump `CURRENT` to `"v1.2.0"`. Update every `Workflow { .. }` literal (lowering `build_module`, tests) to set `checks: BTreeMap::new()`.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(ir): carry checks on Workflow; bump format to v1.2.0`.

---

### Task 10: `StepRun::Check` variant

**Files:** `crates/tau-ir/src/pipeline.rs` (`StepRun` enum lines 30-39).

- [ ] **Step 1: Failing test** — round-trip a `PipelineStep` whose `run` is `StepRun::Check(CheckId("c".into()))`.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** — add to `StepRun`:
```rust
    /// Evaluate a postcondition check.
    Check(crate::ids::CheckId),
```
This forces new match arms; the compiler will point them out (typecheck.rs, the interpreter — handled in Tasks 13/19). For now add `StepRun::Check(_) => {}` / `unreachable!()` placeholders only where the crate won't compile (tau-ir typecheck) — but prefer to land Task 13's real arm in the same session to avoid a placeholder.
- [ ] **Step 4: Run, confirm pass** (`... cargo nextest run -p tau-ir` — expect typecheck.rs to need the arm; if so, proceed straight to Task 13 before committing, then commit both).
- [ ] **Step 5: commit** `feat(ir): add StepRun::Check pipeline-step variant`.

---

### Task 11: `IrError` variants for check integrity

**Files:** `crates/tau-ir/src/error.rs`.

- [ ] **Step 1: Failing test** — assert the variants exist by constructing them in a small test, or fold into Task 13's tests. (Minimal: add variants, no separate test; Task 13 exercises them.)
- [ ] **Step 2/3: Implement**:
```rust
    /// A `StepRun::Check` references a check id absent from `workflow.checks`.
    #[error("pipeline step '{step}' runs check '{check}' but no such check is defined")]
    UnknownCheckRef { step: String, check: String },
    /// A check's `Locus::Output` references an unknown / later pipeline step.
    #[error("check '{check}' evaluates output of '{output}' which is not an earlier pipeline step")]
    UnknownCheckLocus { check: String, output: String },
```
- [ ] **Step 4: Run** `... cargo build -p tau-ir`. Expected: compiles.
- [ ] **Step 5: commit** `feat(ir): add check-integrity IrError variants`.

---

### Task 12: Lowering — `produces`, goals/deliverables → checks, step positioning, gate resolution

**Files:** `crates/tau-ir/src/lower/parse.rs` (agent loop ~lines 91-119; pipeline lowering ~lines 178-193; `build_module`).

This is the heart of Phase B. Lowering must:
1. Copy `entry.produces` onto the IR `Agent`.
2. Lower each `GoalEntry` → `Check { verify: Goal, retry: Abort/maxattempts1/gate=self }` (goals don't retry in v1 — `on_fail` is a deliverable concept; a goal always aborts on fail). Lower each `DeliverableEntry` → `Check { verify: Deliverable, retry }` with gate resolved: `retry_from` step id, or default = the producer's pipeline-step id.
3. Insert checks into the workflow map and position a `StepRun::Check` step: if a pipeline step `run = "check:<id>"` already names it, keep that position; otherwise append a synthetic step at the tail (deterministic order: goals by id, then deliverables by id). Synthetic step `id` = the check id, `input = "${input}"` (unused by checks), `run = StepRun::Check(id)`.
4. Map `LocusConfig` → `Locus`, `GoalPredicateConfig` → `GoalPredicate` (`NativeFn(name)` → `GoalPredicate::NativeFn(NativeFnRef::from(name))` — match how `Deterministic.fn_ref` is built in parse.rs lines 121-135), `OnFailConfig` → `OnFail`, `JudgeConfig` → `JudgeRef`.

> `PipelineRunRef` (tau-pkg) needs a `Check(String)` arm so `run = "check:<id>"` parses. Add it: in tau-pkg `PipelineRunRef` enum (project.rs line 210) add `Check(String)`, and in the `run = "<kind>:<id>"` splitter accept `"check"`. Do this as Step 0 here (with a parse test in tau-pkg) before touching tau-ir. Map `PipelineRunRef::Check` → `StepRun::Check` in parse.rs's pipeline lowering match (lines 178-193).

- [ ] **Step 0: tau-pkg — `PipelineRunRef::Check`**
  - Test (tau-pkg): a pipeline step `run = "check:report"` validates to `PipelineRunRef::Check("report".into())`.
  - Implement the variant + splitter arm. Run, commit `feat(pkg): accept run = "check:<id>" pipeline steps`.

- [ ] **Step 1: Failing test** (tau-ir, `lower/parse.rs` tests or a new `lower/tests`): lower a `ProjectConfig` (built via `UncheckedProjectConfig::validate`) containing the worked-example goal+deliverable and a `gather→writer` pipeline; assert:
```rust
let ir = lower_project(&cfg, &target, &caches).unwrap();
// produces copied
assert_eq!(ir.workflow.agents[&AgentId("writer".into())].produces, vec!["/workspace/report.md".to_string()]);
// two checks present
assert_eq!(ir.workflow.checks.len(), 2);
// checks appended after writer, in order: goal(has_sources) then deliverable(report)
let pipe = ir.workflow.pipeline.as_ref().unwrap();
let tail: Vec<_> = pipe.steps.iter().rev().take(2).map(|s| &s.run).collect();
assert!(matches!(tail[1], StepRun::Check(CheckId(ref s)) if s == "has_sources"));
assert!(matches!(tail[0], StepRun::Check(CheckId(ref s)) if s == "report"));
// gate defaults to producer step
let report = &ir.workflow.checks[&CheckId("report".into())];
assert_eq!(report.retry.gate, PipelineStepId("writer".into()));
assert_eq!(report.retry.on_fail, OnFail::Retry);
```

- [ ] **Step 2: Run, confirm failure** (`... cargo nextest run -p tau-ir --features with-std-adapters lower_`). Lowering ignores goals/deliverables → assertions fail.

- [ ] **Step 3: Implement** the lowering described above in `parse.rs`. Add a helper `fn lower_checks(config, &agents, &mut pipeline) -> Result<BTreeMap<CheckId, Check>, IrError>`. Resolve the producer step id by scanning the pipeline for `StepRun::Agent(producer_agent_id)` (the producer agent was validated in tau-pkg; re-derive it here from `produces` ∋ locus, or — cleaner — have tau-pkg's `DeliverableEntry` carry the resolved `producer: String` and `gate: String` so lowering doesn't re-derive). **Decision:** add `producer: String` and resolved `gate: String` to `DeliverableEntry`, populated in Task 4/5, so lowering is a pure structural copy. Update Tasks 3-5 accordingly (carry the fields; this avoids duplicating producer-resolution logic across crates).

- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(ir): lower goals/deliverables into Check steps`.

---

### Task 13: Typecheck — `StepRun::Check` + `Locus` integrity

**Files:** `crates/tau-ir/src/lower/typecheck.rs` (pipeline checks ~lines 92-159).

- [ ] **Step 1: Failing tests** — (a) a `StepRun::Check("ghost")` with no matching `workflow.checks` entry → `UnknownCheckRef`; (b) a check whose `Locus::Output("later")` references a step that runs after the check → `UnknownCheckLocus`.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** the `StepRun::Check(check_id)` arm in `check_pipeline`: assert `workflow.checks.contains_key(check_id)` else `UnknownCheckRef`; for the check's locus, if `Locus::Output(step_id)`, assert that step id appears in `pipeline.steps` *before* the current index, else `UnknownCheckLocus`. This mirrors the existing `extract_refs` forward-reference logic (lines 131-156).
- [ ] **Step 4: Run, confirm pass** (`... cargo nextest run -p tau-ir --features with-std-adapters`).
- [ ] **Step 5: commit** `feat(ir): typecheck Check step + output-locus integrity`.

---

### Task 14: Canonical round-trip with checks

**Files:** `crates/tau-ir/src/canonical.rs` (test module ~lines 35-67).

- [ ] **Step 1: Failing test** — build a small `IrModule` with one goal check and one deliverable check, `to_canonical_bytes` → `serde_json::from_slice::<IrModule>` → assert structural equality; also assert `to_canonical_bytes` is byte-stable across two calls.
- [ ] **Step 2: Run, confirm failure** (only if a helper is missing — likely passes once types derive `Serialize`/`Deserialize`; if it passes immediately, that is acceptable for this characterization test, note it).
- [ ] **Step 3:** No impl needed beyond derives; if a constructor is missing add it.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `test(ir): canonical round-trip covers checks`.

---

## PHASE C — runtime evaluation (`tau-runtime-core`)

### Task 15: `ArtifactReader` port + defaulted dispatcher method

**Files:**
- Create: `crates/tau-runtime-core/src/interpreter/artifact.rs`
- Modify: `crates/tau-runtime-core/src/interpreter/mod.rs` (`pub mod artifact;`)
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` (add defaulted method ~after line 84)

- [ ] **Step 1: Failing test** (artifact.rs test module): construct the `InMemoryArtifactReader` mock, seed `/x` → `b"hi"`, assert `read_path("/x") == Some(b"hi")` and `read_path("/y") == None`.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement**:
```rust
//! Reading produced artifacts (files / named outputs) for check evaluation.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::RuntimeError;

/// Reads filesystem artifacts so checks can inspect produced content.
/// Host-implemented (`std::fs`); `no_std` core stays I/O-free.
pub trait ArtifactReader: Send + Sync {
    /// Read a path's bytes. `Ok(None)` means the path does not exist.
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, RuntimeError>;
}

/// In-memory reader for tests.
#[derive(Debug, Default, Clone)]
pub struct InMemoryArtifactReader {
    files: BTreeMap<String, Vec<u8>>,
}

impl InMemoryArtifactReader {
    /// Empty reader.
    pub fn new() -> Self { Self { files: BTreeMap::new() } }
    /// Seed a path with bytes (builder).
    pub fn with_file(mut self, path: &str, bytes: &[u8]) -> Self {
        self.files.insert(String::from(path), bytes.to_vec());
        self
    }
}

impl ArtifactReader for InMemoryArtifactReader {
    fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.files.get(path).cloned())
    }
}
```
Add to `ToolDispatcher` (mirroring `clock()`):
```rust
    /// Optional reader for produced artifacts (checks). Default: none.
    fn artifact_reader(&self) -> Option<Arc<dyn crate::interpreter::artifact::ArtifactReader>> {
        None
    }
```
- [ ] **Step 4: Run, confirm pass** (`... cargo nextest run -p tau-runtime-core artifact`).
- [ ] **Step 5: commit** `feat(runtime): ArtifactReader port + defaulted dispatcher method`.

---

### Task 16: `RuntimeError::CheckFailed` + vocabulary constants + byte-stability guard

**Files:** `crates/tau-runtime-core/src/error.rs`; `crates/tau-runtime-core/src/vocabulary.rs` (after line 103); `crates/tau-ir-conformance/tests/conformance.rs` (assert existing fixture 08 bytes unchanged — or a tau-ir canonical test).

- [ ] **Step 1: Failing test** — (a) construct `RuntimeError::CheckFailed { .. }` and assert its `Display`; (b) assert the new vocabulary constants equal their literals; (c) a drift test: lower fixture 08's `workflow.toml` and assert `to_canonical_bytes` is byte-identical to the committed `expected` (if 08 stores canonical bytes; otherwise assert the `checks` map is empty + version is `v1.2.0` without changing 08's report).
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** error variant:
```rust
    /// A postcondition check failed terminally (abort, or after max attempts).
    #[error("check '{id}' ({kind}) failed after attempt {attempt}: {rationale}")]
    CheckFailed { id: String, kind: String, rationale: String, attempt: u32 },
```
Vocabulary:
```rust
/// Span wrapping a single check evaluation.
pub const SPAN_PIPELINE_CHECK: &str = "pipeline.check";
/// Event: a check produced a verdict.
pub const EV_CHECK_EVALUATED: &str = "check.evaluated";
/// Event: a failed check rewound to its gate.
pub const EV_CHECK_RETRY: &str = "check.retry";
/// Built-in goal predicate fn names (host-registered in the DeterministicRegistry).
pub const FN_BUILTIN_EXISTS: &str = "__tau::goal::exists";
pub const FN_BUILTIN_NON_EMPTY: &str = "__tau::goal::non_empty";
pub const FN_BUILTIN_EQUALS: &str = "__tau::goal::equals";
pub const FN_BUILTIN_MATCHES: &str = "__tau::goal::matches";
pub const FN_BUILTIN_MIN_COUNT: &str = "__tau::goal::min_count";
pub const FN_BUILTIN_SCHEMA_VALID: &str = "__tau::goal::schema_valid";
```
(Add doc comments per `deny(missing_docs)` if it applies to this crate — match the file's existing style.)
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(runtime): CheckFailed error + check vocabulary; assert byte-stability`.

---

### Task 17: `evaluate_goal`

**Files:** Create `crates/tau-runtime-core/src/interpreter/check.rs`; `mod.rs` (`pub mod check;`).

`evaluate_goal` resolves the locus to `(present: bool, content: Option<String>)`, builds the predicate args, and invokes the predicate fn through the `DeterministicRegistry`. Menu predicates map to the `FN_BUILTIN_*` names; `NativeFn(fn_ref)` uses `fn_ref.name`. Args contract:
```json
{ "present": true, "content": "<text>", "pattern": "...", "equals": "...", "min_count": 3, "schema": {...} }
```
The fn returns either a bare bool or `{ "met": bool, "rationale": "..." }`. `evaluate_goal` normalizes to `CheckVerdict`.

- [ ] **Step 1: Failing test** (check.rs tests) using a hand-rolled registry that implements `DeterministicRegistry` and answers `FN_BUILTIN_MATCHES` by regex-free substring (test-only) — or simpler, answers `__tau::goal::non_empty` → `json!(args["present"].as_bool() && !args["content"].as_str().unwrap_or("").is_empty())`:
```rust
#[test]
fn goal_non_empty_passes_on_present_content() {
    let reg = TestRegistry; // returns met = present && content non-empty
    let store = OutputStore::new();
    let reader = InMemoryArtifactReader::new().with_file("/r.md", b"hello");
    let verdict = evaluate_goal(
        &Locus::Path("/r.md".into()),
        &GoalPredicate::NonEmpty,
        &store, Some(&reader as &dyn ArtifactReader), &reg,
    ).unwrap();
    assert!(verdict.met);
}

#[test]
fn goal_non_empty_fails_when_absent() {
    let reg = TestRegistry;
    let store = OutputStore::new();
    let reader = InMemoryArtifactReader::new();
    let verdict = evaluate_goal(
        &Locus::Path("/missing".into()),
        &GoalPredicate::NonEmpty,
        &store, Some(&reader as &dyn ArtifactReader), &reg,
    ).unwrap();
    assert!(!verdict.met);
}
```

- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement**:
```rust
/// Outcome of evaluating a check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckVerdict {
    /// Whether the postcondition held.
    pub met: bool,
    /// Why (load-bearing — fed into the retry loop).
    pub rationale: String,
}

/// Resolve a locus to `(present, content)`.
fn resolve_locus(
    locus: &Locus,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
) -> Result<(bool, Option<String>), RuntimeError> {
    match locus {
        Locus::Output(step_id) => Ok(match store.get(&step_id.0) {
            Some(v) => (true, Some(value_to_string(v))),
            None => (false, None),
        }),
        Locus::Path(p) => {
            let r = reader.ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!("check needs an artifact reader to read {p}"),
            })?;
            Ok(match r.read_path(p)? {
                Some(bytes) => (true, Some(String::from_utf8_lossy(&bytes).into_owned())),
                None => (false, None),
            })
        }
    }
}

/// Map a `GoalPredicate` to `(fn_name, extra_args)`.
fn predicate_call(p: &GoalPredicate) -> (&str, Value) { /* match: Matches(x) -> (FN_BUILTIN_MATCHES, json!({"pattern": x})), etc. NativeFn(f) -> (f.name.as_str(), json!({})) */ }

/// Evaluate a goal predicate via the deterministic registry.
pub fn evaluate_goal(
    evaluates: &Locus,
    predicate: &GoalPredicate,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
    registry: &dyn DeterministicRegistry,
) -> Result<CheckVerdict, RuntimeError> {
    let (present, content) = resolve_locus(evaluates, store, reader)?;
    let (fn_name, mut args) = predicate_call(predicate);
    // merge present/content into args object
    if let Value::Object(ref mut m) = args {
        m.insert("present".into(), Value::Bool(present));
        m.insert("content".into(), content.map(Value::String).unwrap_or(Value::Null));
    }
    let raw = registry.invoke(fn_name, &args)?;
    Ok(normalize_verdict(raw, predicate))
}
```
`normalize_verdict` accepts `Value::Bool` or `{met, rationale}` and synthesizes a default rationale (e.g. `"goal predicate {p:?} returned false"`). `value_to_string` mirrors `OutputStore::template_map`'s `Value::String` passthrough / `to_string()` fallback.

- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(runtime): evaluate_goal via DeterministicRegistry`.

---

### Task 18: `evaluate_deliverable` (existence floor + LLM judge)

**Files:** `crates/tau-runtime-core/src/interpreter/check.rs`.

Flow: (1) resolve locus → `(present, content)`; if not present → `CheckVerdict { met: false, rationale: "deliverable <locus> was not produced" }` (existence floor, no LLM spent). (2) Build the judge agent: `JudgeRef::Builtin { model }` → synthesize an `Agent` with the canonical judge prompt embedding `must_satisfy` (and record `model` into `Agent.model`, a runtime no-op per Decision 6); `JudgeRef::Agent(id)` → clone `module.workflow.agents[id]`. (3) `run_agent(module, &judge, dispatcher, vec![user_message(judge_input)])` where `judge_input` = the artifact content. (4) Parse `last_assistant_text` as `{met, rationale}` (serde, fallback `{met:false, rationale: raw}`).

- [ ] **Step 1: Failing test** using `MockLlmBackend` scripted to return `{"met": true, "rationale": "good"}` — wire it through a test dispatcher (copy the `TestDispatcher` pattern from agent_loop.rs tests, lines ~551-640). Assert `evaluate_deliverable` with a present artifact + builtin judge returns `met == true`. Second test: absent artifact → `met == false` and the backend was never invoked (`backend.invocation_count() == 0`).
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement**:
```rust
/// Canonical built-in judge prompt.
fn builtin_judge_prompt(must_satisfy: &str) -> String {
    alloc::format!(
        "You are a strict reviewer. Judge whether the artifact below satisfies this \
         criterion:\n\n{must_satisfy}\n\nReply ONLY with JSON: {{\"met\": bool, \"rationale\": string}}."
    )
}

/// Evaluate a deliverable: existence floor, then LLM judge.
pub async fn evaluate_deliverable<D>(
    module: Arc<IrModule>,
    locus: &Locus,
    must_satisfy: &str,
    judge: &JudgeRef,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
    dispatcher: Arc<D>,
) -> Result<CheckVerdict, RuntimeError>
where D: ToolDispatcher + Send + Sync + 'static {
    let (present, content) = resolve_locus(locus, store, reader)?;
    if !present {
        return Ok(CheckVerdict { met: false,
            rationale: alloc::format!("deliverable {locus:?} was not produced") });
    }
    let artifact = content.unwrap_or_default();
    let judge_agent: Agent = match judge {
        JudgeRef::Agent(id) => module.workflow.agents.get(id)
            .ok_or_else(|| RuntimeError::AgentNotFound { agent: id.0.clone() })?.clone(),
        JudgeRef::Builtin { model } => Agent {
            id: AgentId(alloc::format!("__judge")),
            prompt: builtin_judge_prompt(must_satisfy),
            model: model.clone().unwrap_or_default(),
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            produces: alloc::vec::Vec::new(),
            budget: AgentBudget { max_turns: Some(1), max_tokens: None },
        },
    };
    let outcome = Box::pin(run_agent(module.clone(), &judge_agent, dispatcher,
        alloc::vec![user_message(&artifact)])).await?;
    let text = last_assistant_text(&outcome);
    Ok(parse_verdict(&text))
}

/// Parse a judge reply into a verdict; fall back to met=false on non-JSON.
fn parse_verdict(text: &str) -> CheckVerdict {
    #[derive(serde::Deserialize)]
    struct Raw { met: bool, #[serde(default)] rationale: String }
    match serde_json::from_str::<Raw>(text.trim()) {
        Ok(r) => CheckVerdict { met: r.met, rationale: r.rationale },
        Err(_) => CheckVerdict { met: false, rationale: String::from(text) },
    }
}
```
Move `user_message` from pipeline.rs into a shared spot (e.g. make it `pub(crate)` in pipeline.rs and `use` it here, or relocate to `check.rs`). Import `Agent`, `AgentId`, `AgentBudget`, `run_agent`, `last_assistant_text`.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(runtime): evaluate_deliverable with existence floor + LLM judge`.

---

### Task 19: `run_pipeline` — index loop + `StepRun::Check` dispatch (no retry yet)

**Files:** `crates/tau-runtime-core/src/interpreter/pipeline.rs`.

Convert the `for step in &pipeline.steps` loop (line 56) into a `while i < pipeline.steps.len()` index loop. Add a `StepRun::Check(check_id)` arm that looks up `module.workflow.checks[check_id]`, evaluates via Task 17/18, emits `EV_CHECK_EVALUATED`, and — for now — on failure returns `RuntimeError::CheckFailed` (abort behavior; retry comes in Task 20). Resolve the artifact reader once via `dispatcher.artifact_reader()` and the registry via `dispatcher.deterministic_registry()`.

- [ ] **Step 1: Failing test** (`tau-runtime-core/tests/` — copy the harness from `pipeline_executor.rs`): a pipeline `[writer-agent, check:goal(non_empty on steps.writer.output)]`; a `TestDispatcher` returning an `InMemoryArtifactReader` + a registry answering `FN_BUILTIN_NON_EMPTY`. Assert the run completes and the store holds the writer output. Second test: a goal that fails → `run_pipeline` returns `Err(RuntimeError::CheckFailed { id, .. })`.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** the index loop + Check arm:
```rust
StepRun::Check(check_id) => {
    let check = module.workflow.checks.get(check_id).ok_or_else(|| RuntimeError::Internal {
        message: alloc::format!("unknown check {}", check_id.0),
    })?;
    let (verdict, kind) = match &check.verify {
        CheckVerify::Goal { evaluates, predicate } => {
            let reg = dispatcher.deterministic_registry().ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!("check {} needs a deterministic registry", check_id.0) })?;
            let reader = dispatcher.artifact_reader();
            (evaluate_goal(evaluates, predicate, &store, reader.as_deref(), reg.as_ref())?, "goal")
        }
        CheckVerify::Deliverable { locus, must_satisfy, judge } => {
            let reader = dispatcher.artifact_reader();
            (Box::pin(evaluate_deliverable(module.clone(), locus, must_satisfy, judge,
                &store, reader.as_deref(), dispatcher.clone())).await?, "deliverable")
        }
    };
    let attempt = 1u32; // Task 20 makes this real
    tracing::info!(parent: &step_span, name = EV_CHECK_EVALUATED, id = check_id.0.as_str(),
                   kind = kind, verdict = if verdict.met {"pass"} else {"fail"}, attempt = attempt);
    if !verdict.met {
        return Err(RuntimeError::CheckFailed { id: check_id.0.clone(),
            kind: String::from(kind), rationale: verdict.rationale, attempt });
    }
    // checks store no output
    i += 1;
    continue;
}
```
Keep Agent/Tool/Deterministic arms storing into `store` and advancing `i`. (`reader.as_deref()` turns `Option<Arc<dyn ArtifactReader>>` into `Option<&dyn ArtifactReader>` — adjust to `.as_ref().map(|a| a.as_ref())` if `as_deref` doesn't apply to `Arc<dyn _>`.)
- [ ] **Step 4: Run, confirm pass.** Re-run the *existing* `pipeline_executor.rs` suite to prove the loop rewrite is behavior-preserving.
- [ ] **Step 5: commit** `feat(runtime): run_pipeline evaluates Check steps (abort path)`.

---

### Task 20: `run_pipeline` — rewind-to-gate retry + feedback injection

**Files:** `crates/tau-runtime-core/src/interpreter/pipeline.rs`.

Replace the Task-19 abort-on-fail Check logic with the full retry loop:
- Track `attempts: BTreeMap<String, u32>` keyed by check id, and `feedback: Option<String>`.
- On Check eval: `attempt = attempts[id] + 1; attempts[id] = attempt`. Emit `EV_CHECK_EVALUATED` with the real `attempt`.
- Pass → `feedback = None; i += 1`.
- Fail → if `on_fail == Abort` **or** `attempt >= max_attempts` → return `CheckFailed { attempt, rationale, .. }`. Else emit `EV_CHECK_RETRY { rewind_to = gate, next_attempt = attempt + 1 }`, set `feedback = Some(rationale)`, set `i = gate_index` (precomputed id→index map), `continue`.
- Agent steps: when `feedback` is `Some`, prepend a prior turn so the agent sees the "why": `initial = vec![ user_message(&format!("Previous attempt rejected: {fb}")), user_message(&rendered) ]` (the kernel's `split_history` treats all-but-last as history, last as the live turn — see agent_loop.rs:506-519).

- [ ] **Step 1: Failing test** — scripted `MockLlmBackend` (or a sequencing test backend) where the `writer` agent returns a bad artifact on call 1 and a good one on call 2, and the deliverable judge returns `met:false` then `met:true`. Assert: the run **completes**, the judge saw two attempts, and (via a tracing capture layer, as in the existing trace test at `pipeline_executor.rs:427`) `check.retry` was emitted once with `next_attempt = 2`. Second test: `max_attempts = 1` + always-failing judge → `Err(CheckFailed { attempt: 1, .. })` and **no** `check.retry` event.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** the retry loop + feedback injection + the `gate_index` map (`BTreeMap<&str, usize>` over `pipeline.steps`).
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(runtime): rewind-to-gate retry with rationale feedback`.

---

### Task 21: Budget-cap interplay (assertion-only)

**Files:** `crates/tau-runtime-core/tests/`.

The `AgentBudget.max_turns` cap is already enforced inside `run_agent`; a producer that exhausts its budget returns `Failed`, which the Agent step arm already turns into a `RuntimeError`. Prove the retry loop honors it (a producer with `max_turns` exhausted aborts even though `max_attempts` would allow more).

- [ ] **Step 1: Failing/characterization test** — producer agent with `budget.max_turns = Some(0)` inside a retry-enabled pipeline → `run_pipeline` returns an error (budget abort) and does **not** spin to `max_attempts`. If it already passes given Task 20, mark it a characterization test and note that.
- [ ] **Step 2: Run.** If green immediately, document why (budget enforced upstream). If red, fix the Agent step arm to surface budget `Failed` as an error before re-checking.
- [ ] **Step 3-5: adjust if needed, commit** `test(runtime): budget cap is authoritative below max_attempts`.

---

## PHASE D — host wiring

### Task 22: `StdFsArtifactReader` + register built-in predicate fns + dispatcher wiring

**Files:** the host dispatcher crate — locate with `grep -rn "impl ToolDispatcher for\|fn deterministic_registry" crates/tau-cli crates/tau-runtime-tokio`. Likely `crates/tau-cli/src/.../ForwardingDispatcher`. Also the host `DeterministicRegistry` impl.

Two host additions:
1. `StdFsArtifactReader` implementing `ArtifactReader` via `std::fs::read` (`Ok(None)` on `NotFound`, error otherwise). Return it from the dispatcher's `artifact_reader()`.
2. Register the six built-in predicate fns in the host registry, keyed by the `FN_BUILTIN_*` constants. Each takes `args = {present, content, ...}` and returns `Value::Bool` (or `{met, rationale}`):
   - `exists` → `present`.
   - `non_empty` → `present && !content.is_empty()`.
   - `equals` → `content == args["equals"]`.
   - `matches` → `regex::Regex::new(pattern)?.is_match(content)` (build-time already proved it compiles, but handle the `Err` as `met:false` defensively).
   - `min_count` → count of non-empty lines `>= min_count` (document the "items = lines" rule).
   - `schema_valid` → validate `content`-as-JSON against `schema` (use the JSON-schema validator already in the workspace if present; else parse-only + presence-of-required-keys, and note the limitation).

- [ ] **Step 1: Failing tests** (host crate) — one per predicate: e.g. `matches` true/false, `non_empty` absent→false, `min_count` boundary. Drive the registry directly: `registry.invoke(FN_BUILTIN_MATCHES, &json!({"present":true,"content":"## Sources","pattern":"(?m)^## Sources"}))` → `json!(true)`.
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** the reader + the six fns + wire `artifact_reader()`.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(cli): std-fs artifact reader + built-in goal predicate fns`.

---

## PHASE E — `.ts` authoring parity

### Task 23: `goals` / `deliverables` / `produces` in `tau-ts-extract`

**Files:** `crates/tau-ts-extract/src/factory.rs` (`Factory` enum + recognizer), `crates/tau-ts-extract/src/lower.rs` (intermediate structs + extraction + `build_toml`).

Add `produces: [...]` extraction onto the agent object, and `goals([...])` / `deliverables([...])` factories that emit `[goals.*]` / `[deliverables.*]` TOML. The TOML-bridge trick (β.8): TS lowers to the same TOML the parser already validates, so parity is byte-equality of the lowered TOML's *canonical IR*.

- [ ] **Step 1: Failing test** (`tau-ts-extract/tests/`, copy `fan_monitor_conformance.rs`): a `.ts` authoring the worked example via `agent({..., produces:["/workspace/report.md"]})`, `goals([{id:"has_sources", evaluates:"/workspace/report.md", check:"matches", pattern:"(?m)^## Sources"}])`, `deliverables([{id:"report", path:"/workspace/report.md", mustSatisfy:"...", onFail:"retry", maxAttempts:3, retryFrom:"writer"}])`. Assert `lower_project(ts) == lower_project(equivalent_toml)` (canonical bytes equal).
- [ ] **Step 2: Run, confirm failure.**
- [ ] **Step 3: Implement** the factory variants + `IrGoal`/`IrDeliverable` structs + extraction fns (copy `extract_pipeline_steps`, lines 352-410) + `build_toml` sections (copy lines 236-244). Map camelCase TS keys (`mustSatisfy`/`onFail`/`maxAttempts`/`retryFrom`/`judgeModel`) to snake_case TOML.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `feat(ts-extract): goals/deliverables/produces authoring parity`.

---

## PHASE F — conformance fixtures

### Task 24: Fixture 09 — happy path (goal pass + deliverable pass)

**Files:** `crates/tau-ir-conformance/fixtures/09_deliverables_happy/{workflow.toml,mock_llm.jsonl,expected_report.json}`; register in `tests/conformance.rs`.

Copy fixture 08's structure. `workflow.toml` = the spec's worked example with an explicit `[[pipeline.steps]]` (gather→writer). `mock_llm.jsonl`: writer writes the report (the fixture harness's tool path) and the judge returns `{"met":true,"rationale":"ok"}`. `expected_report.json`: `run_outcome_kind = "Completed"`. The fixture harness must provide an `InMemoryArtifactReader` seeded with the report content (or the writer's WriteFile must land somewhere the reader sees) — check how 08 wires its dispatcher; extend the conformance dispatcher to return an `artifact_reader()` and register the built-in predicate fns (reuse Task 22's logic, or a conformance-local copy).

- [ ] **Step 1:** Author the fixture files.
- [ ] **Step 2: Run** `... cargo nextest run -p tau-ir-conformance fixture_09` (or the harness's fixture-discovery test). Confirm it fails (unregistered / wrong expected).
- [ ] **Step 3:** Wire the conformance dispatcher (`src/lib.rs`) to provide the reader + predicates; fix expected.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `test(conformance): fixture 09 deliverables happy path`.

---

### Task 25: Fixture 10 — deliverable retry converges

**Files:** `fixtures/10_deliverable_retry/...`.

`on_fail="retry"`, `max_attempts=3`, `retry_from="writer"`. `mock_llm.jsonl`: judge returns `met:false` (attempt 1) then `met:true` (attempt 2); writer produces a better artifact on the second pass. `expected_report.json`: `Completed`, and (if the harness records events) one `check.retry`.

- [ ] **Steps 1-5** as Task 24, asserting convergence. commit `test(conformance): fixture 10 deliverable retry converges`.

---

### Task 26: Fixture 11 — build refused (no producer)

**Files:** `fixtures/11_deliverable_no_producer/...`.

A deliverable whose path no agent `produces`. `expected_report.json` uses the `build_refused` field (see `ConformanceReport`, lib.rs:87-154) with the `DeliverableNoProducer` message. This proves the build-time gate fires through the full lower path.

- [ ] **Steps 1-5** — assert `lower_project` (or the harness's build step) returns the refusal. commit `test(conformance): fixture 11 build refused on missing producer`.

---

## PHASE G — integration + docs

### Task 27: `tau check` surfaces the new build-time errors

**Files:** `crates/tau-cli/tests/` (copy an existing `cmd_check` integration test).

`tau check` already runs `validate()` + `lower_project`, so the new `ProjectConfigError`s should surface with the right exit code automatically. Prove it end-to-end.

- [ ] **Step 1: Failing test** — write a temp project with a deliverable lacking a producer; run `tau check`; assert non-zero exit + the `has no producer` message in stderr/JSON.
- [ ] **Step 2: Run, confirm** it fails only if wiring is missing; if it passes, it's a characterization test (note that).
- [ ] **Step 3:** If `tau check` swallows the error, thread it through the check renderer.
- [ ] **Step 4: Run, confirm pass.**
- [ ] **Step 5: commit** `test(cli): tau check reports deliverable build-time errors`.

---

### Task 28: Docs + ADR

**Files:** `docs/` (a how-to page under the existing Diátaxis tree + `SUMMARY.md` entry), `docs/explanation/adr/ADR-00xx-deliverables-and-goals.md` (next free number; check the latest ADR — ADR-0043 per recent commits, so use **0044**), `SUMMARY.md`.

Per repo DOCS RULES: every page in `SUMMARY.md`; build locally before the PR.

- [ ] **Step 1:** Write the how-to (`goal`/`deliverable` authoring, the predicate menu, judge resolution, retry semantics, the worked example) and ADR-0044 (record: Check-not-Node modeling, checks-as-pipeline-steps, semantic-checks-in-tau-pkg, predicates-as-deterministic-fns, the **`judge_model` runtime no-op honest limit**, and the deferred `output_schema` judge warning since agents lack `output_schema`).
- [ ] **Step 2:** Add both to `SUMMARY.md`.
- [ ] **Step 3: Build the book:**
```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book
```
Expected: only `[INFO]` lines.
- [ ] **Step 4:** N/A.
- [ ] **Step 5: commit** `docs: deliverables & goals how-to + ADR-0044`.

---

## Final verification (before opening the PR)

- [ ] Full suites green:
  `... cargo nextest run -p tau-pkg && ... -p tau-ir && ... -p tau-runtime-core && ... -p tau-ts-extract && ... -p tau-ir-conformance && ... -p tau-cli`
- [ ] Doctests: `... cargo test -p tau-ir --doc && ... -p tau-pkg --doc`
- [ ] Clippy clean on every touched crate: `... cargo clippy -p <crate> -- -D warnings`
- [ ] `cargo fmt --check` clean.
- [ ] Existing fixtures (01-08) still pass byte-stable — proves the v1.1.0→v1.2.0 bump is additive.
- [ ] Manual smoke: build the worked-example project and run it through `tau run` with a real/mock backend; confirm a `check.evaluated` line lands in the JSONL trace.

---

## Self-review notes (spec coverage)

- **`goal` menu + native fn** → Tasks 2, 7, 12, 17, 22. **`deliverable` 3 layers** → Tasks 3, 18 (existence floor + judge), 4 (build-time producible). **Producer binding** → Tasks 1, 4. **Judge easy/tune/power** → Tasks 3, 6, 18 (tune = `judge_model`, a v1 no-op per Decision 6). **Verdict `{met,rationale}`** → Task 18. **abort vs retry, gate ≤ producer, span-has-LLM** → Tasks 5, 20. **feedback "why"** → Task 20. **budget bound** → Task 21. **placement/checkpoints** → Decision 2 + Task 12. **trace events** → Tasks 16, 19, 20. **IR representation + ripple** → Tasks 7-14. **build-time refusal** → Tasks 4-6, 26, 27. **out-of-scope items** (legacy runner, external/provenance loci, score thresholds, multi-gate, project-level default judge model) → untouched.
- **Known honest limits carried into the ADR:** `judge_model` is a runtime no-op (Decision 6); `output_schema` judge-compat warning deferred (agents have no `output_schema`); `min_count`/`schema_valid` semantics are pinned in Task 22's docs.
```
