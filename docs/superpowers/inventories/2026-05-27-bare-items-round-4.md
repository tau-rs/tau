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
- **skip-binary-internal**: item is `pub` for the binary's integration-test reachability, but not a stable library API for embedders.
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

## tau-app

tau-app is a binary crate. Its internal modules (`mod cancel`, `mod dispatch`, …) are all private — only items explicitly re-exported from `tau_app::serve` are reachable from doctests. Items in private modules are classified `skip-binary-internal` even when they carry `pub` inside the module.

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | `lib.rs:20` | `pub mod serve` | skip-reexport | Module declaration. |
| 2 | `serve/cancel.rs:10` | `pub struct CancelRegistry` | include | Construct via `Default`; exercise `register`, `cancel`, `forget`, `cancel_all`, `len`, `is_empty`. Five fences (struct + 4 methods). |
| 3 | `serve/dispatch.rs:25` | `pub struct Dispatcher` | skip-binary-internal | `#[doc(hidden)]` re-export; requires `Arc<Runtime>` + channel setup; binary dispatch loop machinery. |
| 4 | `serve/dispatch_run.rs:19` | `pub async fn execute` | skip-binary-internal | Private module (`mod dispatch_run`); binary per-request executor; not accessible from doctests. |
| 5 | `serve/error_codes.rs:14` | `pub const PARSE_ERROR` | skip-binary-internal | Private module (`mod error_codes`); constants not accessible from doctests. |
| 6 | `serve/error_codes.rs:16` | `pub const INVALID_REQUEST` | skip-binary-internal | Private module. |
| 7 | `serve/error_codes.rs:18` | `pub const METHOD_NOT_FOUND` | skip-binary-internal | Private module. |
| 8 | `serve/error_codes.rs:20` | `pub const INVALID_PARAMS` | skip-binary-internal | Private module. |
| 9 | `serve/error_codes.rs:22` | `pub const INTERNAL_ERROR` | skip-binary-internal | Private module. |
| 10 | `serve/error_codes.rs:26` | `pub const HANDSHAKE_MISMATCH` | skip-binary-internal | Private module. |
| 11 | `serve/error_codes.rs:28` | `pub const CANCELLED` | skip-binary-internal | Private module. |
| 12 | `serve/error_codes.rs:30` | `pub const HANDSHAKE_REQUIRED` | skip-binary-internal | Private module. |
| 13 | `serve/error_codes.rs:32` | `pub const ALREADY_HANDSHAKEN` | skip-binary-internal | Private module. |
| 14 | `serve/error_codes.rs:34` | `pub const SERVER_BUSY` | skip-binary-internal | Private module. |
| 15 | `serve/error_codes.rs:36` | `pub const PROJECT_ERROR` | skip-binary-internal | Private module. |
| 16 | `serve/error_codes.rs:38` | `pub const RUNTIME_ERROR` | skip-binary-internal | Private module. |
| 17 | `serve/error_codes.rs:40` | `pub const CAPABILITY_DENIED` | skip-binary-internal | Private module. |
| 18 | `serve/error_codes.rs:42` | `pub const TOOL_ERROR` | skip-binary-internal | Private module. |
| 19 | `serve/error_codes.rs:44` | `pub const LLM_ERROR` | skip-binary-internal | Private module. |
| 20 | `serve/error_codes.rs:46` | `pub const UNKNOWN_AGENT` | skip-binary-internal | Private module. |
| 21 | `serve/error_map.rs:13` | `pub fn from_runtime_error` | skip-binary-internal | Private module (`mod error_map`); not accessible from doctests. |
| 22 | `serve/framing.rs:11` | `pub enum Inbound` | skip-binary-internal | `#[doc(hidden)]` re-export; framing impl detail; variants (`Json`, `ParseError`, `Eof`) have no meaningful standalone construction outside the reader task. |
| 23 | `serve/framing.rs:22` | `pub async fn reader_task` | skip-binary-internal | Private module; reads real stdin — cannot be meaningfully tested in a doctest. |
| 24 | `serve/framing.rs:56` | `pub async fn writer_task` | skip-binary-internal | Private module; writes real stdout — cannot be meaningfully tested in a doctest. |
| 25 | `serve/handshake.rs:12` | `pub struct HandshakeState` | include | Demonstrate `Default`, `mark_handshaken`, `is_handshaken`, and clone-shares-state. Three fences (struct + 2 methods). |
| 26 | `serve/handshake.rs:20` | `pub enum Check` | skip-binary-internal | Private module (`mod handshake`); not re-exported; returned by `HandshakeState::check()` but inaccessible to name in a doctest. |
| 27 | `serve/lifecycle.rs:16` | `pub async fn run` | skip-binary-internal | Private module; full serve loop entry point — starts real runtime and I/O tasks. |
| 28 | `serve/methods.rs:6` | `pub const META_HANDSHAKE` | skip-binary-internal | Private module (`mod methods`); not accessible from doctests. |
| 29 | `serve/methods.rs:9` | `pub const META_PING` | skip-binary-internal | Private module. |
| 30 | `serve/methods.rs:12` | `pub const RUNTIME_RUN` | skip-binary-internal | Private module. |
| 31 | `serve/methods.rs:15` | `pub const RUNTIME_RUN_STREAMING` | skip-binary-internal | Private module. |
| 32 | `serve/methods.rs:18` | `pub const RUNTIME_CANCEL` | skip-binary-internal | Private module. |
| 33 | `serve/methods.rs:21` | `pub const RUNTIME_EVENT` | skip-binary-internal | Private module. |
| 34 | `serve/mod.rs:45` | `pub async fn run` | skip-binary-internal | Accessible as `tau_app::serve::run` but starts the full async serve loop; doctest would need a real project + runtime, exceeding doctest scope. |
| 35 | `serve/options.rs:11` | `pub struct ServeOptions` | include | Construct with struct-update from `Default`; assert custom field values and that `idle_timeout` defaults to `None`. One fence. |
| 36 | `serve/project.rs:34` | `pub struct Project` | skip-binary-internal | `#[doc(hidden)]` re-export; `Project::load` requires a real tau project directory on disk (tau.toml + lockfile); exceeds doctest scope. |
| 37 | `serve/protocol.rs:14` | `pub enum RequestId` | include | Demonstrate `Int` and `Str` variants; use as `HashMap` key (via `Hash + Eq`); assert serde untagged round-trip. One fence. |
| 38 | `serve/protocol.rs:23` | `pub struct Request` | skip-binary-internal | Private module (`mod protocol`); not re-exported; not accessible from doctests. |
| 39 | `serve/protocol.rs:37` | `pub struct Response` | skip-binary-internal | Private module; not re-exported. |
| 40 | `serve/protocol.rs:48` | `pub struct ErrorResponse` | skip-binary-internal | Private module; not re-exported. |
| 41 | `serve/protocol.rs:59` | `pub struct Notification` | skip-binary-internal | Private module; not re-exported. |
| 42 | `serve/protocol.rs:71` | `pub struct ErrorObject` | skip-binary-internal | Private module; not re-exported. |
| 43 | `serve/protocol.rs:84` | `pub enum Outbound` | skip-binary-internal | Re-exported as `tau_app::serve::Outbound` but variants wrap `Response`/`ErrorResponse`/`Notification` which are in a private module and cannot be named or constructed from doctests. |
| 44 | `serve/tracing_init.rs:10` | `pub fn install` | skip-binary-internal | Private module; installs a global tracing subscriber — a global side effect with no meaningful return value to assert. |

**Total:** 44 items — 4 include, 1 skip-reexport, 39 skip-binary-internal.

## Status log

- 2026-05-27 — tau-workflow classifications + 14 includes (PR-A). `cargo test --doc -p tau-workflow` → 19 passed, 0 failed.
- 2026-05-27 — tau-app classifications + 4 includes (PR-B). `cargo test --doc -p tau-app` → 10 passed, 0 failed. Introduced skip-binary-internal category: tau-app's private serve submodules expose pub items unreachable from doctests; all such items use this classification.
