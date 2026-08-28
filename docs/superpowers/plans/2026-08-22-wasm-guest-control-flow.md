# Wasm Guest Control-Flow (#621) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The wasm guest executes Branch/Parallel/Loop pipelines via the existing `run_pipeline` interpreter; `any-wasi-strict` flips to `supported_features = [Branch, Parallel, Loop]`; the north-star fixture gains a wasm execution leg with the same terminal outcome as the dev leg.

**Architecture:** In-guest interpretation (ADR-0068): the guest calls the same `no_std` `run_pipeline` the native path uses. The only new machinery is a `no_std` goal-predicate registry (5 predicates, `matches` via `regex-automata`), a wasm-only build-time fn-availability gate, and a shared "last leaf step" helper. No WIT world change.

**Tech Stack:** Rust workspace; `regex-automata 0.4` (`default-features = false`); wasm32-wasip2; `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-22-wasm-guest-control-flow-design.md` (decision record: `docs/decisions/0068-wasm-guest-control-flow.md`)

## Global Constraints

- Every cargo command: `timeout <300|180> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo … -p <crate>` (CLAUDE.md CARGO RULES; subagents use `target/agent-<role>`). Prefer `cargo nextest run` over `cargo test` (except `--doc`).
- Branch: `feat/621-wasm-guest-control-flow` (PR-1). PR-2/PR-3 branch from the previous PR's branch if it hasn't merged yet, or from `main` after it merges; each PR: `gh pr create --base main` then `gh pr merge <N> --squash --delete-branch --auto`.
- Conventional commits, imperative, scoped. Commit with `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit …`.
- `#![no_std]` + `#![forbid(unsafe_code)]` stays in force in `tau-native-tools` and `tau-wasm-guest`. Never add a `std`-requiring dependency to either.
- Wasm supported set is exactly `{Branch, Parallel, Loop}`. Suspend, Dynamic, `schema_valid`, and `NativeFn` predicates stay build-time refused for wasm.
- Docs edits (PR-3) require a clean `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`, then `rm -rf docs/book`.

## PR map

| PR | Tasks | Deliverable |
|---|---|---|
| PR-1 | 1–3 | no_std goal predicates + CLI delegation + shared last-leaf helper (pure refactor, native behavior unchanged) |
| PR-2 | 4–6 | guest executes pipelines; wasm fn-availability gate; linear-pipeline wasm e2e |
| PR-3 | 7–9 | registry flip; feature-fit/registry test updates; north-star suspend-twin refusal + wasm execution leg |

---

### Task 1: no_std goal predicates in `tau-native-tools`

**Files:**
- Modify: `crates/tau-native-tools/Cargo.toml`
- Modify: `Cargo.toml` (workspace root, `[workspace.dependencies]`)
- Create: `crates/tau-native-tools/src/goal_predicates.rs`
- Modify: `crates/tau-native-tools/src/lib.rs` (add `pub mod goal_predicates;` behind the feature)

**Interfaces:**
- Consumes: nothing new.
- Produces (used by Tasks 2 and 5):
  ```rust
  // crate tau_native_tools, feature "goal-predicates"
  pub mod goal_predicates {
      pub const FN_EXISTS: &str = "__tau::goal::exists";
      pub const FN_NON_EMPTY: &str = "__tau::goal::non_empty";
      pub const FN_EQUALS: &str = "__tau::goal::equals";
      pub const FN_MATCHES: &str = "__tau::goal::matches";
      pub const FN_MIN_COUNT: &str = "__tau::goal::min_count";
      /// The five predicate fn names answerable without std.
      pub const SUPPORTED: &[&str; 5];
      /// `None` = this crate does not answer `fn_name` (schema_valid, unknown).
      /// `Some(Err(msg))` = malformed args (missing "pattern"/"min_count").
      pub fn invoke(fn_name: &str, args: &serde_json::Value) -> Option<Result<serde_json::Value, alloc::string::String>>;
  }
  ```

- [ ] **Step 1: Add the workspace dep and the feature**

In root `Cargo.toml` `[workspace.dependencies]` (next to `regex = "1"`, line ~138):

```toml
regex-automata = { version = "0.4.18", default-features = false }
```

In `crates/tau-native-tools/Cargo.toml`:

```toml
[dependencies]
serde_json = { workspace = true, default-features = false, features = ["alloc"] }
regex-automata = { workspace = true, optional = true, features = ["alloc", "meta", "syntax", "nfa-pikevm", "unicode-case", "unicode-perl"] }

[features]
goal-predicates = ["dep:regex-automata"]
```

Feature-list note: this is the starting set for a no_std `meta::Regex` with `(?i)` support. If `cargo build` in Step 4 reports a missing/unknown feature, adjust to the minimal set that compiles AND passes Step 3's tests, and record the final set in a Cargo.toml comment. Do not enable `std`.

- [ ] **Step 2: Write the failing tests**

