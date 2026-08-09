# EPIC 4.3 — Suspend (HITL pause + resume) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Author a `Suspend` pipeline step (`run = "suspend:<signal>"`), have the engine checkpoint + pause at it, and resume past it via `tau run --resume <id> --signal <name>`.

**Architecture:** Suspend is a 5th leaf `run`-kind (top-level only, enforced at typecheck). The interpreter uses **restore-and-continue**: on pause it persists the full `OutputStore` + step cursor via a new `SuspensionStore` port; on resume it restores the store and jumps to cursor+1 — prior (LLM) steps are never re-run. The pipeline gains a dedicated `PipelineOutcome::{Completed, Suspended}` return; suspend state lives in `.tau/runs/<run_id>/suspend.json` beside the existing turn checkpoints.

**Tech Stack:** Rust workspace (no_std kernel core + tokio host), `serde_json`, `sha2`, `cargo nextest`.

## Global Constraints

- **CARGO RULES (verbatim from CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-<role> cargo <cmd> -p <crate>`. Timeouts: test 300, build/check 180, clippy 240, fmt 30. Use `cargo nextest run` for tests, `cargo test --doc` for doctests. Never bare `cargo`, never `--workspace`, always `-p`.
- **tau-ports version bump: `0.3.0` → `0.4.0`** (new pub trait + struct trips the ABI semver gate; branch is fast-forwarded to main @ f930ef6c where tau-ports is already `0.3.0`).
- **`PipelineRunRef` is an exhaustive pub enum** — adding `Suspend` is a workspace-internal breaking change; sweep every `match` on it.
- **no_std kernel:** `tau-domain`, `tau-ir`, `tau-ports`, `tau-runtime-core` are `no_std + alloc`. Use `alloc::` types, `BTreeMap`, no `std::`.
- **Nested step ids share ONE flat global namespace** (typecheck.rs).
- **Suspend is top-level only** — reject anywhere below the top-level pipeline slice at typecheck (build-time enforcement).
- **resume_signal match semantics** — the resumer must pass the exact signal; mismatch is rejected.
- Conventional commits, imperative, scoped.

---

### Task 1: `SuspensionStore` port + `PipelineSuspension` (tau-ports)

**Files:**
- Modify: `crates/tau-ports/Cargo.toml:4` (version → `0.4.0`)
- Modify: `crates/tau-ports/src/orchestration.rs` (add type + trait after `CheckpointStore`)
- Modify: `crates/tau-ports/src/lib.rs:62` (re-export `PipelineSuspension`, `SuspensionStore`)
- Modify: `crates/tau-ports/src/fixtures.rs` (add `MockSuspensionStore` after `MockCheckpointStore`, ~line 904)

**Interfaces:**
- Produces: `PipelineSuspension { run_id: RunId, resume_signal: String, step_cursor: usize, step_id: String, ir_digest: String, outputs: BTreeMap<String, serde_json::Value> }`; `trait SuspensionStore { persist_suspension(&self, &PipelineSuspension) -> Result<(), CheckpointError>; load_suspension(&self, &RunId) -> Result<Option<PipelineSuspension>, CheckpointError> }`; `MockSuspensionStore::{new, persist_count}`.

- [ ] **Step 1: Write the failing test** — append to `crates/tau-ports/src/fixtures.rs` under a `#[cfg(test)]` block (or add to the existing tests module):

```rust
#[cfg(all(test, feature = "serde"))]
mod suspension_fixture_tests {
    use super::*;
    use crate::orchestration::{PipelineSuspension, SuspensionStore};
    use alloc::collections::BTreeMap;

    fn susp(run_id: &str, cursor: usize) -> PipelineSuspension {
        PipelineSuspension {
            run_id: run_id.into(),
            resume_signal: "approved".into(),
            step_cursor: cursor,
            step_id: "pause".into(),
            ir_digest: "sha256:deadbeef".into(),
            outputs: BTreeMap::new(),
        }
    }

    #[test]
    fn mock_persists_and_loads_by_run_id() {
        let store = MockSuspensionStore::new();
        assert!(store.load_suspension(&"r".to_string()).unwrap().is_none());
        store.persist_suspension(&susp("r", 2)).unwrap();
        let got = store.load_suspension(&"r".to_string()).unwrap().unwrap();
        assert_eq!(got.step_cursor, 2);
        assert_eq!(got.resume_signal, "approved");
        assert_eq!(store.persist_count(), 1);
    }

    #[test]
    fn later_persist_overwrites_prior_for_same_run() {
        let store = MockSuspensionStore::new();
        store.persist_suspension(&susp("r", 1)).unwrap();
        store.persist_suspension(&susp("r", 5)).unwrap();
        assert_eq!(store.load_suspension(&"r".to_string()).unwrap().unwrap().step_cursor, 5);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-ports cargo nextest run -p tau-ports --features serde,test-fixtures suspension_fixture`
Expected: FAIL — `PipelineSuspension` / `SuspensionStore` / `MockSuspensionStore` not found.

- [ ] **Step 3: Add the type + trait** to `crates/tau-ports/src/orchestration.rs`, immediately after the `CheckpointStore` trait (~line 327):

```rust
/// A resumable snapshot of a pipeline paused at a top-level `Suspend` step.
///
/// Distinct from [`TurnCheckpoint`] (agent-turn durability): this carries the
/// pipeline `OutputStore` snapshot + the step cursor, not message history. Both
/// share the `run_id` handle and the `.tau/runs/<run_id>/` directory. Resume is
/// restore-and-continue: rehydrate `outputs`, jump to `step_cursor + 1`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PipelineSuspension {
    /// Run this suspension belongs to (the `--resume` key).
    pub run_id: RunId,
    /// Signal the resumer must match to continue (`--signal`).
    pub resume_signal: String,
    /// Index of the `Suspend` step in the top-level pipeline slice. Resume
    /// re-enters at `step_cursor + 1`.
    pub step_cursor: usize,
    /// The `Suspend` step's id (for the "paused at <id>" human message).
    pub step_id: String,
    /// Canonical-IR SHA-256 of the module at pause time (`"sha256:" + hex`).
    /// Resume rejects a project that changed since the pause.
    pub ir_digest: String,
    /// The `OutputStore` snapshot as of the pause (step id -> output value).
    pub outputs: BTreeMap<String, serde_json::Value>,
}

/// Port: persists and loads a pipeline [`PipelineSuspension`] for HITL resume.
///
/// One live suspension per run (a second `Suspend` on resume overwrites it; a
/// completed run removes it). Keyed by the same `RunId` as [`CheckpointStore`]
/// and stored in the same run directory, so one `--resume <run_id>` handle
/// covers both agent-turn and pipeline-step resume.
pub trait SuspensionStore: Send + Sync {
    /// Durably record the pause point. Overwrites any prior suspension for the
    /// same `run_id`.
    fn persist_suspension(&self, s: &PipelineSuspension) -> Result<(), CheckpointError>;

    /// Load the current suspension for `run_id`, or `None` if the run is not
    /// paused.
    fn load_suspension(&self, run_id: &RunId) -> Result<Option<PipelineSuspension>, CheckpointError>;
}
```

Add the `use` for `serde_json::Value` if not present at the top of the file (`use serde_json::Value;` is not — use the fully-qualified `serde_json::Value` in the field type as written above, which needs `serde_json` as a dep; confirm it is: tau-ports already depends on `serde_json` via the `serde` feature — check `Cargo.toml`; if it is optional under `serde`, gate `PipelineSuspension.outputs` accordingly or keep `serde_json` available).

- [ ] **Step 4: Add `MockSuspensionStore`** to `crates/tau-ports/src/fixtures.rs`, after `MockCheckpointStore` (~line 904):

```rust
use crate::orchestration::{PipelineSuspension, SuspensionStore};

/// In-memory [`SuspensionStore`] for tests. One suspension per run (last write
/// wins), mirroring the host `FileCheckpointStore` semantics.
#[derive(Default)]
pub struct MockSuspensionStore {
    by_run: Mutex<BTreeMap<RunId, PipelineSuspension>>,
    persists: Mutex<usize>,
}

impl MockSuspensionStore {
    /// Construct an empty store.
    pub fn new() -> Self { Self::default() }
    /// Total `persist_suspension` calls across all runs. Test introspection.
    pub fn persist_count(&self) -> usize { *self.persists.lock().expect("persist mutex") }
}

impl SuspensionStore for MockSuspensionStore {
    fn persist_suspension(&self, s: &PipelineSuspension) -> Result<(), CheckpointError> {
        self.by_run.lock().expect("suspension mutex").insert(s.run_id.clone(), s.clone());
        *self.persists.lock().expect("persist mutex") += 1;
        Ok(())
    }
    fn load_suspension(&self, run_id: &RunId) -> Result<Option<PipelineSuspension>, CheckpointError> {
        Ok(self.by_run.lock().expect("suspension mutex").get(run_id).cloned())
    }
}
```

- [ ] **Step 5: Re-export** in `crates/tau-ports/src/lib.rs:62` — extend the orchestration re-export list to include `PipelineSuspension, SuspensionStore`.

- [ ] **Step 6: Run tests + verify PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-ports cargo nextest run -p tau-ports --features serde,test-fixtures`
Expected: PASS.

- [ ] **Step 7: fmt + clippy**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-ports cargo fmt -p tau-ports -- --check` then `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-ports cargo clippy -p tau-ports --all-targets -- -D warnings`

- [ ] **Step 8: Commit**

```bash
git add crates/tau-ports
git commit -m "feat(tau-ports): SuspensionStore port + PipelineSuspension (EPIC 4.3)"
```

---

### Task 2: Authoring surface `run = "suspend:<signal>"` (tau-pkg)

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs:407` (`PipelineRunRef` — add `Suspend`)
- Modify: `crates/tau-pkg/src/project/project.rs:2003` (leaf `split_once(':')` match — add `suspend` arm)
- Test: same file's `#[cfg(test)]` module (parse tests live there, e.g. ~line 3979)

**Interfaces:**
- Consumes: nothing new.
- Produces: `PipelineRunRef::Suspend { resume_signal: String }`.

- [ ] **Step 1: Write the failing test** in `crates/tau-pkg/src/project/project.rs` tests module:

```rust
#[test]
fn parses_suspend_leaf_step() {
    let toml = r#"
        [project]
        name = "p"
        [[pipeline.steps]]
        id = "await-approval"
        run = "suspend:approved"
    "#;
    let cfg = ProjectConfig::from_toml_str(toml).expect("valid");
    let pipe = cfg.pipeline.as_ref().expect("pipeline");
    assert_eq!(
        pipe.steps[0].run,
        PipelineRunRef::Suspend { resume_signal: "approved".into() }
    );
}

#[test]
fn rejects_suspend_with_empty_signal() {
    let toml = r#"
        [project]
        name = "p"
        [[pipeline.steps]]
        id = "await"
        run = "suspend:"
    "#;
    assert!(ProjectConfig::from_toml_str(toml).is_err());
}
```

(Match the exact `ProjectConfig` constructor used by neighbouring tests — grep for `from_toml_str` / `parse` in the tests module and use that helper.)

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-pkg cargo nextest run -p tau-pkg suspend`
Expected: FAIL — no `Suspend` variant.

- [ ] **Step 3: Add the variant** to `PipelineRunRef` (`project.rs:407`), after `Check`:

```rust
    /// `suspend:<signal>` — pause the pipeline until resumed with a matching
    /// signal. Top-level only (enforced at typecheck). Produces no output.
    Suspend {
        /// The signal a resumer must match to continue.
        resume_signal: String,
    },
```

- [ ] **Step 4: Add the parse arm** at `project.rs:2003` inside `match run_str.split_once(':')`, after the `check` arm (line 2007):

```rust
            Some(("suspend", sig)) if !sig.is_empty() =>
                PipelineRunRef::Suspend { resume_signal: sig.to_string() },
```

(The empty-signal case falls through to the existing malformed-`run` error arm, satisfying `rejects_suspend_with_empty_signal`. Confirm the fall-through arm returns an error, not a panic.)

- [ ] **Step 5: Run tests + verify PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-pkg cargo nextest run -p tau-pkg`
Expected: PASS. (Compiler will flag any other exhaustive `match` on `PipelineRunRef` in tau-pkg — add a `Suspend` arm wherever it does; e.g. any producer/consumer analysis around `project.rs:2429/2477/2539`. For those analyses Suspend has no agent/tool, so the arm is a no-op `_ => {}`-equivalent handled explicitly.)

- [ ] **Step 6: fmt + clippy**

Run: `timeout 30 env CARGO_TARGET_DIR=target/agent-pkg cargo fmt -p tau-pkg -- --check` then `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-pkg cargo clippy -p tau-pkg --all-targets -- -D warnings`

- [ ] **Step 7: Commit**

```bash
git add crates/tau-pkg
git commit -m "feat(tau-pkg): author suspend leaf step run=\"suspend:<signal>\" (EPIC 4.3)"
```

---

### Task 3: Lowering + typecheck rules (tau-ir-lower)

**Files:**
- Modify: `crates/tau-ir-lower/src/lower/parse.rs:529` (`lower_step` match — add `Suspend` arm)
- Modify: `crates/tau-ir-lower/src/lower/typecheck.rs` (nested-rejection at ~line 508; no-output-ref rejection in the ref loop ~line 243)
- Modify: `crates/tau-ir-lower/src/error.rs` (2 new `LowerError` variants)
- Test: `crates/tau-ir-lower/src/lower/typecheck.rs` tests module + a lowering test near `parse.rs`

**Interfaces:**
- Consumes: `PipelineRunRef::Suspend { resume_signal }` (Task 2), `StepRun::Suspend { resume_signal }` (tau-ir, exists).
- Produces: `LowerError::SuspendNotTopLevel { step }`, `LowerError::SuspendHasNoOutput { step, referenced }`.

- [ ] **Step 1: Write the failing lowering test** (near `parse.rs`'s tests or the lower integration tests):

```rust
#[test]
fn lowers_suspend_leaf() {
    // Build a ProjectConfig with a single suspend step, lower it, assert the
    // IR carries StepRun::Suspend { resume_signal: "go" }.
    // (Use the crate's existing lower-a-project test helper.)
    let module = lower_single_pipeline_step(
        "pause",
        tau_pkg::project::PipelineRunRef::Suspend { resume_signal: "go".into() },
    );
    let step = &module.workflow.pipeline.unwrap().steps[0];
    assert_eq!(step.run, tau_ir::pipeline::StepRun::Suspend { resume_signal: "go".into() });
}
```

- [ ] **Step 2: Add the lowering arm** to `parse.rs:529` `match &s.run`, after the `Check` arm (line 533):

```rust
        PipelineRunRef::Suspend { resume_signal } =>
            StepRun::Suspend { resume_signal: resume_signal.clone() },
```

- [ ] **Step 3: Run the lowering test → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-lower cargo nextest run -p tau-ir-lower lowers_suspend`
Expected: PASS.

- [ ] **Step 4: Write the failing typecheck tests** in `typecheck.rs` tests module:

```rust
#[test]
fn typecheck_rejects_suspend_inside_branch() {
    // Top-level Branch whose `then` contains StepRun::Suspend → SuspendNotTopLevel.
    let err = typecheck_pipeline(vec![branch_step(
        "gate",
        vec![suspend_step("pause", "go")], // then
        vec![],                            // otherwise
    )])
    .expect_err("nested suspend rejected");
    assert!(matches!(err, LowerError::SuspendNotTopLevel { step } if step == "pause"));
}

#[test]
fn typecheck_allows_top_level_suspend() {
    typecheck_pipeline(vec![suspend_step("pause", "go")]).expect("top-level suspend ok");
}

#[test]
fn typecheck_rejects_ref_to_suspend_output() {
    // agent step whose input is "${steps.pause.output}" after a suspend `pause`.
    let err = typecheck_pipeline(vec![
        suspend_step("pause", "go"),
        agent_step_with_input("tail", "echo", "${steps.pause.output}"),
    ])
    .expect_err("ref to suspend output rejected");
    assert!(matches!(err, LowerError::SuspendHasNoOutput { referenced, .. } if referenced == "pause"));
}
```

(Reuse the tests module's existing builders — grep for how `typecheck` is invoked in the current tests, e.g. a `Parsed`/`Workflow` fixture; add small `suspend_step`/`branch_step` helpers mirroring the existing ones.)

- [ ] **Step 5: Run → verify FAIL**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-lower cargo nextest run -p tau-ir-lower typecheck_rejects_suspend typecheck_allows_top_level typecheck_rejects_ref_to_suspend`
Expected: FAIL — variants don't exist; nested suspend currently accepted.

- [ ] **Step 6: Add the two `LowerError` variants** in `crates/tau-ir-lower/src/error.rs`:

```rust
    /// A `Suspend` step appeared below the top-level pipeline slice (inside a
    /// Branch arm, Loop body, or Parallel branch). Suspend is top-level only.
    #[error("suspend step {step:?} is nested inside a control-flow block; suspend is only allowed at the top level of the pipeline (EPIC 4.3)")]
    SuspendNotTopLevel { step: String },

    /// A step template references `${steps.<id>.output}` where `<id>` is a
    /// `Suspend` step, which produces no output.
    #[error("step {step:?} references {referenced:?}.output, but {referenced:?} is a suspend step and produces no output")]
    SuspendHasNoOutput { step: String, referenced: String },
```

- [ ] **Step 7: Thread a `nested` flag through `validate_step_run`** (`typecheck.rs:383`). Change its signature to add `nested: bool` (last param). The top-level call at `typecheck.rs:206` passes `false`; every recursive call inside the `Branch`/`Parallel`/`Loop` arms (the `validate_step_run(&nested.run, ...)` calls around lines 440/458/498/502) passes `true`. Then replace the `StepRun::Suspend { .. } => { ... }` arm at `typecheck.rs:508`:

```rust
        StepRun::Suspend { .. } => {
            if nested {
                return Err(LowerError::SuspendNotTopLevel {
                    step: outer_step_id.into(),
                });
            }
            // Top-level suspend: nothing to validate (produces no output,
            // references nothing).
        }
```

- [ ] **Step 8: Reject refs to suspend outputs.** Build a set of suspend-step ids once (alongside `all_ids`), then in the ref-validation loop (`typecheck.rs:243`), after the existence check, reject a `StepOutput` ref whose target is a suspend id:

```rust
    // Near where `all_ids` is built — collect ids of Suspend steps tree-wide.
    let mut suspend_ids: alloc::collections::BTreeSet<&str> = Default::default();
    collect_suspend_ids(&pipeline.steps, &mut suspend_ids);
    // ... inside the `for r in refs` loop, in the `TemplateRef::StepOutput(ref_id)` arm,
    // after the `all_ids.contains` existence check:
    if suspend_ids.contains(ref_id.as_str()) {
        return Err(LowerError::SuspendHasNoOutput {
            step: sid.into(),
            referenced: ref_id,
        });
    }
```

Add the helper beside `collect_all_ids` (`typecheck.rs:297`):

```rust
fn collect_suspend_ids<'a>(steps: &'a [PipelineStep], out: &mut BTreeSet<&'a str>) {
    for step in steps {
        match &step.run {
            StepRun::Suspend { .. } => { out.insert(step.id.0.as_str()); }
            StepRun::Branch { then, otherwise, .. } => {
                collect_suspend_ids(then, out);
                collect_suspend_ids(otherwise, out);
            }
            StepRun::Parallel { branches } => for b in branches { collect_suspend_ids(b, out); },
            StepRun::Loop { body, .. } => collect_suspend_ids(body, out),
            _ => {}
        }
    }
}
```

- [ ] **Step 9: Run typecheck tests + full crate → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-lower cargo nextest run -p tau-ir-lower`
Expected: PASS.

- [ ] **Step 10: fmt + clippy + commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-lower cargo fmt -p tau-ir-lower -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-lower cargo clippy -p tau-ir-lower --all-targets -- -D warnings
git add crates/tau-ir-lower
git commit -m "feat(tau-ir-lower): lower suspend + typecheck top-level-only & no-output-ref (EPIC 4.3)"
```

---

### Task 4: Interpreter pause + resume (tau-runtime-core)

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/output_store.rs:15` (serde derives + `snapshot`/`restore`)
- Modify: `crates/tau-runtime-core/src/interpreter/pipeline.rs` (`PipelineOutcome`, `StepsFlow`, `run_steps` suspend handling, `run_pipeline_suspendable`, `run_pipeline` wrapper)
- Modify: `crates/tau-runtime-core/src/error.rs:369` (retire `SuspendNotImplemented`, add `SuspendUnsupported`)
- Test: `crates/tau-runtime-core/tests/pipeline_control_flow.rs` (update the existing suspend test + add resume tests)

**Interfaces:**
- Consumes: `PipelineSuspension`, `SuspensionStore` (Task 1); `tau_ir::to_canonical_bytes`, `tau_ir::asset::asset_hash` (for `ir_digest`).
- Produces:
  - `pub enum PipelineOutcome { Completed(OutputStore), Suspended { run_id: String, resume_signal: String, step_id: String } }`
  - `pub struct SuspendConfig { pub run_id: String, pub store: Arc<dyn SuspensionStore> }`
  - `pub struct ResumeState { pub store: OutputStore, pub start_at: usize }`
  - `pub async fn run_pipeline_suspendable<D>(module, input, dispatcher, suspend: SuspendConfig, resume: Option<ResumeState>) -> Result<PipelineOutcome, RuntimeError>`
  - `run_pipeline` keeps `(module, input, dispatcher) -> Result<OutputStore, RuntimeError>` (wrapper; errors `SuspendUnsupported` if the pipeline suspends).

- [ ] **Step 1: OutputStore serde + snapshot/restore.** Edit `output_store.rs:15`:

```rust
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputStore {
    map: alloc::collections::BTreeMap<alloc::string::String, serde_json::Value>,
}
```
Add methods:
```rust
    /// Clone the backing map for a durable suspension snapshot.
    pub fn snapshot(&self) -> alloc::collections::BTreeMap<alloc::string::String, serde_json::Value> {
        self.map.clone()
    }
    /// Rebuild a store from a persisted snapshot (resume path).
    pub fn restore(map: alloc::collections::BTreeMap<alloc::string::String, serde_json::Value>) -> Self {
        Self { map }
    }
```

- [ ] **Step 2: Write the failing interpreter tests** in `tests/pipeline_control_flow.rs`. Replace `suspend_returns_named_error` and add resume coverage:

```rust
use tau_ports::fixtures::MockSuspensionStore;
use tau_ports::orchestration::SuspensionStore;
use tau_runtime_core::interpreter::pipeline::{
    run_pipeline_suspendable, PipelineOutcome, ResumeState, SuspendConfig,
};
use std::sync::Arc;

#[tokio::test]
async fn suspend_persists_and_returns_suspended() {
    let module = Arc::new(seed_then_suspend_module()); // agent:seed -> suspend:go(id "pause")
    let store: Arc<dyn SuspensionStore> = Arc::new(MockSuspensionStore::new());
    let outcome = run_pipeline_suspendable(
        module.clone(),
        "x".to_string(),
        dispatcher(),
        SuspendConfig { run_id: "r1".into(), store: store.clone() },
        None,
    )
    .await
    .expect("suspends cleanly");
    match outcome {
        PipelineOutcome::Suspended { run_id, resume_signal, step_id } => {
            assert_eq!(run_id, "r1");
            assert_eq!(resume_signal, "go");
            assert_eq!(step_id, "pause");
        }
        other => panic!("expected Suspended, got {other:?}"),
    }
    // The seed step's output was persisted in the snapshot.
    let susp = store.load_suspension(&"r1".to_string()).unwrap().unwrap();
    assert_eq!(susp.step_cursor, 1); // index of "pause"
    assert!(susp.outputs.contains_key("seed"));
}

#[tokio::test]
async fn resume_continues_without_rerunning_prefix() {
    let module = Arc::new(seed_suspend_tail_module()); // seed -> suspend(pause) -> tail(agent)
    let counting = counting_dispatcher(); // counts run_agent invocations by agent id
    let store: Arc<dyn SuspensionStore> = Arc::new(MockSuspensionStore::new());

    // Run 1: suspends after seed.
    let _ = run_pipeline_suspendable(
        module.clone(), "x".into(), counting.clone(),
        SuspendConfig { run_id: "r2".into(), store: store.clone() }, None,
    ).await.unwrap();
    let susp = store.load_suspension(&"r2".to_string()).unwrap().unwrap();

    // Run 2: resume restores the store and continues at cursor+1.
    let outcome = run_pipeline_suspendable(
        module.clone(), "x".into(), counting.clone(),
        SuspendConfig { run_id: "r2".into(), store: store.clone() },
        Some(ResumeState {
            store: tau_runtime_core::interpreter::output_store::OutputStore::restore(susp.outputs),
            start_at: susp.step_cursor + 1,
        }),
    ).await.unwrap();

    assert!(matches!(outcome, PipelineOutcome::Completed(_)));
    // seed ran exactly once (run 1 only); tail ran exactly once (run 2 only).
    assert_eq!(counting.calls("seed"), 1, "prefix must NOT be re-run on resume");
    assert_eq!(counting.calls("tail"), 1);
}
```

(Add small module/dispatcher builders beside the existing `suspend_module`/`dispatcher` helpers. `counting_dispatcher` wraps the test dispatcher and increments a per-agent-id counter in `run_agent`/`invoke`; if the existing test dispatcher can't count, add an `Arc<Mutex<BTreeMap<String,usize>>>` field.)

- [ ] **Step 3: Run → verify FAIL**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-rt cargo nextest run -p tau-runtime-core suspend resume_continues`
Expected: FAIL — new symbols missing.

- [ ] **Step 4: Retire `SuspendNotImplemented`, add `SuspendUnsupported`** in `error.rs:369`:

```rust
    /// A pipeline hit a `Suspend` step but the caller wired no `SuspensionStore`
    /// (e.g. the bundle-run path in v1, or a context that cannot pause). Callers
    /// that support HITL use `run_pipeline_suspendable`.
    #[error("suspend {step} (signal {resume_signal}) requires a suspend-capable run path; this caller cannot pause")]
    SuspendUnsupported { step: String, resume_signal: String },
```

- [ ] **Step 5: Add `PipelineOutcome`, `StepsFlow`, `SuspendConfig`, `ResumeState`** to `pipeline.rs` (top-level items):

```rust
/// Terminal outcome of driving a pipeline.
#[derive(Debug)]
pub enum PipelineOutcome {
    /// Ran to the end; carries the accumulated outputs.
    Completed(OutputStore),
    /// Paused at a top-level `Suspend`. State persisted via the `SuspensionStore`.
    Suspended { run_id: alloc::string::String, resume_signal: alloc::string::String, step_id: alloc::string::String },
}

/// How a `run_steps` slice terminated. Only the top-level slice can Suspend.
enum StepsFlow {
    Completed,
    Suspended { step_id: alloc::string::String, resume_signal: alloc::string::String },
}

/// Durable-suspend wiring for the top-level pipeline driver.
pub struct SuspendConfig {
    /// Run id used as the suspension key + `--resume` handle.
    pub run_id: alloc::string::String,
    /// Where a pause is persisted.
    pub store: alloc::sync::Arc<dyn tau_ports::orchestration::SuspensionStore>,
}

/// Rehydrated state for a resumed run.
pub struct ResumeState {
    /// The `OutputStore` snapshot restored from the suspension.
    pub store: OutputStore,
    /// Top-level step index to resume at (`step_cursor + 1`).
    pub start_at: usize,
}

// Internal: passed down to `run_steps` for the top-level slice only.
struct SuspendCtx<'a> {
    run_id: &'a str,
    ir_digest: &'a str,
    store: &'a dyn tau_ports::orchestration::SuspensionStore,
}
```

- [ ] **Step 6: Change `run_steps`** (`pipeline.rs:117`) to return `Result<StepsFlow, RuntimeError>`, and add two params: `suspend: Option<&SuspendCtx<'_>>` and `start_at: usize`.
  - Initialise the index loop with `let mut i = start_at;` instead of `0`.
  - Replace the Suspend stub (`pipeline.rs:372`) with:

```rust
        if let StepRun::Suspend { resume_signal } = &step.run {
            match suspend {
                Some(ctx) => {
                    ctx.store.persist_suspension(&tau_ports::orchestration::PipelineSuspension {
                        run_id: ctx.run_id.to_string(),
                        resume_signal: resume_signal.clone(),
                        step_cursor: i,
                        step_id: step.id.0.clone(),
                        ir_digest: ctx.ir_digest.to_string(),
                        outputs: store.snapshot(),
                    })
                    .map_err(|e| RuntimeError::Internal { message: format!("persist suspension: {e}") })?;
                    return Ok(StepsFlow::Suspended {
                        step_id: step.id.0.clone(),
                        resume_signal: resume_signal.clone(),
                    });
                }
                // Nested slices pass `None` — typecheck guarantees no Suspend
                // appears here, so reaching this is an invariant violation.
                None => return Err(RuntimeError::SuspendUnsupported {
                    step: step.id.0.clone(),
                    resume_signal: resume_signal.clone(),
                }),
            }
        }
```

  - Every recursive `run_steps(...)` call inside the `Branch`/`Loop`/`Parallel` arms (lines 305, 330, 397) passes `None, 0` for the two new args and must handle the `StepsFlow` return:

```rust
        // Branch arm example (line 305):
        match Box::pin(run_steps(module, arm, input, store, dispatcher, None, None, 0)).await? {
            StepsFlow::Completed => {}
            StepsFlow::Suspended { .. } => {
                return Err(RuntimeError::Internal {
                    message: "suspend escaped a nested slice (typecheck should reject)".to_string(),
                })
            }
        }
```
Apply the same `match` to the Loop body call and the Parallel branch call. At the end of `run_steps` return `Ok(StepsFlow::Completed)` instead of `Ok(())`.

- [ ] **Step 7: Add `run_pipeline_suspendable`** and rewrite `run_pipeline` as a wrapper. Replace `run_pipeline` (`pipeline.rs:77`):

```rust
pub async fn run_pipeline_suspendable<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
    suspend: SuspendConfig,
    resume: Option<ResumeState>,
) -> Result<PipelineOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let pipeline = module.workflow.pipeline.clone().ok_or_else(|| RuntimeError::Internal {
        message: "run_pipeline called on a module without a pipeline".to_string(),
    })?;

    // Canonical-IR digest, computed once, for drift-detection on resume.
    let ir_digest = {
        let bytes = tau_ir::to_canonical_bytes(&module)
            .map_err(|e| RuntimeError::Internal { message: format!("canonical IR: {e}") })?;
        tau_ir::asset::asset_hash(&bytes)
    };

    let (mut store, start_at) = match resume {
        Some(r) => (r.store, r.start_at),
        None => (OutputStore::new(), 0),
    };

    let ctx = SuspendCtx { run_id: &suspend.run_id, ir_digest: &ir_digest, store: suspend.store.as_ref() };
    let flow = run_steps(&module, &pipeline.steps, &input, &mut store, &dispatcher, None, Some(&ctx), start_at).await?;
    Ok(match flow {
        StepsFlow::Completed => PipelineOutcome::Completed(store),
        StepsFlow::Suspended { step_id, resume_signal } =>
            PipelineOutcome::Suspended { run_id: suspend.run_id, resume_signal, step_id },
    })
}

/// Non-suspend convenience wrapper (existing callers/tests). Errors
/// `SuspendUnsupported` if the pipeline pauses.
pub async fn run_pipeline<D>(
    module: Arc<IrModule>,
    input: String,
    dispatcher: Arc<D>,
) -> Result<OutputStore, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    struct NoopSuspensions;
    impl tau_ports::orchestration::SuspensionStore for NoopSuspensions {
        fn persist_suspension(&self, _: &tau_ports::orchestration::PipelineSuspension) -> Result<(), tau_ports::orchestration::CheckpointError> { Ok(()) }
        fn load_suspension(&self, _: &tau_ports::orchestration::RunId) -> Result<Option<tau_ports::orchestration::PipelineSuspension>, tau_ports::orchestration::CheckpointError> { Ok(None) }
    }
    let outcome = run_pipeline_suspendable(
        module, input, dispatcher,
        SuspendConfig { run_id: String::new(), store: Arc::new(NoopSuspensions) },
        None,
    ).await?;
    match outcome {
        PipelineOutcome::Completed(store) => Ok(store),
        PipelineOutcome::Suspended { step_id, resume_signal, .. } =>
            Err(RuntimeError::SuspendUnsupported { step: step_id, resume_signal }),
    }
}
```

Update the doc comment on `run_pipeline` and add one on `run_pipeline_suspendable`. Update the stale `unreachable!("control-flow blocks are early-dispatched")` note at `pipeline.rs:528-537` so the `Suspend` arm text matches the new early-dispatch (it now returns `StepsFlow`, not an error).

- [ ] **Step 8: Update the retired-error test.** The old `suspend_returns_named_error` (pipeline_control_flow.rs:685) is replaced by `suspend_persists_and_returns_suspended` (Step 2). Also fix any other reference to `SuspendNotImplemented` (grep):

Run: `git grep -n SuspendNotImplemented` — expect zero hits after edits.

- [ ] **Step 9: Run tests + verify PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-rt cargo nextest run -p tau-runtime-core`
Expected: PASS.
Then doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-rt cargo test -p tau-runtime-core --doc`

- [ ] **Step 10: fmt + clippy + commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-rt cargo fmt -p tau-runtime-core -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-rt cargo clippy -p tau-runtime-core --all-targets -- -D warnings
git add crates/tau-runtime-core
git commit -m "feat(tau-runtime-core): pipeline suspend/resume (restore-and-continue) (EPIC 4.3)"
```

---

### Task 5: File-backed `SuspensionStore` (tau-runtime-tokio)

**Files:**
- Modify: `crates/tau-runtime-tokio/src/checkpoint.rs` (add `impl SuspensionStore for FileCheckpointStore`)
- Modify: `crates/tau-runtime-tokio/src/lib.rs` (re-export `SuspensionStore` if the crate re-exports the port surface — mirror `CheckpointStore`)
- Test: `checkpoint.rs` tests module

**Interfaces:**
- Consumes: `PipelineSuspension`, `SuspensionStore` (Task 1).
- Produces: `FileCheckpointStore: SuspensionStore`, file `<scope>/.tau/runs/<run_id>/suspend.json`.

- [ ] **Step 1: Write the failing test** in `checkpoint.rs` tests:

```rust
#[test]
fn suspension_round_trips_on_disk() {
    use tau_ports::orchestration::{PipelineSuspension, SuspensionStore};
    use std::collections::BTreeMap;
    let tmp = tempfile::tempdir().unwrap();
    let store = FileCheckpointStore::new(tmp.path());
    assert!(store.load_suspension(&"run-1".to_string()).unwrap().is_none());

    let mut outputs = BTreeMap::new();
    outputs.insert("seed".to_string(), serde_json::json!("GO"));
    let s = PipelineSuspension {
        run_id: "run-1".into(), resume_signal: "approved".into(),
        step_cursor: 1, step_id: "pause".into(),
        ir_digest: "sha256:abc".into(), outputs,
    };
    store.persist_suspension(&s).unwrap();

    let got = store.load_suspension(&"run-1".to_string()).unwrap().unwrap();
    assert_eq!(got, s);
    assert!(tmp.path().join(".tau/runs/run-1/suspend.json").exists());
}
```

- [ ] **Step 2: Run → FAIL**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-tokio cargo nextest run -p tau-runtime-tokio suspension_round_trips`
Expected: FAIL — `SuspensionStore` not implemented for `FileCheckpointStore`.

- [ ] **Step 3: Implement** in `checkpoint.rs` (after the `CheckpointStore` impl, ~line 96):

```rust
impl tau_ports::orchestration::SuspensionStore for FileCheckpointStore {
    fn persist_suspension(
        &self,
        s: &tau_ports::orchestration::PipelineSuspension,
    ) -> Result<(), CheckpointError> {
        let dir = self.run_dir(&s.run_id);
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        let json = serde_json::to_vec_pretty(s)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let final_path = dir.join("suspend.json");
        let tmp_path = dir.join("suspend.json.tmp");
        std::fs::write(&tmp_path, &json).map_err(io_err)?;
        std::fs::rename(&tmp_path, &final_path).map_err(io_err)?;
        Ok(())
    }

    fn load_suspension(
        &self,
        run_id: &RunId,
    ) -> Result<Option<tau_ports::orchestration::PipelineSuspension>, CheckpointError> {
        let path = self.run_dir(run_id).join("suspend.json");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(io_err)?;
        let s = serde_json::from_slice(&bytes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        Ok(Some(s))
    }
}
```

- [ ] **Step 4: Run → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-tokio cargo nextest run -p tau-runtime-tokio`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-tokio cargo fmt -p tau-runtime-tokio -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-tokio cargo clippy -p tau-runtime-tokio --all-targets -- -D warnings
git add crates/tau-runtime-tokio
git commit -m "feat(tau-runtime-tokio): FileCheckpointStore implements SuspensionStore (EPIC 4.3)"
```

---

### Task 6: CLI — `--signal`, suspend outcome, resume path (tau-cli)

**Files:**
- Modify: `crates/tau-cli/src/cli.rs:625` (add `--signal` to `RunArgs`)
- Modify: `crates/tau-cli/src/exit.rs` (add `ExitCode::Suspended`)
- Modify: `crates/tau-cli/src/cmd/run.rs:431` (`try_run_pipeline`: wire `SuspendConfig`, handle `PipelineOutcome`, resume branch) + `render_pipeline_result` (Suspended arm)
- Test: `crates/tau-cli/tests/` (add a suspend→resume integration test alongside existing `run` CLI tests)

**Interfaces:**
- Consumes: `run_pipeline_suspendable`, `PipelineOutcome`, `SuspendConfig`, `ResumeState` (Task 4); `FileCheckpointStore` as `SuspensionStore` (Task 5); `mint_run_id` (`run.rs:906`).
- Produces: `ExitCode::Suspended`; JSON `{"outcome":"suspended", "run_id":.., "resume_signal":.., "step_id":..}`.

- [ ] **Step 1: Add `--signal` flag** to `RunArgs` (`cli.rs:625`, beside `resume`):

```rust
    /// Signal name to resume a suspended pipeline run (with `--resume`).
    #[arg(long, value_name = "NAME")]
    pub signal: Option<String>,
```

- [ ] **Step 2: Add `ExitCode::Suspended`** in `exit.rs`. Add the variant, map it to process code `3` (verify `3` is unused on the `run` path — `exit.rs` today defines only 0/1/2):

```rust
    /// `tau run` only: a pipeline paused at a `Suspend` step (HITL). Not a
    /// failure — the run can be resumed with `--resume`.
    Suspended,
```
In `impl From<ExitCode> for std::process::ExitCode`: `ExitCode::Suspended => Self::from(3),`. (Leave the `From<&RunOutcome>` impl unchanged — suspension is pipeline-only, surfaced directly by the pipeline path, not via `RunOutcome`.)

- [ ] **Step 3: Write the failing CLI integration test** (new file `crates/tau-cli/tests/pipeline_suspend.rs` or add to an existing run test file). Use the crate's existing CLI-invocation harness (grep tests for how they build a temp project + call `tau run`):

```rust
// 1. Scaffold a temp project whose pipeline is: agent:seed -> suspend:approved(id "pause").
// 2. `tau run` (json): expect exit code 3 and {"outcome":"suspended","resume_signal":"approved",...}.
// 3. Assert <proj>/.tau/runs/<run_id>/suspend.json exists.
// 4. `tau run --resume <run_id> --signal approved` (json): expect exit 0 and {"outcome":"completed",...}.
// 5. `tau run --resume <run_id> --signal wrong`: expect a non-zero error exit (signal mismatch).
```

Write it concretely against the test harness in the crate (assert on captured stdout JSON + `assert_cmd`/status as the neighbouring tests do).

- [ ] **Step 4: Run → FAIL**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo nextest run -p tau-cli pipeline_suspend`
Expected: FAIL.

- [ ] **Step 5: Wire the pipeline path.** In `try_run_pipeline` (`run.rs:431`), thread the resume args in (the caller `run()` must pass `args.resume` and `args.signal` down — extend the `try_run_pipeline` signature). After building `dispatcher` (line 516), replace the `run_pipeline` call (lines 520-531) with:

```rust
    let project_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let suspensions: std::sync::Arc<dyn tau_ports::orchestration::SuspensionStore> =
        std::sync::Arc::new(tau_runtime_tokio::FileCheckpointStore::new(project_root.clone()));

    // Resume vs fresh.
    let module_arc = std::sync::Arc::new(module);
    let (run_id, resume) = match (&resume_id, &signal) {
        (Some(rid), Some(sig)) => {
            let susp = match suspensions.load_suspension(rid) {
                Ok(Some(s)) => s,
                Ok(None) => return Some(Err(anyhow::anyhow!("no suspended run {rid:?} to resume"))),
                Err(e) => return Some(Err(anyhow::Error::new(e).context("loading suspension"))),
            };
            if &susp.resume_signal != sig {
                return Some(Err(anyhow::anyhow!(
                    "signal {sig:?} does not match the suspended run's signal {:?}", susp.resume_signal
                )));
            }
            // Drift guard: the re-lowered module must match the pause-time IR.
            let digest = match tau_ir::to_canonical_bytes(&module_arc) {
                Ok(b) => tau_ir::asset::asset_hash(&b),
                Err(e) => return Some(Err(anyhow::Error::new(e).context("canonical IR"))),
            };
            if digest != susp.ir_digest {
                return Some(Err(anyhow::anyhow!(
                    "project changed since the run was suspended; cannot resume {rid:?}"
                )));
            }
            let start_at = susp.step_cursor + 1;
            (rid.clone(), Some(tau_runtime_core::interpreter::pipeline::ResumeState {
                store: tau_runtime_core::interpreter::output_store::OutputStore::restore(susp.outputs),
                start_at,
            }))
        }
        (Some(_), None) => return Some(Err(anyhow::anyhow!("--resume requires --signal <NAME>"))),
        (None, Some(_)) => return Some(Err(anyhow::anyhow!("--signal is only valid with --resume"))),
        (None, None) => (crate::cmd::run::mint_run_id(), None),
    };

    let outcome = match tau_runtime_core::interpreter::pipeline::run_pipeline_suspendable(
        module_arc, prompt_text, dispatcher,
        tau_runtime_core::interpreter::pipeline::SuspendConfig { run_id: run_id.clone(), store: suspensions },
        resume,
    ).await {
        Ok(o) => o,
        Err(e) => return Some(Err(anyhow::Error::new(e).context("running pipeline"))),
    };

    Some(render_pipeline_outcome(outcome, &last_step_id, output))
```

(`resume_id` / `signal` are the new `try_run_pipeline` params sourced from `args.resume` / `args.signal`.)

- [ ] **Step 6: Add `render_pipeline_outcome`** beside `render_pipeline_result` (`run.rs:550`). Keep `render_pipeline_result` for the `Completed` store rendering; the new fn dispatches:

```rust
pub(super) fn render_pipeline_outcome(
    outcome: tau_runtime_core::interpreter::pipeline::PipelineOutcome,
    last_step_id: &str,
    output: &mut Output,
) -> anyhow::Result<()> {
    use tau_runtime_core::interpreter::pipeline::PipelineOutcome;
    match outcome {
        PipelineOutcome::Completed(store) => render_pipeline_result(&store, last_step_id, output),
        PipelineOutcome::Suspended { run_id, resume_signal, step_id } => {
            if output.is_json() {
                output.json(&serde_json::json!({
                    "outcome": "suspended",
                    "run_id": run_id,
                    "resume_signal": resume_signal,
                    "step_id": step_id,
                }))?;
            } else {
                output.human(&format!(
                    "Paused at step '{step_id}' (signal: {resume_signal}).\n\
                     Resume with:  tau run --resume {run_id} --signal {resume_signal}"
                ))?;
            }
            // Signal the suspended exit code to the caller.
            Err(SuspendedRun { }.into())
        }
    }
}
```

Because the `run()` outer flow maps `anyhow::Error` → `ExitCode::Error` (2), introduce a typed sentinel so suspension maps to exit 3. Add a small error type and teach `run_main`/exit mapping to detect it:

```rust
// run.rs
#[derive(Debug, thiserror::Error)]
#[error("pipeline suspended")]
pub(crate) struct SuspendedRun;
```
In the top-level exit mapping (where `anyhow::Error` → `ExitCode`), downcast: `if err.downcast_ref::<SuspendedRun>().is_some() { ExitCode::Suspended } else { ExitCode::Error }`. (Locate the mapping in `crates/tau-cli/src/exit.rs` `From<&anyhow::Error>` or the `run_main` glue; add the downcast there.)

- [ ] **Step 7: Pass `args.resume` / `args.signal`** from `run()` into `try_run_pipeline` (extend its call at `run.rs:286`). The single-agent `--resume` handling on the bundle path (`ir_dispatcher.rs`) is unchanged.

- [ ] **Step 8: Run tests → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo nextest run -p tau-cli`
Expected: PASS.

- [ ] **Step 9: fmt + clippy + commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-cli cargo fmt -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-cli cargo clippy -p tau-cli --all-targets -- -D warnings
git add crates/tau-cli
git commit -m "feat(tau-cli): tau run suspend outcome + --resume --signal + exit code 3 (EPIC 4.3)"
```

---

### Task 7: Conformance fixture (tau-ir-conformance)

**Files:**
- Modify: `crates/tau-ir-conformance/src/` (report shape gains a "suspended" terminal comparison)
- Create: a suspend fixture under the crate's fixtures dir (mirror an existing pipeline fixture)
- Test: the crate's conformance test entrypoint

**Interfaces:**
- Consumes: `run_pipeline_suspendable`, `PipelineOutcome` (Task 4); `MockSuspensionStore` (Task 1).

- [ ] **Step 1: Inspect the current report shape.** Read the crate's `ConformanceReport` / driver (grep `ConformanceReport`, `run_pipeline`). Identify where a dev-run and a bundle-run are compared for equality.

- [ ] **Step 2: Write the failing fixture test.** Add a fixture whose pipeline is `agent:seed -> suspend:approved(pause)` and a test asserting dev-run and bundle-run agree at the pause point:

```rust
#[test]
fn suspend_fixture_dev_and_bundle_agree_at_pause() {
    // Drive the fixture dev-side and bundle-side, each with a MockSuspensionStore,
    // run_id "conf". Both must return PipelineOutcome::Suspended with the same
    // resume_signal + step_id, and the persisted PipelineSuspension must have the
    // same step_cursor + outputs snapshot.
    let dev = run_fixture_dev("suspend");   // -> (PipelineOutcome, PipelineSuspension)
    let bundle = run_fixture_bundle("suspend");
    assert_eq!(dev.suspension.resume_signal, bundle.suspension.resume_signal);
    assert_eq!(dev.suspension.step_cursor, bundle.suspension.step_cursor);
    assert_eq!(dev.suspension.outputs, bundle.suspension.outputs);
}
```

- [ ] **Step 3: Run → FAIL**, then extend the report/driver so a suspended run is a first-class terminal state compared across dev vs bundle (add a `Suspended { resume_signal, step_cursor, outputs }` comparison branch rather than forcing `Completed`).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance suspend`
Expected: FAIL then (after impl) PASS.

- [ ] **Step 4: Run full crate → PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo nextest run -p tau-ir-conformance`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-conf cargo fmt -p tau-ir-conformance -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-conf cargo clippy -p tau-ir-conformance --all-targets -- -D warnings
git add crates/tau-ir-conformance
git commit -m "test(tau-ir-conformance): suspend fixture dev-vs-bundle pause equivalence (EPIC 4.3)"
```

---

### Task 8: Docs + cross-crate green

**Files:**
- Modify: relevant `docs/` reference page for pipeline authoring (grep `SUMMARY.md` for the pipeline/control-flow page that documents Branch/Loop/Parallel) — add the `suspend:` leaf + `--resume`/`--signal` usage.
- Modify: an ADR if the EPIC 4.x series uses one per step (check `docs/` for ADR-0058/0059 pattern; add ADR-00xx for suspend if the series expects it).

- [ ] **Step 1: Document the authoring surface** — add a `run = "suspend:<signal>"` example, the top-level-only rule, and the `tau run --resume <id> --signal <name>` resume flow to the pipeline docs page. Ensure the page is in `docs/SUMMARY.md` (per DOCS RULES).

- [ ] **Step 2: Build the book** (per DOCS RULES):

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` then `rm -rf docs/book`
Expected: only `[INFO]` lines.

- [ ] **Step 3: Cross-crate sanity** — build every touched crate together via each isolated target dir already used above (no `--workspace`). Confirm `git grep -n SuspendNotImplemented` is empty and `git grep -n 'run_pipeline\b'` callers all compile.

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "docs: document pipeline suspend authoring + resume (EPIC 4.3)"
```

---

## Self-Review

**Spec coverage:**
- Authoring surface → Task 2. Lowering → Task 3. Typecheck (nested-reject + no-output-ref) → Task 3. Interpreter pause+resume (restore-and-continue) → Task 4. `PipelineOutcome` → Task 4. `SuspensionStore` + `PipelineSuspension` + IR digest → Task 1 (+ digest use Task 4). File store → Task 5. CLI `--signal`/`--resume`/exit code → Task 6. Conformance fixture → Task 7. Version bump → Task 1 (Global Constraints). Docs → Task 8. All spec sections covered.

**Placeholder scan:** No TBD/"handle appropriately". Every code step has concrete code. Two spec-acknowledged open items are resolved in the plan: `ExitCode::Suspended = 3` (Task 6 Step 2, with a verify note) and bundle-path suspend is explicitly deferred (`run_pipeline` wrapper errors `SuspendUnsupported` on the bundle path; only `try_run_pipeline` wires HITL).

**Type consistency:** `PipelineSuspension` fields (`run_id, resume_signal, step_cursor, step_id, ir_digest, outputs`) are identical across Tasks 1/4/5/6/7. `PipelineOutcome::Suspended { run_id, resume_signal, step_id }` identical in Tasks 4/6. `SuspendConfig { run_id, store }` and `ResumeState { store, start_at }` identical in Tasks 4/6. `run_pipeline_suspendable` signature identical in Tasks 4/6/7. `SuspendUnsupported { step, resume_signal }` identical in Tasks 4/6. `ir_digest` computed the same way (`asset_hash(&to_canonical_bytes(module))`) in Tasks 4 and 6.

**Known verify-at-execution points (not placeholders):**
- `serde_json` availability in tau-ports (Task 1 Step 3) — confirm it's a non-optional dep or gate the field.
- `ExitCode::Suspended` numeric `3` unused elsewhere on the `run` path (Task 6 Step 2).
- Exact test-harness helpers in each crate's existing test modules (builders, CLI invocation) — reuse, don't reinvent.
