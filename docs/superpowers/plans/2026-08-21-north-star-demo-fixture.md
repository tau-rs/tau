# North-Star Demo Fixture (#461) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One `[allow]`-governed fixture workflow exercising Branch + Loop, executed via `tau run` (dev) AND `tau build` + `tau run --bundle` (artifact), with the wasm-guest build-time refusal asserted as a gate, a negative over-reach twin failing `tau check governance`, all wired into CI via nextest, plus a docs walkthrough.

**Architecture:** Fixtures live at `crates/tau-cli/tests/fixtures/north-star{,-over-reach}/tau.toml` (canonical on-disk). One integration test file `crates/tau-cli/tests/north_star_demo.rs` copies the fixture into a tempdir, adds the echo-llm scaffold (`.tau/config.toml`, package manifest, schema-v6 lockfile written to both `tau.lock` and `tau-lock.toml`), and drives the real `tau` binary. The wasm leg is a library-level `lower_to_wasm_ir` refusal assertion (feature-fit rejects Branch/Loop on `any-wasi-strict` — ADR-0059 by design; guest execution of control-flow is a filed engine gap, not fixable here). No `.github/workflows` change: `ci.yml` `test-stable / linux` runs all non-ignored `tau-cli` nextest tests.

**Tech Stack:** Rust integration tests (`assert_cmd`, `tempfile`, `serde_json`, `predicates`), echo-llm scripted plugin harness (`tests/common/echo_plugins`), mdBook docs.

**Spec:** Design presented in-session (Conductor workspace tallinn, 2026-08-21) against issue #461; authoritative surfaces: `docs/how-to/authoring-a-branch.md` (Branch syntax), `crates/tau-ir-lower/src/lower/parse.rs::lowers_loop_step_with_until_and_bound` (Loop syntax), `crates/tau-cli/tests/{run_pipeline,cmd_run_bundle_pipeline,cmd_check_governance,cmd_build_wasm}.rs` (harness patterns), `docs/explanation/slicing-policy.md` rule 5.

## Global Constraints

- No engine changes. Fixtures + tests + docs only. Engine gaps → file GitHub issues.
- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo …` scoped `-p tau-cli` (main agent; per-role dirs for subagents).
- Prefer `cargo nextest run` over `cargo test`.
- All step outputs are the ENTRY agent's `canned_text` (one shared echo-llm backend, configured from the entry agent's `[agents.<id>.config]`).
- Branch/Loop/Parallel/Suspend steps store no output → last pipeline step must be a leaf `run = "agent:…"` step.
- Loop `until` exhaustion (max_iters without the predicate holding) is a hard error → the canned text must satisfy the `until` pattern.
- Template refs to steps that never ran hard-error at resolution → final step reading `${steps.escalate.output}` / `${steps.draft.output}` proves the then-arm and loop body executed.
- Windows/CI: tests touching `tau check`/`tau build` must ensure a `TAU_HOME` (mirror `check_common::ensure_tau_home()` / explicit `TAU_HOME` env).
- Docs: new page must be listed in `docs/SUMMARY.md`; `mdbook build` clean from `docs/` with `PATH="$HOME/.cargo/bin:$PATH"`; `rm -rf docs/book` after.
- Conventional commits; never push to main; PR closes #461.

---

### Task 1: The governed north-star fixture + over-reach twin

**Files:**
- Create: `crates/tau-cli/tests/fixtures/north-star/tau.toml`
- Create: `crates/tau-cli/tests/fixtures/north-star-over-reach/tau.toml`

**Interfaces:**
- Produces: fixture paths consumed by Task 2/3 tests via `fixture("north-star")` helper; the canned text sentinel `"URGENT: coolant temperature rising - fan engaged. APPROVED"` and step ids `triage`, `route`, `escalate`, `ack`, `review`, `draft`, `report` asserted downstream.

- [ ] **Step 1: Write `north-star/tau.toml`**

```toml
# The cross-epic north-star demo (issue #461, slicing-policy rule 5):
# ONE [allow]-governed workflow using Branch + Loop, capability-bounded,
# runnable in dev (`tau run`) and as a built artifact (`tau run --bundle`).
# `tau build --target wasm-guest` refuses it at feature-fit (ADR-0059):
# control-flow is not yet executable in the wasm guest — that leg extends
# this same fixture when guest control-flow lands.

[project]
name = "north-star"
version = "0.1.0"

# The constitution: nothing in this project may exceed these ceilings.
[allow]
"fs.read" = { paths = ["${PROJECT}/sensors/**"] }

[allow.models.default]
backend = "echo-llm"
model = "claude-haiku-4-5"

[allow.tools.read_temp]
native = "ReadTemp"

[agents.triage]
display_name = "Triage"
package      = "echo-llm@^0.1"
model        = "default"
tool_refs    = ["read_temp"]