Create `crates/tau-native-tools/src/goal_predicates.rs` with a `#[cfg(test)] mod tests` first (the module body in Step 3 makes them compile). Tests mirror the CLI registry's behavior byte-for-byte — the args contract is `{ "present": bool, "content": string|null, ...predicate_params }`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exists_returns_present() {
        assert_eq!(invoke(FN_EXISTS, &json!({"present": true})), Some(Ok(json!(true))));
        assert_eq!(invoke(FN_EXISTS, &json!({"present": false})), Some(Ok(json!(false))));
    }

    #[test]
    fn non_empty_requires_present_and_nonblank() {
        assert_eq!(invoke(FN_NON_EMPTY, &json!({"present": true, "content": "hi"})), Some(Ok(json!(true))));
        assert_eq!(invoke(FN_NON_EMPTY, &json!({"present": true, "content": "  "})), Some(Ok(json!(false))));
        assert_eq!(invoke(FN_NON_EMPTY, &json!({"present": false, "content": "hi"})), Some(Ok(json!(false))));
    }

    #[test]
    fn equals_compares_literal() {
        assert_eq!(invoke(FN_EQUALS, &json!({"present": true, "content": "a", "equals": "a"})), Some(Ok(json!(true))));
        assert_eq!(invoke(FN_EQUALS, &json!({"present": true, "content": "a", "equals": "b"})), Some(Ok(json!(false))));
    }

    #[test]
    fn matches_supports_case_insensitive_regex() {
        // The north-star fixture's exact patterns.
        assert_eq!(invoke(FN_MATCHES, &json!({"present": true, "content": "URGENT: fan", "pattern": "(?i)urgent"})), Some(Ok(json!(true))));
        assert_eq!(invoke(FN_MATCHES, &json!({"present": true, "content": "draft APPROVED", "pattern": "APPROVED"})), Some(Ok(json!(true))));
        assert_eq!(invoke(FN_MATCHES, &json!({"present": true, "content": "routine", "pattern": "(?i)urgent"})), Some(Ok(json!(false))));
        assert_eq!(invoke(FN_MATCHES, &json!({"present": false, "content": "URGENT", "pattern": "URGENT"})), Some(Ok(json!(false))));
    }

    #[test]
    fn matches_bad_pattern_is_met_false_with_rationale() {
        let got = invoke(FN_MATCHES, &json!({"present": true, "content": "x", "pattern": "("})).unwrap().unwrap();
        assert_eq!(got["met"], json!(false));
        assert!(got["rationale"].as_str().unwrap().contains("regex compile error"));
    }

    #[test]
    fn matches_missing_pattern_is_err() {
        assert!(matches!(invoke(FN_MATCHES, &json!({"present": true, "content": "x"})), Some(Err(_))));
    }

    #[test]
    fn min_count_counts_nonempty_lines() {
        assert_eq!(invoke(FN_MIN_COUNT, &json!({"present": true, "content": "a\n\nb", "min_count": 2})), Some(Ok(json!(true))));
        assert_eq!(invoke(FN_MIN_COUNT, &json!({"present": true, "content": "a", "min_count": 2})), Some(Ok(json!(false))));
    }

    #[test]
    fn schema_valid_and_unknown_are_none() {
        assert_eq!(invoke("__tau::goal::schema_valid", &json!({})), None);
        assert_eq!(invoke("nope", &json!({})), None);
    }
}
```

- [ ] **Step 3: Implement the module**

Port the five bodies **verbatim** from `crates/tau-cli/src/cmd/builtin_registry.rs` (`builtin_exists` … `builtin_min_count`), with two mechanical changes: return type becomes `Result<Value, String>` (the CLI's `RuntimeError::Internal { message }` becomes plain `Err(message)`), and `regex::Regex::new(p)?.is_match(c)` becomes:

```rust
match regex_automata::meta::Regex::new(pattern) {
    Ok(re) => Ok(Value::Bool(re.is_match(content))),
    Err(e) => Ok(serde_json::json!({
        "met": false,
        "rationale": alloc::format!("regex compile error for pattern {pattern:?}: {e}")
    })),
}
```

Top-level dispatcher:

```rust
pub fn invoke(fn_name: &str, args: &Value) -> Option<Result<Value, String>> {
    match fn_name {
        FN_EXISTS => Some(exists(args)),
        FN_NON_EMPTY => Some(non_empty(args)),
        FN_EQUALS => Some(equals(args)),
        FN_MATCHES => Some(matches_(args)),
        FN_MIN_COUNT => Some(min_count(args)),
        _ => None,
    }
}
```

In `lib.rs`: `#[cfg(feature = "goal-predicates")] pub mod goal_predicates;`.

