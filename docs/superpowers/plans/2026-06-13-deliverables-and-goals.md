# Deliverables & Goals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two build-time-checked postcondition primitives — `goal` (deterministic predicate) and `deliverable` (produced artifact + swappable LLM judge) — to the `tau.toml` → IR path, with an opt-in rewind-to-gate retry loop.

**Architecture:** A check is a new IR `Check` node stored in `workflow.checks` and positioned in the Plan-1 pipeline by a new `StepRun::Check(CheckId)` variant. Build-time lowering proves producer binding, gate position, and span non-determinism. The pipeline executor (`run_pipeline`) gains a richer `PipelineOutcome`, a trusted engine-side `read_artifact` capability on the dispatcher, deterministic predicate evaluation, an LLM-judge invocation returning `{met, rationale}`, and a bounded rewind-to-gate retry. Runtime evaluation is wired to `tau run` (local) only in v1; build-time checks are universal.

**Tech Stack:** Rust (workspace, `no_std`+`alloc` for `tau-ir`/`tau-runtime-core`), serde/serde_json, thiserror, tracing, `regex` (new dep, predicate menu), tokio (CLI host). Authoritative design: `docs/superpowers/specs/2026-06-13-deliverables-and-goals-design.md` (read the *Reconciliation with Plan 1* section first — D1–D7 are locked).

---

## Conventions (every cargo + commit step)

Per `CLAUDE.md` CARGO RULES — never run bare cargo. Use this shape for the role `impl`:

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate>
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p <crate>
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p <crate>
```

Doctests still use `cargo test --doc` (nextest doctest support is incomplete):
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p <crate>
```

`tau-ir` is `#![no_std]` + `alloc` + `#![deny(missing_docs)]` — every new public item needs a doc comment, and use `alloc::` paths (`String`, `Vec`, `BTreeMap`, `format!`), not `std`.