[agents.triage.prompt]
system = "Classify the incoming incident report."

[agents.triage.config]
canned_text = "URGENT: coolant temperature rising - fan engaged. APPROVED"

[agents.oncall]
display_name = "On-call"
package      = "echo-llm@^0.1"
model        = "default"

[agents.oncall.prompt]
system = "Escalate the incident to the on-call rotation."

[agents.scribe]
display_name = "Scribe"
package      = "echo-llm@^0.1"
model        = "default"

[agents.scribe.prompt]
system = "Acknowledge the routine report."

[agents.reviewer]
display_name = "Reviewer"
package      = "echo-llm@^0.1"
model        = "default"

[agents.reviewer.prompt]
system = "Draft the incident summary; revise until approved."

[agents.reporter]
display_name = "Reporter"
package      = "echo-llm@^0.1"
model        = "default"

[agents.reporter.prompt]
system = "Publish the final incident report."

# Capability-bounded native tool: reads the sensor drop directory, and its
# declared caps sit inside the [allow] fs.read ceiling.
[tools.read_temp]
native      = "ReadTemp"
description = "Read the incident sensor temperature."
capabilities = [{ kind = "fs.read", paths = ["${PROJECT}/sensors/**"] }]

[pipeline]

[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"

# Branch: urgent incidents escalate; routine ones get acknowledged.
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }

  [[pipeline.steps.then]]
  id = "escalate"
  run = "agent:oncall"
  input = "${steps.triage.output}"

  [[pipeline.steps.otherwise]]
  id = "ack"
  run = "agent:scribe"
  input = "${steps.triage.output}"

# Bounded Loop: redraft until the reviewer approves, at most 3 times.
[[pipeline.steps]]
id = "review"
until = { evaluates = "steps.draft.output", check = "matches", pattern = "APPROVED" }
max_iters = 3

  [[pipeline.steps.body]]
  id = "draft"
  run = "agent:reviewer"
  input = "${steps.escalate.output}"

# Leaf final step (Branch/Loop store no output; the pipeline renderer
# renders the LAST leaf step). Reading ${steps.draft.output} proves the
# loop body ran; the `escalate` ref above proves the then-arm ran.
[[pipeline.steps]]
id = "report"
run = "agent:reporter"
input = "${steps.draft.output}"
```

- [ ] **Step 2: Write `north-star-over-reach/tau.toml`** — identical except the project name and the tool's caps escape the ceiling:

Project name `north-star-over-reach`; replace the `[tools.read_temp]` capabilities line with:

```toml
capabilities = [{ kind = "fs.read", paths = ["/etc/**"] }]
```

and add a header comment: `# Negative twin of ../north-star: read_temp over-reaches the [allow] fs.read ceiling (/etc/** vs ${PROJECT}/sensors/**), so tau check governance must fail with exit 2.`

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/tests/fixtures/north-star crates/tau-cli/tests/fixtures/north-star-over-reach
git commit -m "test(cli): add north-star demo fixture + over-reach twin (#461)"
```

### Task 2: `north_star_demo.rs` — dev path, artifact path, governance gates, wasm refusal

**Files:**
- Create: `crates/tau-cli/tests/north_star_demo.rs`

**Interfaces:**
- Consumes: Task 1 fixtures; `common::echo_plugins::ensure_echo_plugins_built()` (`crates/tau-cli/tests/common/echo_plugins.rs`); `tau_cli::cmd::build_wasm::lower_to_wasm_ir` (path per `cmd_build_wasm.rs`); sentinel `"URGENT: coolant temperature rising - fan engaged. APPROVED"`.
- Produces: test names `north_star_runs_governed_in_dev`, `north_star_json_outcome_shape`, `north_star_bundle_roundtrip`, `north_star_wasm_guest_build_is_refused_at_feature_fit`, `north_star_over_reach_twin_fails_check_governance` (referenced by docs page + CI notes).

- [ ] **Step 1: Write the test file** (setup helper mirrors `cmd_run_bundle_pipeline.rs::setup_pipeline_bundle_project` verbatim for the `.tau/config.toml` + package manifest + v6 lockfile blocks, but copies `tau.toml` from the on-disk fixture instead of an inline string):

Skeleton (fill lockfile/manifest strings exactly as in `cmd_run_bundle_pipeline.rs:38-117`):

```rust
//! Issue #461: the cross-epic north-star demo fixture, end to end.
mod common;

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

const SENTINEL: &str = "URGENT: coolant temperature rising - fan engaged. APPROVED";

fn fixture_toml(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .join("tau.toml");
    std::fs::read_to_string(p).expect("fixture tau.toml exists")
}