- [ ] **Step 4: Run the tests and the no_std guard**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-native-tools --features goal-predicates
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-native-tools --features goal-predicates --target wasm32-wasip2
```

Expected: tests PASS; wasm32 build succeeds (proves the dependency set stays no_std-clean). Note the `.rlib` size printed by `ls -la target/agent-impl/wasm32-wasip2/debug/*.rlib` in the PR body (spec obligation: measure the regex-automata size cost; the real component delta is measured in Task 6).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tau-native-tools
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(native-tools): no_std goal-predicate registry behind goal-predicates feature (#621)"
```

### Task 2: CLI `BuiltinDeterministicRegistry` delegates to the shared predicates

**Files:**
- Modify: `crates/tau-cli/Cargo.toml` (add `tau-native-tools = { path = "../tau-native-tools", features = ["goal-predicates"] }`)
- Modify: `crates/tau-cli/src/cmd/builtin_registry.rs`

**Interfaces:**
- Consumes: `tau_native_tools::goal_predicates::invoke` (Task 1).
- Produces: unchanged public surface — `BuiltinDeterministicRegistry` still answers all six `FN_BUILTIN_*` names identically.

- [ ] **Step 1: Rewire `DeterministicRegistry::invoke`**

Replace the six-arm match with: try the shared crate first, keep `schema_valid` local:

```rust
impl DeterministicRegistry for BuiltinDeterministicRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        if let Some(result) = tau_native_tools::goal_predicates::invoke(fn_name, args) {
            return result.map_err(|message| RuntimeError::Internal { message });
        }
        match fn_name {
            _ if fn_name == FN_BUILTIN_SCHEMA_VALID => builtin_schema_valid(args),
            other => Err(RuntimeError::Internal {
                message: format!("BuiltinDeterministicRegistry: unknown fn {other:?}"),
            }),
        }
    }
}
```

Delete `builtin_exists`, `builtin_non_empty`, `builtin_equals`, `builtin_matches`, `builtin_min_count` and the now-unused `Regex` import + unused `FN_BUILTIN_*` imports. Keep `builtin_schema_valid` and `StdFsArtifactReader`. Keep every existing `#[cfg(test)]` test in the file UNCHANGED — they now assert the delegation preserves behavior. If a deleted helper is referenced by a test, point the test at `BuiltinDeterministicRegistry.invoke(FN_BUILTIN_…, …)` instead.

- [ ] **Step 2: Run the registry tests + the north-star dev suite**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli builtin_registry
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test north_star_demo
```

Expected: PASS (the north-star dev legs exercise `matches` end-to-end through the new engine).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "refactor(cli): delegate std-free goal predicates to tau-native-tools (#621)"
```

### Task 3: shared `Pipeline::final_leaf_step_id` helper

**Files:**
- Modify: `crates/tau-ir/src/pipeline.rs`
- Modify: `crates/tau-cli/src/cmd/run.rs:609-617`
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs:352-370`

**Interfaces:**
- Produces (used by Task 5):
  ```rust
  impl Pipeline {
      /// Id of the last top-level step that records an output — skips
      /// trailing `Check` and `Suspend` steps. `None` if no step qualifies.
      pub fn final_leaf_step_id(&self) -> Option<&PipelineStepId>;
  }
  ```

- [ ] **Step 1: Write the failing test** (in `pipeline.rs`'s existing `#[cfg(test)]` module; construct steps with `StepRun::Agent`/`StepRun::Check`/`StepRun::Suspend` following the file's existing test constructors)

```rust
#[test]
fn final_leaf_skips_trailing_check_and_suspend() {
    // steps: agent "a", agent "b", check "c", suspend "s" → final leaf is "b"
    // steps: check "c" only → None
}
```

Write the two cases as real constructions using the module's existing test helpers/literals (`PipelineStep { id: PipelineStepId("a".into()), run: StepRun::Agent(AgentId("x".into())), … }` — copy the field shape from neighbouring tests in the file).

- [ ] **Step 2: Run to verify it fails** — `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir final_leaf` → FAIL (method missing).

- [ ] **Step 3: Implement**

```rust
impl Pipeline {
    pub fn final_leaf_step_id(&self) -> Option<&PipelineStepId> {
        self.steps
            .iter()
            .rev()
            .find(|s| !matches!(s.run, StepRun::Check(_) | StepRun::Suspend { .. }))
            .map(|s| &s.id)
    }
}
```

- [ ] **Step 4: Adopt at both CLI call sites**

- `run.rs:609-617`: replace the inline `.rev().find(…)` with `pipeline.final_leaf_step_id()` (`Some(id) => id.0.clone(), None => return None`). Behavior identical (it already skipped `Check|Suspend`).
- `ir_dispatcher.rs:361-370`: replace the inline `.rev().find(|s| !matches!(s.run, StepRun::Check(_)))` with the helper. NOTE in the commit message: this also starts skipping trailing `Suspend` steps on the bundle path — a latent-drift fix; a trailing `Suspend` records no output, so the old code would have produced the "recorded no output" invariant error.

- [ ] **Step 5: Run tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

Expected: PASS.

- [ ] **Step 6: Commit, push, open PR-1, enrol auto-merge**

```bash
git add crates/tau-ir crates/tau-cli
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "refactor(ir): shared Pipeline::final_leaf_step_id; fix Suspend-skip drift on bundle path (#621)"
git push -u origin feat/621-wasm-guest-control-flow
gh pr create --base main --title "refactor: #621 PR-1 — no_std goal predicates + shared last-leaf helper" --body "<summary; cite ADR-0068; rlib size note>"
gh pr merge <N> --squash --delete-branch --auto
```

### Task 4: wasm-only build-time fn-availability gate (`predicate_fit`)

**Files:**
- Create: `crates/tau-ir-lower/src/lower/predicate_fit.rs`
- Modify: `crates/tau-ir-lower/src/lower/mod.rs:124` (call after `feature_fit::check`)
- Modify: `crates/tau-ir-lower/src/error.rs:58` area (new variant)
- Modify: `crates/tau-cli/src/cmd/build_wasm.rs:60-80` area (render arm)

**Interfaces:**
- Consumes: `Parsed` (same as `feature_fit`), `tau_ir` types: `StepRun`, `Condition`, `GoalPredicate`, `CheckVerify`, `Deterministic`.
- Produces:
  ```rust
  // error.rs — mirror FeatureUnsupported's derive/display style exactly:
  WasmFnUnavailable { fn_names: Vec<String>, target: TargetTriple }
  // predicate_fit.rs:
  pub(super) fn check(parsed: &Parsed, target: &TargetTriple) -> Result<(), LowerError>
  ```

- [ ] **Step 1: Write failing tests** in `predicate_fit.rs` (copy the `parsed()` harness from `feature_fit.rs` tests):

```rust
// TOML A (schema_valid in a Branch condition — authoring syntax mirrors
// feature_fit's BRANCH_TOML but with check = "schema_valid", schema = "{}"):
//   → check(&parsed(A), &"any-wasi-strict".parse()?) == Err(WasmFnUnavailable
//     { fn_names: vec!["__tau::goal::schema_valid".into()], .. })
// TOML B (= feature_fit's BRANCH_TOML, check = "non_empty"):
//   → Ok(()) for any-wasi-strict
// TOML A against "linux-native-strict" → Ok(()) (gate is Wasi-only)
```

Discover the exact authoring key for schema_valid by reading how `check = "matches", pattern = …` parses (grep `"schema_valid"` in `crates/tau-pkg/src/project` / the branch-parsing code) and write real TOML. If a Check-step or Deterministic-step cannot be authored from project TOML yet, cover those arms with direct-IR unit tests on the walker function instead (build a `Pipeline` + `checks`/`steps` maps in Rust) — no fixture is left untested.

- [ ] **Step 2: Run to verify failure** — `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower predicate_fit` → FAIL.

- [ ] **Step 3: Implement**

Gate body: return `Ok(())` unless `target.adapter_family == AdapterFamily::Wasi` and a pipeline exists. Walk every `PipelineStep` recursively, collecting offending fn names (sorted, deduped):

```rust
fn walk(steps: &[PipelineStep], parsed: &Parsed, out: &mut BTreeSet<String>) {
    for step in steps {
        match &step.run {
            StepRun::Branch { on, then, otherwise } => {
                predicate(on, out); walk(then, parsed, out); walk(otherwise, parsed, out);
            }
            StepRun::Loop { body, until, .. } => { predicate(until, out); walk(body, parsed, out); }
            StepRun::Parallel { branches } => branches.iter().for_each(|b| walk(b, parsed, out)),
            StepRun::Check(id) => {
                if let Some(check) = parsed.workflow.checks.get(id) {
                    if let CheckVerify::Goal { predicate: p, .. } = &check.verify { goal_predicate(p, out); }
                    // CheckVerify::Deliverable runs an in-guest judge agent — allowed.
                }
            }
            StepRun::Deterministic(id) => {
                if let Some(node) = parsed.workflow.steps.get(id) {
                    if !GUEST_FNS.contains(&node.fn_ref.name.as_str()) { out.insert(node.fn_ref.name.clone()); }
                }
            }
            StepRun::Agent(_) | StepRun::Tool(_) | StepRun::Suspend { .. } | StepRun::Dynamic { .. } => {}
        }
    }
}
```

where `predicate(cond: &Condition, …)` inspects `cond.predicate`, and `goal_predicate` flags `GoalPredicate::SchemaValid(_)` as `"__tau::goal::schema_valid"` and `GoalPredicate::NativeFn(r)` as `r.name` — everything else is guest-answerable. `GUEST_FNS` is the five names; import them from `tau_native_tools::goal_predicates::SUPPORTED` if `tau-ir-lower` may depend on `tau-native-tools` (no cycle: native-tools depends only on serde_json); otherwise duplicate the five literals with a test asserting they match the `tau_runtime_core::vocabulary` constants.

Wire the call in `lower/mod.rs`:

```rust
feature_fit::check(&resolved, target)?;
predicate_fit::check(&resolved, target)?;
```

Add the `error.rs` variant mirroring `FeatureUnsupported`'s message style, e.g. `"predicate-fit: no wasm guest execution path for fn(s) {fn_names:?} on {target}"`.

- [ ] **Step 4: Render in `build_wasm.rs`**

Next to the existing `LowerError::FeatureUnsupported` arm (~line 74), add an arm producing exit-code-2 stderr in the same voice:

```text
predicate-fit refused for any-wasi-strict: no wasm guest execution path for {fn_names:?}.
schema_valid and user-registered fns have no no_std implementation; use exists/non_empty/equals/matches/min_count, or build for a native target.
```

- [ ] **Step 5: Run tests** — `-p tau-ir-lower` and `-p tau-cli` nextest suites → PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ir-lower crates/tau-cli
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ir-lower): wasm-only predicate-fit gate — refuse schema_valid/NativeFn/unknown deterministic fns (#621)"
```

### Task 5: guest executes pipelines via `run_pipeline`

**Files:**
- Create: `crates/tau-wasm-guest/src/goal_registry.rs`
- Modify: `crates/tau-wasm-guest/src/lib.rs` (module decl)
- Modify: `crates/tau-wasm-guest/src/dispatcher.rs` (override `deterministic_registry`)
- Modify: `crates/tau-wasm-guest/src/guest.rs:124-184`
- Modify: `crates/tau-wasm-guest/Cargo.toml` (`tau-native-tools` gains `features = ["goal-predicates"]`)

**Interfaces:**
- Consumes: `goal_predicates::invoke` (Task 1), `Pipeline::final_leaf_step_id` (Task 3), `run_pipeline` (existing), `GuestDispatcher` (existing).
- Produces: guest `run(prompt)` returns the rendered last-leaf output for pipeline IR; single-agent path byte-for-byte unchanged.

- [ ] **Step 1: `goal_registry.rs`**

```rust
//! In-guest `DeterministicRegistry`: the five no_std goal predicates.
//! `schema_valid` / unknown fns are build-time refused (predicate-fit);
//! reaching them here means the gate was bypassed — fail loudly.

use alloc::format;
use alloc::string::String;
use serde_json::Value;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;

pub struct GuestGoalRegistry;

impl DeterministicRegistry for GuestGoalRegistry {
    fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
        match tau_native_tools::goal_predicates::invoke(fn_name, args) {
            Some(Ok(v)) => Ok(v),
            Some(Err(message)) => Err(RuntimeError::Internal { message }),
            None => Err(RuntimeError::Internal {
                message: format!("tau-wasm-guest: fn {fn_name:?} has no wasm execution path (predicate-fit should have refused this build)"),
            }),
        }
    }
}
```

(Adjust imports to what compiles — `String` may be unused.)

- [ ] **Step 2: Override in `dispatcher.rs`**

```rust
fn deterministic_registry(
    &self,
) -> Option<Arc<dyn tau_runtime_core::interpreter::deterministic::DeterministicRegistry>> {
    Some(Arc::new(crate::goal_registry::GuestGoalRegistry))
}
```

- [ ] **Step 3: Replace the pipeline rejection in `guest.rs`**

Delete the `if module.workflow.pipeline.is_some() { return Err(…) }` block (lines 134-143) and rename `_prompt` → `prompt`. After `from_canonical_bytes`, insert (before the entry-agent path; the ports/dispatcher construction mirrors the existing lines 152-162):

```rust
if let Some(pipeline) = &module.workflow.pipeline {
    let last_leaf = pipeline
        .final_leaf_step_id()
        .ok_or_else(|| "tau-wasm-guest: pipeline has only check/suspend steps".to_string())?
        .0
        .clone();

    let backend: Arc<dyn tau_runtime_core::builder::DynLlmBackend> =
        Arc::new(crate::host_ports::HostLlmBackend);
    let clock: Arc<dyn tau_ports::Clock> = Arc::new(crate::host_ports::HostClock);
    let random: Arc<dyn tau_ports::RandomSource> = Arc::new(crate::host_ports::HostRandom);
    let module = Arc::new(module);
    let dispatcher = Arc::new(crate::dispatcher::GuestDispatcher::new(
        backend, clock, random, module.clone(),
    ));

    // Same contract as the native bundle path (`ir_dispatcher::run_via_ir` +
    // `render_pipeline_result`): drive the whole pipeline, return the last
    // leaf's output as the payload. Native pipeline runs stream no RunEvents;
    // neither does this path (terminal-outcome parity, ADR-0068).
    let store = crate::executor::block_on(
        tau_runtime_core::interpreter::pipeline::run_pipeline(module, prompt, dispatcher),
    )
    .map_err(|e| e.to_string())?;

    let value = store.get(&last_leaf).ok_or_else(|| {
        alloc::format!(
            "tau-wasm-guest: pipeline completed but final step {last_leaf:?} recorded no output"
        )
    })?;
    return Ok(match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    });
}
```

If `run_pipeline`'s `input` parameter type differs from the `run` export's `String` (check the signature at `pipeline.rs:223`), adapt at the call site. If `OutputStore::get` takes `&str`, pass `&last_leaf`.

- [ ] **Step 4: Compile the guest for wasm32**

```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-wasm-guest --target wasm32-wasip2 --release
```

Expected: success (this is CI's structural guard, `ci.yml:269`). If `run_pipeline` is feature-gated out of the `wasm-interpreter` feature set, extend that feature in `crates/tau-runtime-core/Cargo.toml` to include the pipeline interpreter module rather than adding a new feature.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-wasm-guest crates/tau-runtime-core
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(wasm-guest): execute pipelines in-guest via run_pipeline (#621)"
```

### Task 6: linear-pipeline wasm e2e (+ shared component-build helper)

**Files:**
- Create: `crates/tau-cli/tests/fixtures/wasm-build/pipeline/tau.toml`
- Create: `crates/tau-cli/tests/common/wasm_component.rs` (extract from `build_wasm_e2e.rs:16-73`)
- Modify: `crates/tau-cli/tests/build_wasm_e2e.rs`
- Modify: `crates/tau-cli/tests/common/mod.rs` (module decl — check how `echo_plugins` is declared and mirror it)

**Interfaces:**
- Produces (used by Task 9): `common::wasm_component::build_component_with_ir(ir_bytes: &[u8]) -> Vec<u8>` — writes `ir_bytes` to a temp file, cargo-builds `tau-wasm-guest` for wasm32-wasip2 with `TAU_IR_BYTES`, `CARGO_TARGET_DIR=target/tau-build-wasm-e2e` (the existing shared dir; cargo's own lock serializes concurrent callers), returns the component bytes.

- [ ] **Step 1: Extract the helper** — move the body of `build_trivial_component` after the `lower_to_wasm_ir` call into `build_component_with_ir(&bytes)`; `build_wasm_e2e.rs` keeps `build_trivial_component()` as `lower_to_wasm_ir(&fixture("trivial")) → build_component_with_ir(&bytes)`.

- [ ] **Step 2: Fixture** — `fixtures/wasm-build/pipeline/tau.toml`, the trivial fixture plus a two-step linear pipeline (linear pipelines already pass wasm feature-fit; this closes today's real gap where such a build produced a component that errored "pipelines are not yet executed in-wasm" at runtime):

```toml
packages = ["anthropic"]

[project]
name = "pipeline-wasm"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.draft]
display_name = "Draft"
package = "pipeline-wasm@^0.1"
model = "claude"

[agents.draft.prompt]
system = "Draft a reply."

[agents.polish]
display_name = "Polish"
package = "pipeline-wasm@^0.1"
model = "claude"

[agents.polish.prompt]
system = "Polish the draft."

[pipeline]

[[pipeline.steps]]
id = "draft"
run = "agent:draft"
input = "${input}"

[[pipeline.steps]]
id = "polish"
run = "agent:polish"
input = "${steps.draft.output}"
```

(If `load_project` rejects this shape — e.g. governance or package resolution — adapt by copying whatever the `trivial` fixture does to satisfy it; the two-agent + linear-pipeline shape is the requirement.)

- [ ] **Step 3: Write the failing e2e** (in `build_wasm_e2e.rs`):

```rust
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn build_wasm_linear_pipeline_runs_in_guest_and_returns_last_leaf() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("pipeline")).expect("lowers");
    let component = common::wasm_component::build_component_with_ir(&bytes);
    let response = |text: &str| {
        format!(r#"{{"text":"{text}","tool_uses":[],"stop_reason":"EndTurn","usage":null}}"#)
    };
    let (payload, _events) = tau_wasm_host::run_component(
        &component,
        "hello",
        vec![response("the draft"), response("the polished reply")],
    )
    .expect("guest runs the pipeline");
    assert_eq!(
        payload, "the polished reply",
        "payload must be the LAST leaf step's rendered output"
    );
}
```

(Check whether `build_wasm_e2e.rs` already declares `mod common;`; add it if not, mirroring `north_star_demo.rs`.)

- [ ] **Step 4: Run it** (wasm toolchain required):

```
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test build_wasm_e2e --run-ignored all
```

Expected: BOTH e2e tests pass (old trivial + new pipeline). Record the component size delta vs a pre-change build in the PR body (spec obligation — the regex-automata cost; `ls -la` the `.wasm` before/after).

- [ ] **Step 5: Commit, push, open PR-2, enrol auto-merge** (same command shapes as Task 3 Step 6; branch `feat/621-wasm-guest-pipeline` if PR-1 is still open, title `feat: #621 PR-2 — guest executes pipelines + predicate-fit gate`).

### Task 7: registry flip + feature-fit/registry test updates

**Files:**
- Modify: `crates/tau-ports/src/target/registry.rs:130-140` + test at `:252-262`
- Modify: `crates/tau-ir-lower/src/lower/feature_fit.rs` (module doc + tests)
- Modify: `docs/**` pages stating wasm cannot execute control-flow (grep, see Step 4)

- [ ] **Step 1: Flip the entry**

```rust
        shapes_fn: fs_rw_net,
        // Guest control-flow (ADR-0068, #621): Branch/Parallel/Loop execute
        // in-guest via run_pipeline. Suspend (no SuspensionStore channel in
        // the WIT world) and Dynamic (EPIC 4.5 pending) stay build-refused.
        supported_features: &[IrFeature::Branch, IrFeature::Parallel, IrFeature::Loop],
```

- [ ] **Step 2: Replace the emptiness test**

```rust
#[test]
fn any_wasi_strict_supports_exactly_branch_parallel_loop() {
    // ADR-0068: in-guest run_pipeline executes Branch/Parallel/Loop.
    // Suspend and Dynamic must stay absent (build-time refusal).
    let t: TargetTriple = "any-wasi-strict".parse().unwrap();
    let e = lookup(&t).expect("any-wasi-strict must be registered");
    assert_eq!(
        e.supported_features,
        &[IrFeature::Branch, IrFeature::Parallel, IrFeature::Loop],
    );
}
```

- [ ] **Step 3: Update `feature_fit.rs`** — rewrite the stale module-doc paragraph ("Today the only target that lists no features…") to describe the ADR-0068 state; change `wasm_target_rejects_control_flow` to `wasm_target_accepts_branch` (asserts `Ok(())` on `BRANCH_TOML`); add a Suspend refusal test:

```rust
const SUSPEND_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "a"
run = "agent:a"
input = "${input}"

[[pipeline.steps]]
id = "pause"
run = "suspend:human"
"#;

#[test]
fn wasm_target_rejects_suspend() {
    let t: TargetTriple = "any-wasi-strict".parse().unwrap();
    let err = check(&parsed(SUSPEND_TOML), &t).expect_err("wasm must refuse Suspend");
    match err {
        LowerError::FeatureUnsupported { missing, target } => {
            assert_eq!(missing, alloc::vec![IrFeature::Suspend]);
            assert_eq!(target, t);
        }
        other => panic!("expected FeatureUnsupported, got {other:?}"),
    }
}
```

Keep `wasm_target_rejects_dynamic_region` unchanged.

- [ ] **Step 4: Sweep stale claims** — `grep -rn "no features\|cannot execute control-flow\|supported_features" crates docs --include="*.rs" --include="*.md" -l`, fix every comment/doc page still claiming wasm supports nothing (at minimum: `registry.rs` old comment is gone via Step 1; `north_star_demo.rs` header comment; the fixture's `tau.toml` header comment; any docs/ page). For docs/: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` must stay clean; `rm -rf docs/book` after.

- [ ] **Step 5: Run tests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-lower
```

Expected: PASS. (`tau-cli`'s north-star refusal test now FAILS — expected; Task 8 fixes it. Do not commit yet if you want green-per-commit: Tasks 7+8 may be one commit.)

### Task 8: north-star refusal retargets to a Suspend twin

**Files:**
- Create: `crates/tau-cli/tests/fixtures/north-star-suspend/tau.toml`
- Modify: `crates/tau-cli/tests/north_star_demo.rs:279-294`

- [ ] **Step 1: Fixture** — copy `fixtures/north-star/tau.toml` byte-for-byte, then insert between the `review` Loop block and the final `report` step:

```toml
# Suspend twin: identical workflow plus a human pause. Wasm builds must
# refuse THIS at feature-fit (ADR-0068: no SuspensionStore channel in the
# guest); the suspend-free original builds and runs in-guest.
[[pipeline.steps]]
id = "pause"
run = "suspend:human-signoff"
```

Update the copied header comment to say this is the refusal twin.

- [ ] **Step 2: Retarget the test**

```rust
/// Wasm path: control-flow now executes in-guest (ADR-0068), so the
/// refusal witness moves to the Suspend twin — the guest has no durable
/// suspend channel, and feature-fit refuses BEFORE any artifact exists.
#[test]
fn north_star_wasm_guest_build_is_refused_at_feature_fit() {
    let dir = setup_project(&fixture_toml("north-star-suspend"));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build", "--target", "wasm-guest"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("feature-fit"))
        .stderr(predicate::str::contains("Suspend"));
}
```

Check whether the dev-leg tests should also witness the Suspend twin runs natively (exit 3 suspension path) — NO: out of scope, the twin exists only as a build-refusal witness.

- [ ] **Step 3: Run** — `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test north_star_demo` → PASS.

- [ ] **Step 4: Commit** (Tasks 7+8 together so the tree is green):

```bash
git add crates/tau-ports crates/tau-ir-lower crates/tau-cli docs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(ports): any-wasi-strict supports Branch/Parallel/Loop; refusal witness moves to Suspend twin (#621)"
```

### Task 9: north-star wasm execution leg

**Files:**
- Modify: `crates/tau-cli/tests/north_star_demo.rs` (new test + `mod common;` already present)

**Interfaces:**
- Consumes: `common::wasm_component::build_component_with_ir` (Task 6), `tau_cli::cmd::build_wasm::lower_to_wasm_ir`, `tau_wasm_host::run_component`, the existing `setup_project`/`SENTINEL`.

- [ ] **Step 1: Write the test**

```rust
/// Wasm execution leg (#621 DoD): the SAME Branch+Loop fixture builds for
/// wasm and the guest executes it to the SAME terminal outcome as the dev
/// leg. Completion itself witnesses both control-flow paths: `report`'s
/// input template references `steps.escalate.output` (Branch then-arm) and
/// `steps.draft.output` (Loop body), and template resolution hard-errors
/// on unresolved refs — so a returned payload proves both ran in-guest.
#[test]
#[ignore = "builds a wasm component; tier2 --run-ignored lane"]
fn north_star_wasm_guest_executes_same_workflow_same_terminal_outcome() {
    let dir = setup_project(&fixture_toml("north-star"));

    let (_module, ir_bytes) = tau_cli::cmd::build_wasm::lower_to_wasm_ir(dir.path())
        .expect("wasm lowering now admits Branch+Loop (ADR-0068)");
    let component = common::wasm_component::build_component_with_ir(&ir_bytes);

    // Host cassette replays the fixture's canned text for every agent turn
    // (triage, escalate, draft, report consume one completion each; extra
    // entries stay unconsumed).
    let response = serde_json::json!({
        "text": SENTINEL, "tool_uses": [], "stop_reason": "EndTurn", "usage": null
    })
    .to_string();
    let (payload, _events) =
        tau_wasm_host::run_component(&component, "incident: coolant temperature rising", vec![response; 8])
            .expect("guest executes the Branch+Loop pipeline");

    // Same terminal outcome as the dev leg (`assert_completed_pipeline_outcome`
    // asserts final_message == SENTINEL): the last leaf (`report`) echoes the
    // canned text.
    assert_eq!(payload, SENTINEL);
}
```

Adapt mechanical details to what compiles: `lower_to_wasm_ir` takes `&Path`; `vec![response; 8]` needs `response: String` cloneable (it is); if `SENTINEL` is a `&str` const the `json!` embeds it directly. If `lower_to_wasm_ir` fails on the echo-scaffold project for a reason unrelated to feature-fit (e.g. package-resolution differences vs the anthropic fixture), fix the SETUP (extend `setup_project`'s scaffold), not the production code — the dev `tau build` test (`north_star_builds_governed_bundle`) proves the project lowers.

- [ ] **Step 2: Run it** (wasm toolchain required):

```
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test north_star_demo --run-ignored all
```

Expected: PASS (non-ignored legs + the new wasm leg).

- [ ] **Step 3: Verify the tier2 lane picks it up** — read `.github/workflows/tier2.yml:~275` (`--run-ignored only`); confirm its nextest filter includes `-p tau-cli` ignored tests (it runs the workspace's ignored set). If the lane filters by test name, add this test to the filter.

- [ ] **Step 4: Full pre-PR gates**

```
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ports -p tau-ir -p tau-ir-lower -p tau-native-tools -p tau-wasm-guest -p tau-cli -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

Expected: all clean (workspace lints make warnings deny in CI).

- [ ] **Step 5: Commit, push, open PR-3, enrol auto-merge**

```bash
git add crates/tau-cli
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "test(cli): north-star wasm leg — guest executes Branch+Loop, same terminal outcome (#621)"
git push -u origin <branch>
gh pr create --base main --title "feat: #621 PR-3 — flip any-wasi-strict features; north-star wasm execution leg" --body "<summary; closes #621; cite ADR-0068>"
gh pr merge <N> --squash --delete-branch --auto
```

---

## Self-review notes (spec → plan)

- Spec "supported set flip + exact-set registry test" → Task 7. "Guest pipeline path + payload contract" → Task 5. "no_std registry split" → Tasks 1–2. "Shared last-leaf helper (three call sites)" → Tasks 3, 5. "Build-time fn gate" → Task 4. "North-star DoD (refusal twin + execution leg)" → Tasks 8–9. "Size measurement obligation" → Task 1 Step 4 + Task 6 Step 4. "Determinism invariant" → implied by `run_component`'s cassette + Task 6/9 assertions.
- Known unknowns are confined and have named fallbacks: regex-automata feature list (Task 1 Step 1), schema_valid authoring TOML (Task 4 Step 1), `run_pipeline` feature-gating (Task 5 Step 4), fixture project-loading quirks (Task 6 Step 2, Task 9 Step 1). Each names the file to read and the acceptance command.
- Out of scope (spec §Out of scope): Suspend/Dynamic on wasm, pipeline event streaming/`WasmProfile`, artifact-locus checks in-guest, `schema_valid` no_std port.