Commits (the pre-commit hook trips on a pre-existing `tau-pkg` echo-tool fixture failure; `--no-verify` is sanctioned **after** your crate's nextest run is green):
```
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "<conventional message>"
```

**PR order is sequential: B → A → C1 → C2 → D.** Each PR is its own branch off `main` and its own GitHub PR. PR-A and PR-B touch disjoint crates and may overlap if desired; everything from C1 onward needs both merged.

---

## File map

| File | PR | Responsibility |
|---|---|---|
| `crates/tau-runtime-core/src/outcome.rs` | B | add `PipelineOutcome` + `PipelineStatus` |
| `crates/tau-runtime-core/src/interpreter/pipeline.rs` | B,C1,C2 | return `PipelineOutcome`; check eval; retry loop |
| `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs` | B | add `read_artifact` trait method (default `None`) |
| `crates/tau-cli/src/cmd/ir_dispatcher.rs` | B,D | impl `read_artifact` (host fs); call site update |
| `crates/tau-ir/src/check.rs` *(new)* | A | `Check`, `CheckVerify`, `Locus`, `Predicate`, `JudgeRef`, `Retry`, `OnFail` |
| `crates/tau-ir/src/ids.rs` | A | add `CheckId` |
| `crates/tau-ir/src/pipeline.rs` | A | add `StepRun::Check(CheckId)` |
| `crates/tau-ir/src/module.rs` | A | add `Workflow.checks`; bump `IrFormatVersion` to `v1.2.0` |
| `crates/tau-ir/src/error.rs` | A | new `IrError` variants |
| `crates/tau-ir/src/lib.rs` | A | export check types + `CheckId` |
| `crates/tau-ir/src/lower/parse.rs` | A | populate `checks`; thread `produces` |
| `crates/tau-ir/src/lower/typecheck.rs` | A | check refs, loci, gate (G1), span non-determinism (G2), span overlap (D7) |
| `crates/tau-ir/src/lower/capability_fit.rs` | A | producer binding + fs-write coverage cross-check |
| `crates/tau-pkg/src/project/project.rs` | A | `[goals.*]`/`[deliverables.*]` tables + agent `produces`; validation |
| `crates/tau-ts-extract/src/{factory,lower}.rs` | A | `goals(...)`/`deliverables(...)` TS factories + TOML emission |
| `crates/tau-runtime-core/src/interpreter/verdict.rs` *(new)* | C2 | `Verdict { met, rationale }` + parse helper |
| `crates/tau-observe/src/vocabulary.rs` | C1,C2 | `EV_CHECK_EVALUATED`, `EV_CHECK_RETRY`, `SPAN_CHECK` (canonical) |
| `crates/tau-runtime-core/src/vocabulary.rs` | C1,C2 | mirror constants |
| `crates/tau-runtime-tokio/tests/*` | C1,C2 | vocabulary drift test update |
| `crates/tau-cli/src/cmd/run.rs` | D | thread `PipelineOutcome` through `try_run_pipeline` |
| `crates/tau-ir-conformance/fixtures/09_*`, `tests/conformance.rs` | A,C1,C2 | fixtures + cross-mode tests |

---

# PR-B — Runtime substrate (D1 `PipelineOutcome`/#5, D3 `read_artifact`)

**Branch:** `feat/checks-runtime-substrate`. No checks yet — pure plumbing the retry loop will need. Crate: `tau-runtime-core` (+ `tau-cli` for the dispatcher impl).

### Task B1: `PipelineOutcome` + `PipelineStatus` types

**Files:**
- Modify: `crates/tau-runtime-core/src/outcome.rs`

- [ ] **Step 1: Add the types after `RunOutcome`.** Append to `outcome.rs`:

```rust
/// Outcome of a `run_pipeline` call (D1).
///
/// Richer than the bare `OutputStore` Plan 1 returned: it carries the
/// step outputs, aggregated token usage across every step *and retry
/// attempt*, and a terminal status that distinguishes an agent-level
/// failure (ADR-0006) and a check abort from a clean completion. Kernel
/// / dispatch errors remain `Err(RuntimeError)` and never appear here.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// Every pipeline step's output, keyed by pipeline-step id.
    pub outputs: crate::interpreter::output_store::OutputStore,
    /// Token usage summed across all steps and all retry attempts.
    pub token_usage: TokenUsage,
    /// How the pipeline terminated.
    pub status: PipelineStatus,
}

/// Terminal status of a pipeline run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineStatus {
    /// Every step (and any checks) passed.
    Completed,
    /// An agent step failed via a typed `AgentStatus::Failed` (ADR-0006).
    /// This is NOT a kernel error.
    AgentFailed {
        /// The pipeline-step id that failed.
        step: alloc::string::String,
        /// The agent's typed failure status.
        status: tau_domain::AgentStatus,
    },
    /// A check failed and its `on_fail` policy (or exhausted retries)
    /// aborted the run.
    CheckAborted {
        /// The check id that aborted the run.
        check: alloc::string::String,
        /// The final verdict rationale / diagnostic.
        rationale: alloc::string::String,
    },
}
```

- [ ] **Step 2: Add a `TokenUsage` sum helper** (the retry loop accumulates per-attempt usage). In `crates/tau-runtime-core/src/options.rs`, find `TokenUsage` and add:

```rust
impl TokenUsage {
    /// Accumulate another usage into this one (saturating).
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = match (self.total_tokens, other.total_tokens) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (a, b) => a.or(b),
        };
    }
}
```
(Read `options.rs` first to confirm `TokenUsage`'s exact field types — the test in `outcome.rs:110` shows `input_tokens`/`output_tokens`/`total_tokens: Option<_>`.)

- [ ] **Step 3: Build the crate.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core`
Expected: PASS.

- [ ] **Step 4: Add a unit test** in `outcome.rs`'s `tests` module:

```rust
#[test]
fn token_usage_add_saturates_and_sums() {
    let mut a = TokenUsage { input_tokens: 10, output_tokens: 5, total_tokens: Some(15) };
    a.add(&TokenUsage { input_tokens: 1, output_tokens: 2, total_tokens: Some(3) });
    assert_eq!(a.input_tokens, 11);
    assert_eq!(a.output_tokens, 7);
    assert_eq!(a.total_tokens, Some(18));
}
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core token_usage_add`
Expected: PASS.

- [ ] **Step 5: Commit.**
```
git add crates/tau-runtime-core/src/outcome.rs crates/tau-runtime-core/src/options.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime-core): add PipelineOutcome/PipelineStatus + TokenUsage::add (D1)"
```

### Task B2: `run_pipeline` returns `PipelineOutcome`; stop conflating agent-failure with kernel-error (#5)

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/pipeline.rs`

- [ ] **Step 1: Change the signature and accumulate usage.** In `pipeline.rs`, change the return type from `Result<OutputStore, RuntimeError>` to `Result<PipelineOutcome, RuntimeError>` and import `use crate::outcome::{PipelineOutcome, PipelineStatus, RunOutcome};` and `use crate::options::TokenUsage;`. Add `let mut total_usage = TokenUsage::default();` next to `let mut store = OutputStore::new();`.

- [ ] **Step 2: Replace the agent-step `Failed` arm** (currently maps to `RuntimeError::Internal`). The match in the `StepRun::Agent` arm becomes:

```rust
match outcome {
    RunOutcome::Failed { status, token_usage, .. } => {
        total_usage.add(&token_usage);
        return Ok(PipelineOutcome {
            outputs: store,
            token_usage: total_usage,
            status: PipelineStatus::AgentFailed { step: step.id.0.clone(), status },
        });
    }
    RunOutcome::Completed { token_usage, .. } => {
        total_usage.add(&token_usage);
        Value::String(last_assistant_text(&outcome))
    }
}
```
Note: `last_assistant_text(&outcome)` borrows `outcome`, so bind `token_usage` by destructuring in the match arm BEFORE the `Completed` body consumes it — restructure to extract `token_usage` and keep `outcome` for `last_assistant_text` (clone the usage out: `RunOutcome::Completed { token_usage, .. } => { total_usage.add(token_usage); Value::String(last_assistant_text(&outcome)) }` requires `token_usage` borrowed — match on `&outcome` or read fields by ref). Implement by matching `&outcome`:

```rust
let output: Value = match &outcome {
    RunOutcome::Failed { status, token_usage, .. } => {
        total_usage.add(token_usage);
        return Ok(PipelineOutcome {
            outputs: store,
            token_usage: total_usage,
            status: PipelineStatus::AgentFailed { step: step.id.0.clone(), status: status.clone() },
        });
    }
    RunOutcome::Completed { token_usage, .. } => {
        total_usage.add(token_usage);
        Value::String(last_assistant_text(&outcome))
    }
};
```

- [ ] **Step 3: Update the final return.** Replace `Ok(store)` with:
```rust
Ok(PipelineOutcome { outputs: store, token_usage: total_usage, status: PipelineStatus::Completed })
```

- [ ] **Step 4: Fix all callers.** Build the workspace to find them:

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core -p tau-cli`
Expected: compile errors at `run_pipeline` call sites (`tau-cli/src/cmd/run.rs` `try_run_pipeline`, any conformance harness). For `tau-cli`, the caller currently treats the `Ok` as an `OutputStore`; change it to read `.outputs` and to surface `.status` (full wiring is PR-D Task D1 — for now, minimally adapt: `let outcome = run_pipeline(...).await?; let store = outcome.outputs;` and ignore status to keep behavior). Leave a `// TODO(PR-D): surface PipelineStatus` marker.

- [ ] **Step 5: Update existing pipeline tests** in `tau-runtime-core` / `tau-ir-conformance` that asserted on `OutputStore` to read `.outputs`. Run the crate tests:

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS.

- [ ] **Step 6: Commit.**
```
git add crates/tau-runtime-core/src/interpreter/pipeline.rs crates/tau-cli/src/cmd/run.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "refactor(runtime-core): run_pipeline returns PipelineOutcome; surface agent-failure not Internal (#5)"
```

### Task B3: `read_artifact` dispatcher capability (trusted engine read, D3)

**Files:**
- Modify: `crates/tau-runtime-core/src/interpreter/tool_dispatch.rs`
- Modify: `crates/tau-cli/src/cmd/ir_dispatcher.rs`

- [ ] **Step 1: Add the trait method with a default.** Read `tool_dispatch.rs` to match the existing optional-method style (`clock`, `random`, `deterministic_registry` all return `Option<...>` with a default `{ None }`). Add to the `ToolDispatcher` trait:

```rust
/// Read a deliverable/goal artifact by locus for engine-side check
/// evaluation (D3). Trusted-kernel: the path was producer-capability-
/// checked at build time, so this is not capability-gated.
///
/// Returns `None` if the dispatcher provides no host filesystem (core /
/// test dispatchers). `Some(Ok(None))` means "no such artifact"
/// (existence-floor failure); `Some(Ok(Some(bytes)))` is the content.
fn read_artifact(&self, _path: &str) -> Option<Result<Option<alloc::vec::Vec<u8>>, RuntimeError>> {
    None
}
```

- [ ] **Step 2: Implement it in `ForwardingDispatcher`.** In `ir_dispatcher.rs` (which already returns `Some(TokioClock)`/`Some(OsRandom)`), add:

```rust
fn read_artifact(&self, path: &str) -> Option<Result<Option<Vec<u8>>, RuntimeError>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(Ok(Some(bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Ok(None)),
        Err(e) => Some(Err(RuntimeError::Internal {
            message: format!("read_artifact({path}): {e}"),
        })),
    }
}
```
(Confirm `RuntimeError::Internal { message }` is the right shape by reading `crates/tau-runtime-core/src/error.rs`.)

- [ ] **Step 3: Build both crates.**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core -p tau-cli`
Expected: PASS.

- [ ] **Step 4: Unit-test the host impl.** Add a test in `ir_dispatcher.rs` (gated `#[cfg(test)]`) that writes a temp file and asserts `read_artifact` returns its bytes, and returns `Ok(None)` for a missing path. Use `tempfile` (already a dev-dep in tau-cli — confirm in `Cargo.toml`; if absent, use `std::env::temp_dir()` + a unique name).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli read_artifact`
Expected: PASS.

- [ ] **Step 5: Commit.**
```
git add crates/tau-runtime-core/src/interpreter/tool_dispatch.rs crates/tau-cli/src/cmd/ir_dispatcher.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime): trusted read_artifact dispatcher capability (D3)"
```

- [ ] **Step 6: clippy + push + open PR-B.**
```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core -p tau-cli
git push -u origin feat/checks-runtime-substrate
gh pr create --base main --title "feat(runtime): PipelineOutcome + read_artifact substrate for checks (D1/D3/#5)" --body "PR-B of the deliverables-and-goals plan. Adds PipelineOutcome/PipelineStatus, fixes the ADR-0006 agent-failure conflation (#5), and adds the trusted read_artifact dispatcher capability. No checks yet.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge <PR#> --squash --delete-branch --auto
```

---

# PR-A — IR foundation (D2 Check node, D7 span overlap, Guarantees 1/2)

**Branch:** `feat/checks-ir-foundation` (off `main`). Build-time only — fully testable without runtime. Crates: `tau-ir`, `tau-pkg`, `tau-ts-extract`, `tau-ir-conformance`.

### Task A1: `CheckId` newtype

**Files:** Modify `crates/tau-ir/src/ids.rs`

- [ ] **Step 1:** Append (mirroring `PipelineStepId`):
```rust
/// Identifier for a postcondition [`Check`](crate::check::Check). Referenced
/// from the pipeline by [`StepRun::Check`](crate::pipeline::StepRun::Check).
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct CheckId(pub String);
```
- [ ] **Step 2: Build.** `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir` → PASS.

### Task A2: the `Check` node module

**Files:** Create `crates/tau-ir/src/check.rs`; modify `crates/tau-ir/src/lib.rs`

- [ ] **Step 1: Write `check.rs`** (`no_std`/`alloc`, `deny(missing_docs)` — doc every item):

```rust
//! Postcondition check IR (D2). A `Check` is defined in
//! `workflow.checks` and positioned in the pipeline by
//! [`StepRun::Check`](crate::pipeline::StepRun::Check). Two kinds:
//! `goal` (deterministic predicate) and `deliverable` (existence floor +
//! LLM judge of content).

use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AgentId, CheckId, PipelineStepId};

/// A postcondition check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// This check's id.
    pub id: CheckId,
    /// What is verified and how.
    pub verify: CheckVerify,
    /// Failure handling. `None` => abort on failure (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
}

/// What a check asserts and how it is verified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckVerify {
    /// A measurable condition, verified deterministically (no LLM).
    Goal {
        /// The read locus the predicate inspects.
        evaluates: Locus,
        /// The predicate.
        predicate: Predicate,
    },
    /// A produced artifact whose content an LLM judges.
    Deliverable {
        /// Where the artifact lives.
        locus: Locus,
        /// Natural-language acceptance criterion fed to the judge.
        must_satisfy: String,
        /// Who judges.
        judge: JudgeRef,
    },
}

/// What a check inspects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locus {
    /// A filesystem path (read via the engine's trusted `read_artifact`).
    Path(String),
    /// A named pipeline-step output (`steps.<id>.output`).
    Output(PipelineStepId),
}

/// Deterministic predicate menu + native-fn escape hatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    /// The locus exists.
    Exists,
    /// The locus exists and is non-empty.
    NonEmpty,
    /// Equals a literal string.
    Equals(String),
    /// Matches a regular expression.
    Matches(String),
    /// At least N matches of the regex / N items.
    MinCount {
        /// The regex whose matches are counted.
        pattern: String,
        /// Required minimum.
        min: u64,
    },
    /// Validates against a JSON schema.
    SchemaValid(Value),
    /// `<crate>::<path>` registered in the `DeterministicRegistry`.
    NativeFn(String),
}

/// Who judges a deliverable's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JudgeRef {
    /// tau's built-in minimalist judge; `model` overrides the default.
    Builtin {
        /// Optional model override (the `judge_model` author field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A user `[agents.*]` judge.
    Agent(AgentId),
}

/// Failure handling for a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retry {
    /// Abort or rewind-and-retry.
    pub on_fail: OnFail,
    /// Maximum attempts (inclusive of the first).
    pub max_attempts: u32,
    /// Rewind point. Lowering resolves this to a concrete pipeline-step
    /// id (defaults to the bound producer). Validated `<=` producer (G1)
    /// with a non-deterministic step in the span (G2).
    pub gate: PipelineStepId,
    /// The resolved producer step (the step that writes the locus).
    pub producer: PipelineStepId,
}

/// Failure policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnFail {
    /// Exit non-zero with the rationale (default).
    Abort,
    /// Rewind to `gate` and re-run forward, feeding back the rationale.
    Retry,
}
```

- [ ] **Step 2: Wire into `lib.rs`.** Add `pub mod check;` in the module list and append to the re-exports:
```rust
pub use check::{Check, CheckVerify, JudgeRef, Locus, OnFail, Predicate, Retry};
pub use ids::CheckId;
```
(Read `lib.rs` first; add `CheckId` to the existing `pub use ids::{...}` line rather than a new line if one exists.)

- [ ] **Step 3: Round-trip test** in `check.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_serde_round_trips() {
        let c = Check {
            id: CheckId("report".into()),
            verify: CheckVerify::Deliverable {
                locus: Locus::Path("/workspace/report.md".into()),
                must_satisfy: "coherent".into(),
                judge: JudgeRef::Builtin { model: None },
            },
            retry: Some(Retry {
                on_fail: OnFail::Retry, max_attempts: 3,
                gate: PipelineStepId("writer".into()),
                producer: PipelineStepId("writer".into()),
            }),
        };
        let b = serde_json::to_vec(&c).unwrap();
        assert_eq!(c, serde_json::from_slice::<Check>(&b).unwrap());
    }
}
```
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir check_serde_round_trips` → PASS.
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-ir` → PASS (missing-docs gate).

- [ ] **Step 4: Commit.**
```
git add crates/tau-ir/src/ids.rs crates/tau-ir/src/check.rs crates/tau-ir/src/lib.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ir): Check node + CheckId (D2)"
```

### Task A3: `StepRun::Check` + `Workflow.checks` + `ir_format` bump

**Files:** Modify `crates/tau-ir/src/pipeline.rs`, `crates/tau-ir/src/module.rs`

- [ ] **Step 1: Add the variant** to `StepRun` in `pipeline.rs`:
```rust
    /// Evaluate a postcondition check by id. The referenced
    /// [`CheckId`](crate::ids::CheckId) must exist in `workflow.checks`
    /// (enforced by typecheck).
    Check(crate::ids::CheckId),
```
Add `use crate::ids::{AgentId, StepId, ToolId};` already imports ids — extend with `CheckId` if you reference it unqualified; the fully-qualified `crate::ids::CheckId` above avoids touching the import.

- [ ] **Step 2: Add the `checks` field** to `Workflow` in `module.rs` (after `pipeline`):
```rust
    /// Postcondition checks by id, positioned in the pipeline via
    /// [`StepRun::Check`](crate::pipeline::StepRun::Check). `#[serde(default)]`
    /// keeps pre-`v1.2.0` modules (no checks) deserializable; the
    /// skip keeps check-free modules byte-identical save the format bump.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<crate::ids::CheckId, crate::check::Check>,
```

- [ ] **Step 3: Bump the format version** in `module.rs`:
```rust
    pub const CURRENT: &'static str = "v1.2.0";
```

- [ ] **Step 4: Fix every `Workflow { .. }` struct literal.** Build to find them (tests in `typecheck.rs`, `agent_loop.rs`, conformance, etc. construct `Workflow` literally and will now miss `checks`):

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ir`
Expected: errors `missing field checks`. Add `checks: BTreeMap::new(),` to each literal. (Plan-1 `agent_loop.rs:613` constructs a stub `Workflow` — that's in `tau-runtime-core`, fixed in PR-C; for PR-A only `tau-ir` must compile.)

- [ ] **Step 5: Test the variant + field exist & serialize.** In `pipeline.rs` tests add a `StepRun::Check` round-trip; confirm `Workflow::default()` has empty `checks` that skips serialization (assert the canonical bytes of a check-free `Workflow` don't contain `"checks"`).

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir`
Expected: PASS.

- [ ] **Step 6: Commit.**
```
git add crates/tau-ir/src/pipeline.rs crates/tau-ir/src/module.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ir): StepRun::Check + Workflow.checks; bump ir_format v1.2.0 (D2)"
```

### Task A4: project-config surface (`[goals.*]`, `[deliverables.*]`, agent `produces`)

**Files:** Modify `crates/tau-pkg/src/project/project.rs`

Read `project.rs` first to confirm the exact current shapes of `UncheckedProjectConfig`, `UncheckedAgent`, `AgentEntry`, `ProjectConfig`, and `validate()` (the tau-pkg explorer report describes them; verify line numbers before editing).

- [ ] **Step 1: Add `produces` to the agent config.** In `UncheckedAgent` add `#[serde(default)] pub produces: Vec<String>,`; in `AgentEntry` add `pub produces: Vec<String>,`; thread it through `validate_agent` (copy `raw.produces`).

- [ ] **Step 2: Add unchecked tables.** Add to `UncheckedProjectConfig`:
```rust
    #[serde(default)]
    pub goals: BTreeMap<String, UncheckedGoal>,
    #[serde(default)]
    pub deliverables: BTreeMap<String, UncheckedDeliverable>,
```
And the structs (match the spec's authoring surface):
```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedGoal {
    pub evaluates: String,                 // path or "steps.<id>.output"
    pub check: String,                     // "exists"|"non_empty"|"equals"|"matches"|"min_count"|"schema_valid"
    #[serde(default)] pub pattern: Option<String>,
    #[serde(default)] pub equals: Option<String>,
    #[serde(default)] pub min: Option<u64>,
    #[serde(default)] pub schema: Option<serde_json::Value>,
    #[serde(default, rename = "fn")] pub native_fn: Option<String>,
    #[serde(default)] pub on_fail: Option<String>,     // "abort"|"retry"
    #[serde(default)] pub max_attempts: Option<u32>,
    #[serde(default)] pub retry_from: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedDeliverable {
    #[serde(default)] pub path: Option<String>,
    #[serde(default)] pub output: Option<String>,      // "steps.<id>.output"
    pub must_satisfy: String,
    #[serde(default)] pub judge: Option<String>,        // agent id
    #[serde(default)] pub judge_model: Option<String>,
    #[serde(default)] pub on_fail: Option<String>,
    #[serde(default)] pub max_attempts: Option<u32>,
    #[serde(default)] pub retry_from: Option<String>,
}
```

- [ ] **Step 3: Add validated forms** to `ProjectConfig` (`goals: BTreeMap<String, GoalConfig>`, `deliverables: BTreeMap<String, DeliverableConfig>`) and `#[non_exhaustive]` config structs that normalize the raw fields (parse `check` into an enum, reject `equals`/`pattern`/`min`/`schema`/`fn` that don't match the chosen `check`, reject a deliverable with both `path` and `output` or neither, reject `judge` + `judge_model` together). Add `validate_goals`/`validate_deliverables` and call them in `validate()`. Mirror the existing `validate_pipeline` error style (`ProjectConfigError::PipelineValidation` → add `GoalValidation`/`DeliverableValidation` variants to `ProjectConfigError`).

- [ ] **Step 4: Tests** in `project.rs`: a `[deliverables.report]` with both `path` and `output` is rejected; `judge` + `judge_model` together is rejected; a valid goal with `check = "matches"` + `pattern` parses; a `matches` goal *without* `pattern` is rejected.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg project::`
Expected: PASS.

- [ ] **Step 5: Commit.**
```
git add crates/tau-pkg/src/project/project.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(pkg): [goals]/[deliverables] tables + agent produces (D2)"
```

### Task A5: lowering — populate `checks`, thread `produces`

**Files:** Modify `crates/tau-ir/src/lower/parse.rs`

- [ ] **Step 1: Thread `produces`.** The IR `Agent` node has no `produces` field — producer binding is computed during typecheck/capability-fit from the project config, which `parse` does not retain. Decision: carry `produces` into the `Parsed` value by extending `Agent`? No — `Agent` is canonical IR and `produces` is build-time-only metadata. Instead, add a `produces: BTreeMap<AgentId, Vec<String>>` side-table to the `Parsed` struct (it is `pub(super)`, not serialized). Populate it in the agents loop from `entry.produces`.

```rust
// in Parsed:
pub(super) produces: alloc::collections::BTreeMap<AgentId, alloc::vec::Vec<alloc::string::String>>,
// in the agents loop, after inserting the Agent:
produces_map.insert(agent_id_clone, entry.produces.clone());
```

- [ ] **Step 2: Build the `checks` map.** After the pipeline block, lower `config.goals` and `config.deliverables` into `BTreeMap<CheckId, Check>`. Map each `GoalConfig`/`DeliverableConfig` to a `Check` with `verify` set, and `retry: None` for now (gate/producer resolution happens in typecheck — see A6, where it mutates the check or where parse leaves a placeholder). Cleaner: parse builds `Check` with a *pre-resolution* `retry` carrying the author's `retry_from`/`on_fail`/`max_attempts` as raw strings, and typecheck resolves+validates into the final `Retry { gate, producer }`. To keep the IR type clean, store the raw retry intent in the `Parsed` side-table too:

```rust
pub(super) retry_intent: BTreeMap<CheckId, RawRetry>, // { on_fail, max_attempts, retry_from: Option<String> }
```
and leave `Check.retry = None` at parse; A6 fills it.

Map `evaluates`/`path`/`output` strings to `Locus`: a value starting `steps.` and ending `.output` → `Locus::Output(PipelineStepId(id))`; otherwise `Locus::Path(s)`.

- [ ] **Step 3: Update the `Parsed { workflow: Workflow { .. } }` literal** to include `checks` and the new side-tables.

- [ ] **Step 4: Test** lowering produces the checks: a TOML with a `[goals.has_sources]` (`check = "matches"`, `pattern`) lowers to a `Check` with `CheckVerify::Goal { predicate: Predicate::Matches(_), .. }`.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir lower`
Expected: PASS.

- [ ] **Step 5: Commit.**
```
git add crates/tau-ir/src/lower/parse.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ir): lower goals/deliverables into workflow.checks (D2)"
```

### Task A6: typecheck — check refs, loci, gate (G1), span non-determinism (G2), overlap (D7)

**Files:** Modify `crates/tau-ir/src/lower/typecheck.rs`, `crates/tau-ir/src/error.rs`

- [ ] **Step 1: New `IrError` variants** in `error.rs`:
```rust
    /// A `StepRun::Check` references an id absent from `workflow.checks`.
    #[error("pipeline step {step:?} runs check {check:?} but no such check is defined")]
    UnknownCheckRef { step: String, check: String },
    /// A check's `Output` locus names a non-earlier pipeline step.
    #[error("check {check:?} evaluates {output:?}, which is not an earlier pipeline step")]
    UnknownCheckLocus { check: String, output: String },
    /// A deliverable has no step declaring it `produces` the locus.
    #[error("deliverable {check:?} has no producer: no step declares produces = [{locus:?}]")]
    DeliverableNoProducer { check: String, locus: String },
    /// retry_from runs after the producer (Guarantee 1).
    #[error("check {check:?} retry_from {gate:?} runs after producer {producer:?} — gate must be at or before the producer")]
    GateAfterProducer { check: String, gate: String, producer: String },
    /// The retry span has no non-deterministic (agent) step (Guarantee 2).
    #[error("check {check:?} on_fail=retry but the span ({span:?}) contains no agent step; retrying cannot change the result")]
    RetrySpanDeterministic { check: String, span: String },
    /// Two retry spans overlap (D7).
    #[error("retry spans of checks {a:?} and {b:?} overlap; v1 requires disjoint retry spans")]
    OverlappingRetrySpans { a: String, b: String },
    /// A custom judge agent is not defined.
    #[error("deliverable {check:?} sets judge {judge:?} but no such agent is defined")]
    UnknownJudgeAgent { check: String, judge: String },
```

- [ ] **Step 2: Extend `check_pipeline`** in `typecheck.rs`. Add a `StepRun::Check(c)` arm to the existence match (exists = `wf.checks.contains_key(c)`, else `UnknownCheckRef`). For a `Check` step, validate `Locus::Output(ref)` names a strictly-earlier pipeline step (reuse the `seen_ids` set) else `UnknownCheckLocus`.

- [ ] **Step 3: Producer binding + Guarantees.** Because parse left `retry` unresolved, A6 resolves it. Write a new function `resolve_and_check_retries(parsed: &mut Parsed)` called from `typecheck` (note: `typecheck` currently takes `&Parsed`; change the lowering pipeline so retry resolution can mutate, OR resolve in `parse` and only *validate* here. Cleaner: move resolution into this function and have `lower_project` pass `&mut`). For each deliverable check with `retry_intent.on_fail == Retry`:
  - Find the producer: the pipeline step whose `StepRun::Agent(a)` has `a` in `parsed.produces` with the locus path (for `Locus::Path`); or the emitting step (for `Locus::Output(id)` the producer is `id`). If none → `DeliverableNoProducer`.
  - Resolve `gate` = `retry_intent.retry_from` if set, else the producer.
  - G1: index_of(gate) <= index_of(producer) in the pipeline order, else `GateAfterProducer`.
  - G2: the span `pipeline.steps[index(gate)..=index(producer)]` contains at least one `StepRun::Agent`, else `RetrySpanDeterministic`.
  - Custom judge: if `JudgeRef::Agent(a)` and `a` not in `wf.agents` → `UnknownJudgeAgent`.
  - Write the resolved `Retry { on_fail, max_attempts, gate, producer }` into `wf.checks[id].retry`.

- [ ] **Step 4: D7 overlap.** After resolving all retry spans, for every pair of retry-enabled checks compute `[index(gate)..=index(check_step)]` intervals and reject any overlap → `OverlappingRetrySpans`.

- [ ] **Step 5: Tests** (one per guarantee, TOML-driven like the existing `rejects_forward_output_reference` test):
  - deliverable with no producing step → `DeliverableNoProducer`.
  - `retry_from` pointing at a step after the producer → `GateAfterProducer`.
  - a retry span of only deterministic steps → `RetrySpanDeterministic`.
  - two overlapping retry spans → `OverlappingRetrySpans`.
  - `judge = "ghost"` with no `[agents.ghost]` → `UnknownJudgeAgent`.
  - a valid worked example (spec §Worked example) → `Ok`.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir typecheck`
Expected: PASS.

- [ ] **Step 6: Commit.**
```
git add crates/tau-ir/src/lower/typecheck.rs crates/tau-ir/src/error.rs crates/tau-ir/src/lower/mod.rs crates/tau-ir/src/lower/parse.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ir): check typecheck — producer binding, gate G1/G2, span overlap D7"
```

### Task A7: capability-fit — `produces` ⊆ fs-write coverage

**Files:** Modify `crates/tau-ir/src/lower/capability_fit.rs`

- [ ] **Step 1:** After the existing shape loop, for each agent in `parsed.produces` and each declared `produces` path, confirm some tool the agent references declares an `fs-write` capability whose glob covers the path. The fs-write coverage check belongs in `tau-pkg` (it owns `glob_subset` and the `Capability`/`FsCapability` parser); the IR crate has only `CapabilityRequirements`. Two options:
  - (a) Perform this cross-check in `tau-pkg`'s `validate()` where the raw capability strings + `produces` co-exist, emitting a `ProjectConfigError`. **Recommended** — it keeps `glob_subset` private to `tau-pkg` and matches "build-time enforcement at the config boundary."
  - (b) Re-parse capability globs in `tau-ir`. Rejected (duplicates the parser).

  Choose (a): move this task's logic into `tau-pkg`'s `validate_deliverables`/a new `cross_check_producers`. Use `is_glob_subset(produces_path, fs_write_glob)` (the `crates/tau-pkg/src/capability_override/glob_subset.rs` helper — confirm signature `pub(crate) fn is_glob_subset(child, parent) -> bool`). Emit:
```rust
ProjectConfigError::ProducerNotPermitted { agent: String, path: String }
// "step '{agent}' declares it produces '{path}' but holds no fs-write capability covering that path"
```

- [ ] **Step 2: Test** in `tau-pkg`: an agent with `produces = ["/workspace/report.md"]` and a tool whose fs-write cap is `fs-write:/workspace/**` validates; the same `produces` with `fs-write:/other/**` is rejected with `ProducerNotPermitted`.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg`
Expected: PASS.

- [ ] **Step 3: Commit.**
```
git add crates/tau-pkg/src/project/project.rs crates/tau-pkg/src/capability_override/glob_subset.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(pkg): cross-check agent produces against fs-write capability (build-time)"
```

### Task A8: TS-extract parity (`goals(...)`, `deliverables(...)`)

**Files:** Modify `crates/tau-ts-extract/src/factory.rs`, `crates/tau-ts-extract/src/lower.rs`; add to the conformance parity fixture.

- [ ] **Step 1:** Add `Goals`/`Deliverables` to the `Factory` enum and to `recognize_factory_call` (`"goals"`/`"deliverables"`). Mirror `extract_pipeline_steps` to extract object arrays/maps into intermediate structs, and mirror the TOML emission block (`[goals.<id>]` / `[deliverables.<id>]` tables). Read `lower.rs:143-243` (the `Factory::Pipeline` arm + emission) as the template.

- [ ] **Step 2:** Add a `goals`/`deliverables` example to the parity fixture (`crates/tau-ts-extract/tests/...` — the `fan_monitor_conformance.rs` test compares TOML- and TS-derived canonical IR byte-for-byte). Add the equivalent `[goals.*]`/`[deliverables.*]` to the fixture's `.toml` and `goals([...])`/`deliverables([...])` to its `.ts`.

- [ ] **Step 3: Run the parity test.**
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ts-extract`
Expected: PASS (byte-equal canonical IR from both surfaces).

- [ ] **Step 4: Commit.**
```
git add crates/tau-ts-extract/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(ts-extract): goals/deliverables TS factories with TOML parity"
```

### Task A9: conformance fixture (build-time only) + push PR-A

**Files:** Create `crates/tau-ir-conformance/fixtures/09_checks/workflow.toml`; modify `tests/conformance.rs`

- [ ] **Step 1:** Author `09_checks/workflow.toml` = the spec's worked example (gather → writer → `check:report` deliverable + a `has_sources` goal). Add a conformance test that lowers it and asserts `tau check`-level success (lowering `Ok`, `workflow.checks` has 2 entries, the deliverable's `retry.gate == producer == "writer"`).

- [ ] **Step 2: Run conformance + full tau-ir.**
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -p tau-pkg
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ir -p tau-pkg -p tau-ts-extract
```
Expected: PASS.

- [ ] **Step 3: Push + PR.**
```
git add crates/tau-ir-conformance/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(conformance): 09_checks build-time fixture (worked example)"
git push -u origin feat/checks-ir-foundation
gh pr create --base main --title "feat(ir): Check node + goals/deliverables build-time enforcement (D2/D7/G1/G2)" --body "PR-A of the deliverables-and-goals plan. Build-time only: Check IR node, StepRun::Check, workflow.checks, producer binding, gate/span guarantees, fs-write cross-check, TS parity, conformance fixture. No runtime evaluation yet.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge <PR#> --squash --delete-branch --auto
```

---

# PR-C1 — Runtime: deterministic goal evaluation + abort

**Branch:** `feat/checks-goal-eval` (off `main`, after A+B merged). Crate: `tau-runtime-core` (+ `tau-observe`, `tau-runtime-tokio` for vocabulary). Add `regex` to `tau-runtime-core`'s `Cargo.toml` (it is `no_std`-incompatible by default — enable `regex` with `default-features = false, features = ["std"]` only if the crate has `std`; `tau-runtime-core` is `no_std`+alloc. **Check:** `regex` needs `std`. If `tau-runtime-core` cannot take `std`, evaluate predicates in the *host* via a deterministic-registry-style indirection, OR gate predicate eval behind the `with-std-adapters` feature. Decide in Step 1.)

### Task C1.1: vocabulary constants (3-crate)

**Files:** `crates/tau-observe/src/vocabulary.rs` (canonical), `crates/tau-runtime-core/src/vocabulary.rs` (mirror), `crates/tau-runtime-tokio/tests/` (drift test)

- [ ] **Step 1:** Add to the canonical vocabulary (read the file; mirror the `EV_PIPELINE_STEP_*` style):
```rust
/// A check evaluation completed (pass or fail).
pub const EV_CHECK_EVALUATED: &str = "check.evaluated";
/// A failed check rewound to its gate to retry.
pub const EV_CHECK_RETRY: &str = "check.retry";
/// Span wrapping one check evaluation.
pub const SPAN_CHECK: &str = "check";
```
Mirror the same three in `tau-runtime-core/src/vocabulary.rs`.

- [ ] **Step 2:** Update the `tau-runtime-tokio` drift test (it asserts the core mirror equals the canonical set) to include the three new constants.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio vocabulary`
Expected: PASS.

- [ ] **Step 3: Commit.**
```
git add crates/tau-observe/src/vocabulary.rs crates/tau-runtime-core/src/vocabulary.rs crates/tau-runtime-tokio/tests/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(observe): check.evaluated/check.retry/SPAN_CHECK vocabulary"
```

### Task C1.2: predicate evaluation

**Files:** Create `crates/tau-runtime-core/src/interpreter/check_eval.rs`; modify `interpreter/mod.rs` (module decl)

- [ ] **Step 1:** Define a pure function over already-read bytes + the named-output value, so it needs no fs and no `std` regex if you route regex through a feature. Signature:
```rust
/// Evaluate a goal predicate against a locus value already materialized
/// to bytes (Path) or a JSON value (Output). Returns (passed, rationale).
pub(crate) fn eval_predicate(pred: &Predicate, bytes: Option<&[u8]>) -> (bool, String) { ... }
```
Implement `Exists` (bytes.is_some()), `NonEmpty` (non-empty), `Equals`, `Matches`/`MinCount` (regex), `SchemaValid` (jsonschema or a minimal check — confirm an existing schema validator in the workspace; if none, scope `SchemaValid` to "parses as JSON" for v1 and note it). For regex under `no_std`: gate the `Matches`/`MinCount`/`Equals`-as-text arms behind `#[cfg(feature = "with-std-adapters")]` and return an `Internal` error if invoked without it; the production `tau run` path (tau-cli) enables std features.

- [ ] **Step 2: Tests** for each predicate arm (pure, no I/O): `Matches` true/false, `NonEmpty` on empty vs non-empty, `MinCount` boundary.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core check_eval`
Expected: PASS.

### Task C1.3: evaluate `StepRun::Check` (goal kind) in `run_pipeline`, abort on fail

**Files:** Modify `crates/tau-runtime-core/src/interpreter/pipeline.rs`

- [ ] **Step 1:** Add a `StepRun::Check(check_id)` arm to the step match. Look up `module.workflow.checks[check_id]`. For `CheckVerify::Goal { evaluates, predicate }`:
  - Materialize the locus: `Locus::Output(id)` → `store.get(id.0)` stringified to bytes; `Locus::Path(p)` → `dispatcher.read_artifact(p)` (returns `Option<Result<Option<Vec<u8>>>>`; `None` from a dispatcher with no fs → `RuntimeError::Internal` "checks require a host filesystem").
  - `eval_predicate(predicate, bytes.as_deref())`.
  - Emit `EV_CHECK_EVALUATED` (id, kind="goal", verdict, attempt=1) under a `SPAN_CHECK` span.
  - On pass: `store.insert(step.id.0, Value::Bool(true))` and continue.
  - On fail with `retry: None | Some(Retry { on_fail: Abort, .. })`: return `Ok(PipelineOutcome { status: CheckAborted { check, rationale }, .. })`. (Goal *retry* is handled by the shared loop in C2; for C1 a goal with `on_fail=retry` may either be deferred or treated as abort — the build-time G2 check already requires a non-deterministic span, so retry-goals are valid configs. Scope C1 to abort; C2 generalizes the loop to cover goals too.)

- [ ] **Step 2: Test** (in `tau-runtime-core` or conformance, using `MockLlmBackend` + a `RecordingDispatcher` that implements `read_artifact` over an in-memory map): a pipeline `writer → check:has_sources` where the goal passes → `PipelineStatus::Completed`; where it fails → `PipelineStatus::CheckAborted { check: "has_sources", .. }`. Provide a test dispatcher impl of `read_artifact` returning canned bytes.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core pipeline`
Expected: PASS.

- [ ] **Step 3: clippy, commit, push, PR-C1.**
```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core
git add crates/tau-runtime-core/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime): evaluate goal checks in run_pipeline; abort on fail (C1)"
git push -u origin feat/checks-goal-eval
gh pr create --base main --title "feat(runtime): deterministic goal check evaluation + abort (C1)" --body "PR-C1 of the deliverables-and-goals plan. Evaluates StepRun::Check goal predicates against read_artifact / OutputStore, emits check.evaluated, aborts on failure. No judge or retry yet.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge <PR#> --squash --delete-branch --auto
```

---

# PR-C2 — Runtime: LLM judge + rewind-to-gate retry

**Branch:** `feat/checks-judge-retry` (off `main`, after C1). Crate: `tau-runtime-core`.

### Task C2.1: `Verdict` type + parse helper (D5)

**Files:** Create `crates/tau-runtime-core/src/interpreter/verdict.rs`

- [ ] **Step 1:**
```rust
//! LLM judge verdict (D5). The judge is a one-shot agent whose final
//! text is parsed into this struct; an unparseable verdict is a soft
//! failure (`met = false`) with a diagnostic rationale, not a kernel error.

use alloc::string::{String, ToString};
use serde::Deserialize;

/// A judge's structured verdict.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Verdict {
    /// Did the artifact satisfy the criterion?
    pub met: bool,
    /// Why — fed back into the retry loop.
    pub rationale: String,
}

/// Parse a judge's final assistant text into a [`Verdict`]. Tolerant of
/// surrounding prose: extracts the first balanced `{...}` and parses it.
/// On any failure returns a `met = false` verdict carrying the raw text.
pub(crate) fn parse_verdict(text: &str) -> Verdict {
    if let Some(json) = first_json_object(text) {
        if let Ok(v) = serde_json::from_str::<Verdict>(json) {
            return v;
        }
    }
    Verdict { met: false, rationale: alloc::format!("judge returned unparseable verdict: {text}") }
}

/// Return the first balanced top-level `{...}` substring, if any.
fn first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    for (i, c) in s[start..].char_indices() {
        match c { '{' => depth += 1, '}' => { depth -= 1; if depth == 0 { return Some(&s[start..start + i + 1]); } }, _ => {} }
    }
    None
}
```

- [ ] **Step 2: Tests:** clean JSON; JSON embedded in prose; non-JSON → `met=false` with the raw text in `rationale`.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core verdict`
Expected: PASS.

- [ ] **Step 3: Commit.**
```
git add crates/tau-runtime-core/src/interpreter/verdict.rs crates/tau-runtime-core/src/interpreter/mod.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime): Verdict type + tolerant parse_verdict (D5)"
```

### Task C2.2: judge invocation

**Files:** Modify `crates/tau-runtime-core/src/interpreter/check_eval.rs`

- [ ] **Step 1:** Add an async `run_judge` that, given a `JudgeRef`, the `must_satisfy` criterion, and the artifact bytes, runs a one-shot agent via `run_agent` and parses the verdict:
  - `JudgeRef::Builtin { model }` → synthesize an `Agent` node in-memory: prompt = the canonical judge prompt (a const string instructing the `{met, rationale}` JSON shape, embedding `must_satisfy` and the artifact text), `model` = `model` override or a default const, `tool_refs: []`, `budget: AgentBudget::default()`. Call `Box::pin(run_agent(module.clone(), &synth_agent, dispatcher.clone(), vec![user_message(&prompt)]))`.
  - `JudgeRef::Agent(id)` → look up `module.workflow.agents[id]`, run it one-shot with the criterion+artifact as the user message.
  - Extract `last_assistant_text(&outcome)`, `parse_verdict(&text)`. Aggregate the judge's `token_usage` into the caller's accumulator (return it alongside the verdict).

- [ ] **Step 2: Test** with `MockLlmBackend` returning `{"met":true,"rationale":"ok"}` → `Verdict { met: true, .. }`; returning prose without JSON → `met=false`.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core run_judge`
Expected: PASS.

- [ ] **Step 3: Commit.**
```
git add crates/tau-runtime-core/src/interpreter/check_eval.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime): built-in + custom-agent judge invocation"
```

### Task C2.3: deliverable check + rewind-to-gate retry loop (D4)

**Files:** Modify `crates/tau-runtime-core/src/interpreter/pipeline.rs`

The Plan-1 loop is `for step in &pipeline.steps`. Convert it to an index cursor so a failed retry-enabled check can rewind.

- [ ] **Step 1: Convert to an index loop** with per-check attempt counters:
```rust
let steps = &pipeline.steps;
let mut i = 0usize;
let mut attempts: BTreeMap<String, u32> = BTreeMap::new(); // check id -> attempts used
while i < steps.len() {
    let step = &steps[i];
    // ... existing Agent/Tool/Deterministic arms unchanged (they set `output` + store.insert) ...
    // Check arm: see Step 2.
    i += 1;
}
```

- [ ] **Step 2: The `Check` arm.** Evaluate (goal predicate as in C1, or deliverable = existence floor then `run_judge`). On pass → emit `EV_CHECK_EVALUATED verdict=pass`, continue. On fail:
  - If `retry` is `None` or `OnFail::Abort` → return `CheckAborted`.
  - If `OnFail::Retry`: read `attempts[check]` (default 0). If `attempts + 1 >= max_attempts` → emit final `EV_CHECK_EVALUATED verdict=fail`, return `CheckAborted` with the last rationale. Else: increment `attempts`; emit `EV_CHECK_RETRY { id, rewind_to: gate, next_attempt }`; set the cursor `i = index_of(retry.gate)`; **inject the rationale** for the next pass (Step 3); `continue` (do not `i += 1`).

- [ ] **Step 3: Rationale injection.** Carry a `BTreeMap<AgentId, String>` of "pending feedback" keyed by agent id for the steps in the span `[gate..=producer]`. When the cursor re-enters an `StepRun::Agent` step whose id has pending feedback, build `initial = vec![user_message(&rendered), user_message(&feedback)]` (the rendered input followed by a feedback turn `"previous attempt rejected: <rationale>"`). `run_agent` splits this into history + last message, so the feedback is the final user turn the agent sees. Clear the pending feedback after consuming it. Deterministic/tool steps in the span ignore feedback (pure). The producer re-run overwrites its `store` entry (BTreeMap insert), so downstream consumers see the new value.

- [ ] **Step 4: Budget (D4).** Each `run_agent` call already applies the per-agent `AgentBudget` fresh; accumulate every attempt's `token_usage` into `total_usage` (already wired in B2). No span-level cap in v1 — the loop bound is `max_attempts`. (Honest per the spec's reconciled D4.)

- [ ] **Step 5: Tests** (the headline behavior), using `MockLlmBackend` with a scripted cassette: attempt-1 writer output → judge fail (`met=false, rationale="only 1 source"`), attempt-2 writer output (different) → judge pass. Assert: final `PipelineStatus::Completed`; the trace recorded one `check.retry` and two `check.evaluated`; the writer's attempt-2 invocation received the rationale (assert via `MockLlmBackend::invocations()` containing the feedback text). Also test `max_attempts` exhaustion → `CheckAborted`.

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core pipeline`
Expected: PASS.

- [ ] **Step 6: clippy, commit, push, PR-C2.**
```
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core
git add crates/tau-runtime-core/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(runtime): deliverable judge + rewind-to-gate retry with rationale feedback (C2/D4/D5)"
git push -u origin feat/checks-judge-retry
gh pr create --base main --title "feat(runtime): LLM judge + rewind-to-gate retry (C2)" --body "PR-C2 of the deliverables-and-goals plan. Adds the swappable judge, Verdict parsing, and the bounded rewind-to-gate retry loop with rationale feedback. Completes runtime check semantics.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge <PR#> --squash --delete-branch --auto
```

---

# PR-D — CLI surface (D6) + end-to-end conformance

**Branch:** `feat/checks-cli-surface` (off `main`, after C2). Crate: `tau-cli`, `tau-ir-conformance`.

### Task D1: surface `PipelineStatus` in `tau run`

**Files:** Modify `crates/tau-cli/src/cmd/run.rs`

- [ ] **Step 1:** In `try_run_pipeline` (the PR-B `// TODO(PR-D)` site), match on `outcome.status`:
  - `Completed` → render the final step output as today; exit 0.
  - `AgentFailed { step, status }` → render an agent-failure diagnostic; exit non-zero (use the existing ADR-0006 failure exit code path used elsewhere in `run.rs`).
  - `CheckAborted { check, rationale }` → print `check '{check}' failed: {rationale}`; exit non-zero.
  Print `outcome.token_usage` in the same place the single-agent path prints usage (match the existing format).

- [ ] **Step 2: Test** (integration, `tau-cli/tests/`): a fixture project with a passing goal `tau run`s to exit 0; a failing goal exits non-zero with the rationale on stderr. (Use the existing `tau-cli` integration-test harness + a `tau.toml` fixture with a `[goals.*]` whose predicate fails against a file the run writes.)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli run`
Expected: PASS.

- [ ] **Step 3: Commit.**
```
git add crates/tau-cli/src/cmd/run.rs crates/tau-cli/tests/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(cli): surface check pass/abort + token usage in tau run (D6)"
```

### Task D2: end-to-end conformance (the worked example, dev vs run)

**Files:** Modify `crates/tau-ir-conformance/fixtures/09_checks/` (add `mock_llm.jsonl`), `tests/conformance.rs`

- [ ] **Step 1:** Add a `mock_llm.jsonl` cassette to `09_checks` driving the spec's worked example: gather → writer (attempt 1, 1 source) → deliverable judge fail → writer (attempt 2, 2 sources) → judge pass → `has_sources` goal pass. Add a conformance test that runs the pipeline via the conformance harness and asserts `PipelineStatus::Completed` plus the `check.retry`/`check.evaluated` event sequence.

- [ ] **Step 2: Run conformance.**
Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir-conformance`
Expected: PASS.

- [ ] **Step 3: Full sweep of touched crates + clippy.**
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ir -p tau-pkg -p tau-runtime-core -p tau-cli -p tau-ir-conformance -p tau-ts-extract
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli -p tau-ir-conformance
```
Expected: PASS.

- [ ] **Step 4: Push + PR-D.**
```
git add crates/tau-ir-conformance/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(conformance): 09_checks end-to-end judge+retry cassette"
git push -u origin feat/checks-cli-surface
gh pr create --base main --title "feat(cli): wire checks into tau run + e2e conformance (D6)" --body "PR-D of the deliverables-and-goals plan. Surfaces PipelineStatus in tau run, adds the end-to-end worked-example conformance test. Closes the deliverables-and-goals feature.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge <PR#> --squash --delete-branch --auto
```

### Task D3: docs

**Files:** Create `docs/explanation/checks.md` (or extend the relevant reference page); add to `docs/SUMMARY.md`.

- [ ] **Step 1:** Write a Diátaxis explanation page covering goal vs deliverable, the predicate menu, the judge easy/tune/power split, `produces` binding, and the abort/retry semantics. Add a `SUMMARY.md` line (required — mdBook silently skips unlisted pages). Build the book per `CLAUDE.md` DOCS RULES:
```
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build
rm -rf docs/book
```
- [ ] **Step 2: Commit** (docs-only; plain `git push` is fine):
```
git add docs/
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "docs(checks): goals & deliverables explanation page"
```

---

## Self-review

**Spec coverage:** goal predicate menu + native-fn (A4/C1.2); deliverable existence floor + judge easy/tune/power (A4/C2.2); explicit `produces` binding + fs-write cross-check (A4/A7); gate `<=` producer G1 + span non-determinism G2 (A6); abort default + opt-in rewind retry with rationale feedback + max_attempts + budget (C2.3/D4); check.evaluated/check.retry trace (C1.1); new `Check` IR node + `StepRun::Check` (A2/A3); `tau run` surface + universal build-time (D1/A); `{met, rationale}` verdict + parse-failure (C2.1/D5); span-overlap D7 (A6); PipelineOutcome + #5 (B). All D1–D7 mapped.

**Known open decisions deferred to execution (flagged inline, not placeholders):** (1) `regex`/`SchemaValid` under `tau-runtime-core`'s `no_std` — resolved in C1.2 Step 1 by feature-gating behind `with-std-adapters` (tau-cli enables std). (2) fs-write cross-check placed in `tau-pkg` not `tau-ir` (A7 Step 1, option a) to keep `glob_subset` private. (3) typecheck must mutate to resolve `Retry` — A6 Step 3 changes `lower_project` to pass `&mut Parsed`. These are decisions with a chosen answer, not gaps.

**Type consistency:** `PipelineOutcome.outputs/token_usage/status`, `PipelineStatus::{Completed,AgentFailed,CheckAborted}`, `Check{id,verify,retry}`, `CheckVerify::{Goal,Deliverable}`, `Locus::{Path,Output}`, `Predicate::{Exists,NonEmpty,Equals,Matches,MinCount,SchemaValid,NativeFn}`, `JudgeRef::{Builtin,Agent}`, `Retry{on_fail,max_attempts,gate,producer}`, `OnFail::{Abort,Retry}`, `Verdict{met,rationale}` — used identically across B→A→C→D tasks.