/// Echo scaffold (config.toml, echo-llm package manifest, v6 lockfile to
/// BOTH tau.lock and tau-lock.toml) + the named fixture's tau.toml.
fn setup_project(fixture: &str) -> tempfile::TempDir { /* mirror cmd_run_bundle_pipeline.rs */ }
```

Tests:
1. `north_star_runs_governed_in_dev` — `tau run triage "coolant alarm"` (NO `--allow-ungoverned`), `TAU_HOME` set, `TAU_TESTING_ALLOW_MOCK_SANDBOX=1`; assert exit 0 and stdout contains `SENTINEL`.
2. `north_star_json_outcome_shape` — same + `--json`; parse lines, find the `outcome` object; assert `outcome == "completed"`, `final_message == SENTINEL`, and `total_turns` absent (pipeline renderer shape).
3. `north_star_bundle_roundtrip` — `tau build` (NO `--allow-ungoverned`) prints bundle path; then `tau run --bundle <path> triage "coolant alarm" --json`; same outcome assertions. Optionally parse the bundle manifest and assert `governance.verdict == Governed`.
4. `north_star_wasm_guest_build_is_refused_at_feature_fit` — subprocess variant: `tau build --target wasm-guest` in the project dir must exit 2 with stderr naming the feature-fit refusal (contains `"feature-fit"`, `"Branch"`, `"Loop"`). If the subprocess path proves awkward, fall back to the `cmd_build_wasm.rs` library pattern: `lower_to_wasm_ir(&fixture_dir).unwrap_err()` message contains the same.
5. `north_star_over_reach_twin_fails_check_governance` — scaffold `north-star-over-reach`, run `tau check governance`, assert `.code(2)` (mirror `cmd_check_governance.rs`; add a stderr/stdout contains check for the over-reach diagnostic if stable).

- [ ] **Step 2: Run the suite, expect failures only where behavior is genuinely unknown**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-cli -E 'test(north_star)'`

Iterate on fixture/test until green. Known unknowns to resolve empirically (adjust fixture, never the engine): `${PROJECT}` glob subset semantics in the governance lattice; `matches` predicate wiring in the CLI deterministic registry; governed `tau run` gate behavior on the dev path; whether `[allow.tools]` must enumerate every `[tools]` entry.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/tests/north_star_demo.rs crates/tau-cli/tests/fixtures/north-star*
git commit -m "test(cli): north-star demo e2e - dev run, bundle roundtrip, wasm refusal, governance twin (#461)"
```

### Task 3: Engine-gap issue for the wasm execution leg

**Files:** none (GitHub issue).

- [ ] **Step 1: Verify no existing issue covers guest control-flow execution** (`gh issue list --search "wasm control-flow"` etc. — done in-session: none).
- [ ] **Step 2: File the issue**: title `wasm guest cannot execute Branch/Loop — north-star (#461) wasm leg blocked at feature-fit`; body cites `crates/tau-ports/src/target/registry.rs` (`AdapterFamily::Wasi` `supported_features: &[]`), `tau-ir-lower` feature_fit, ADR-0059, and states the north-star fixture's wasm-execution DoD activates when this lands. Link #461.

### Task 4: Docs — "The north-star in action"

**Files:**
- Create: `docs/tutorials/the-north-star-in-action.md`
- Modify: `docs/SUMMARY.md` (Tutorials block, after `build-your-first-skill.md`)
- Modify: `docs/tutorials/README.md` (mention the new page)

- [ ] **Step 1: Write the page** — short walkthrough: the constitution (`[allow]` ceilings), the pipeline shape (triage → Branch route → bounded Loop review → report) with the fixture TOML inlined or excerpted, the three ways it runs (`tau run`, `tau build` + `tau run --bundle`, and the wasm-guest refusal as the build-time gate story), the negative twin + `tau check governance` exit 2, pointers to `docs/explanation/slicing-policy.md` rule 5, ADR-0057/0058/0059, `docs/how-to/authoring-a-branch.md`. Note Loop docs gap: this page is currently the only user-facing Loop authoring example (parse-test-derived syntax).
- [ ] **Step 2: Add SUMMARY.md + tutorials README entries.**
- [ ] **Step 3: Build the book**: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` → only `[INFO]` lines; then `rm -rf docs/book`.
- [ ] **Step 4: Commit**

```bash
git add docs/tutorials/the-north-star-in-action.md docs/SUMMARY.md docs/tutorials/README.md
git commit -m "docs(tutorials): the north-star in action - walk the #461 demo fixture"
```

### Task 5: Gate + PR

- [ ] **Step 1: Full local gate**: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-cli -E 'test(north_star)'` green; `cargo fmt --check` (30s timeout) clean.
- [ ] **Step 2: Push branch, open PR** to main: `Closes #461`; body: fixture description, both-path outputs, the filed engine-gap issue, why no workflow YAML change (nextest = the repo's fixture-CI pattern).
- [ ] **Step 3: Enrol auto-merge**: `gh pr merge <N> --squash --auto` (NO `--delete-branch`); `gh pr update-branch <N>` if BEHIND.
