# Bare-item coverage inventory — round 4

**Source:** tier-2 crate audit on 2026-05-27.
**Spec:** `docs/superpowers/specs/2026-05-27-doctests-round-4-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-27-doctests-round-4.md`.

## Categories

- **include**: classification per spec §3.1 — adds a `///` doctest fence in this PR.
- **skip-trivial**: trivial item not requiring an example.
- **skip-getter / skip-setter**: trivial accessor / mutator.
- **skip-derived**: derived trait impl.
- **skip-alias**: `pub type X = Y`.
- **skip-display / skip-debug**: `Display` / `Debug` impl.
- **skip-marker**: marker trait or unit-struct sentinel.
- **skip-reexport**: `pub use`.
- **skip-feature-gated**: behind a cargo feature; doctest would need `--features <flag>`.
- **skip-needs-fixture**: requires non-trivial test fixtures (real sandbox, env injection, multi-thread) that exceed reasonable doctest scope.
- **done**: already had a fence before round 4 began.

## tau-workflow

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | `error.rs:8` | `pub enum WorkflowError` | include | Construct `ParseFailed` + `DriftDetected` variants, assert `Display` output contains expected substrings. Two separate fences on the enum doc comment. |
| 2 | `lib.rs:13` | `pub mod error` | skip-reexport | Module declaration; not an item with meaningful usage. |
| 3 | `lib.rs:14` | `pub mod model` | skip-reexport | Module declaration. |
| 4 | `lib.rs:15` | `pub mod persistence` | skip-reexport | Module declaration. |
| 5 | `lib.rs:16` | `pub mod runner` | skip-reexport | Module declaration. |
| 6 | `lib.rs:17` | `pub mod template` | skip-reexport | Module declaration. |
| 7 | `model.rs:12` | `pub struct Workflow` | include | Parse a two-step workflow via `from_str`; assert `name`, `description`, `steps.len()`, and first step id. |
| 8 | `model.rs:29` | `pub struct Step` | skip-trivial | Plain data struct; no constructor — constructed only by `Workflow::from_str`. Covered indirectly by `Workflow` and `StepKind` examples. |
| 9 | `model.rs:38` | `pub enum StepKind` | include | Parse a minimal TOML and pattern-match the resulting `AgentRun` variant; assert `agent` and `input` fields. |
| 10 | `model.rs:83` | `Workflow::from_path` | include | Write a TOML file to `tempdir()`, call `from_path`, assert `name` + `steps.len()`. |
| 11 | `model.rs:93` | `Workflow::from_str` | include | Two fences: (a) valid parse asserts `name` + `steps.len()`; (b) invalid TOML asserts `ParseFailed`. |
| 12 | `persistence.rs:15` | `pub struct StepRecord` | include | Build a `StepRecord` literal; round-trip JSON via `serde_json`; assert `step_id`, `status`, and `duration_ms` survive the round-trip. |
| 13 | `persistence.rs:51` | `pub enum StepStatus` | include | Assert `as_str_lowercase()` values match serde JSON encoding for both variants. |
| 14 | `persistence.rs:64` | `StepStatus::as_str_lowercase` | include | Short assertion fence confirming both variants return the expected string. |
| 15 | `persistence.rs:74` | `pub fn run_log_path` | include | Call with sample args; assert `ends_with` the expected filename and that the path contains `.tau/workflow-runs`. |
| 16 | `persistence.rs:102` | `pub struct RunLog` | skip-trivial | No public constructor fence on the struct itself — see `open_for_write` (row 17) which is the only entry point. |
| 17 | `persistence.rs:111` | `RunLog::open_for_write` | include | `tokio_test::block_on` + `tempdir()`; call `open_for_write` on a nested path; assert returned `path()` matches. |
| 18 | `persistence.rs:127` | `RunLog::path` | skip-getter | Trivial `&self.path` getter. Asserted as a side effect of the `open_for_write` fence. |
| 19 | `persistence.rs:138` | `RunLog::append` | skip-needs-fixture | Requires installing a `WorkflowRunLogLayer` tracing subscriber to materialize the JSONL event. This is a non-trivial multi-component fixture (tracing subscriber + async runtime + layer) that exceeds reasonable doctest scope; the full scenario is covered by `persistence.rs` unit tests. |
| 20 | `persistence.rs:164` | `pub async fn replay` | include | `tokio_test::block_on` + `tempdir()`; write a minimal valid JSONL line, call `replay`, assert record count and `step_id`. |
| 21 | `runner.rs:23` | `pub struct Runner` | skip-trivial | No standalone constructor fence on the struct — see `Runner::new` (row 22). |
| 22 | `runner.rs:58` | `Runner::new` | include | `no_run` fence (real `Runtime::builder().build()` touches env + config); shows constructor call shape. |
| 23 | `runner.rs:66` | `Runner::run` | skip-needs-fixture | Async method requiring a fully wired `Runtime`, agent definitions, and FS; integration-test territory. Covered by existing integration tests in `tau-cli`. |
| 24 | `runner.rs:30` | `pub struct RunOpts` | include | Construct `RunOpts` literal with `input`, `run_id: None`, empty `completed` + `agents`; assert field values. |
| 25 | `runner.rs:45` | `pub struct RunOutcome` | skip-trivial | Plain data struct populated only by `Runner::run`; no public constructor. |
| 26 | `runner.rs:354` | `pub fn check_drift` | include | Parse a two-step workflow; build one matching `StepRecord`; assert `Ok` on prefix match. Build a mismatched record; assert `DriftDetected`. |
| 27 | `template.rs:22` | `pub fn resolve` | include | Three fences: (a) `${input}` substitution; (b) `${steps.<id>.output}` chain; (c) unknown reference → `TemplateUnresolved`. |

**Total:** 27 items — 14 include, 11 skip-*, 2 skip-trivial-struct (rows 21/25), 0 done.

## Status log

- 2026-05-27 — tau-workflow classifications + 14 includes (PR-A). `cargo test --doc -p tau-workflow` → 19 passed, 0 failed.
