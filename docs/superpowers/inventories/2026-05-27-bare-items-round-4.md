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

## tau-observe

The `pub mod capture` and `pub mod otlp` are behind `cfg(any(feature = "test-fixtures", test))` and `cfg(feature = "otlp")` respectively; all items inside those modules are `skip-feature-gated`. The `git grep` enumeration command used in the task (using `\s` in ERE with `git grep`) missed impl-block `pub fn` methods; 9 additional methods were enumerated with a plain `grep -nE '^\s+pub '` pass and appear as rows 60–68.

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|
| 1 | `capture.rs:32` | `pub struct CapturedEvent` | skip-feature-gated | Behind `cfg(any(feature = "test-fixtures", test))`; not reachable from default doctests. |
| 2 | `capture.rs:47` | `pub struct Captor` | skip-feature-gated | Same gate as `CapturedEvent`. |
| 3 | `filter.rs:15` | `pub fn env_or_directive` | include | Free fn with verifiable behavior: RUST_LOG-unset path returns filter matching fallback directive. |
| 4 | `install.rs:13` | `pub enum Format` | include | Two variants (`Human`, `Json`); assert `Copy` semantics and inequality. |
| 5 | `install.rs:22` | `pub enum Writer` | include | Two variants (`Stderr`, `Stdout`); assert `Copy` semantics and inequality. |
| 6 | `install.rs:31` | `pub enum Rotation` | include | Three variants; assert `Default` yields `Never`; assert all variants distinct. |
| 7 | `install.rs:42` | `pub struct InstallOptions` | include | Construct via `cli_default()`; assert `format`, `writer`, `non_blocking`, `file_path`, `rotation`, `extra_layers`. |
| 8 | `install.rs:132` | `pub enum InstallError` | include | Single `AlreadyInstalled` variant; assert `Display` contains `"already installed"`. |
| 9 | `install.rs:147` | `pub struct InstallGuard` | skip-trivial | Opaque RAII guard; no public constructor; held by caller for its `Drop`. |
| 10 | `install.rs:216` | `pub fn install` | include | `no_run` fence — installs process-global tracing subscriber; parallel doctests would race on global init. Shows constructor call shape with `.expect()`. |
| 11 | `layers/mod.rs:5` | `pub mod plugin_recording` | skip-reexport | Module declaration. |
| 12 | `layers/mod.rs:6` | `pub mod workflow_run_log` | skip-reexport | Module declaration. |
| 13 | `layers/plugin_recording.rs:36` | `pub const TARGET` | skip-trivial | String constant (`"tau::plugin::frame"`); documented by docstring; asserted inside layer struct fences. |
| 14 | `layers/plugin_recording.rs:47` | `pub struct PluginRecordingLayer` | include | Struct fence: construct via `new(PathBuf)`, verify `Clone` is cheap, assert `TARGET` value. |
| 15 | `layers/workflow_run_log.rs:33` | `pub const TARGET` | skip-trivial | String constant (`"tau::workflow::step"`); documented by docstring; asserted inside layer struct fence. |
| 16 | `layers/workflow_run_log.rs:37` | `pub struct WorkflowRunLogLayer` | include | Struct fence: construct via `new(PathBuf)`, verify `Clone` is cheap, assert `TARGET` value. |
| 17 | `lib.rs:9` | `pub mod capture` | skip-feature-gated | Gated by `cfg(any(feature = "test-fixtures", test))`; not a stable module path in production. |
| 18 | `lib.rs:10` | `pub mod filter` | skip-reexport | Module declaration. |
| 19 | `lib.rs:11` | `pub mod install` | skip-reexport | Module declaration. |
| 20 | `lib.rs:12` | `pub mod layers` | skip-reexport | Module declaration. |
| 21 | `lib.rs:14` | `pub mod otlp` | skip-feature-gated | Gated by `cfg(feature = "otlp")`; not reachable from default doctests. |
| 22 | `lib.rs:15` | `pub mod preview` | skip-reexport | Module declaration. |
| 23 | `lib.rs:16` | `pub mod vocabulary` | skip-reexport | Module declaration. |
| 24 | `otlp.rs:10` | `pub struct OtlpEndpoint` | skip-feature-gated | Behind `feature = "otlp"`. |
| 25 | `preview.rs:22` | `pub fn preview` | include | Free fn; assert short string passes through; assert 300-byte string truncated with `'…'` ellipsis. |
| 26 | `preview.rs:49` | `pub fn preview_json` | include | Free fn; assert small JSON renders as compact JSON; assert large JSON truncated with ellipsis. |
| 27 | `preview.rs:68` | `pub fn full` | include | Free fn; assert 1000-byte string returned in full (no truncation). |
| 28 | `preview.rs:83` | `pub fn full_json` | include | Free fn; assert full JSON contains original data and does not end with `'…'`. |
| 29 | `vocabulary.rs:10` | `pub const SPAN_RUNTIME_AGENT_RUN` | skip-trivial | Documented `&str` constant; value asserted by in-module unit tests. |
| 30 | `vocabulary.rs:12` | `pub const SPAN_RUNTIME_TURN` | skip-trivial | Documented `&str` constant. |
| 31 | `vocabulary.rs:14` | `pub const SPAN_LLM_COMPLETE` | skip-trivial | Documented `&str` constant. |
| 32 | `vocabulary.rs:16` | `pub const SPAN_DISPATCH_TOOL` | skip-trivial | Documented `&str` constant. |
| 33 | `vocabulary.rs:18` | `pub const SPAN_CAPABILITY_CHECK` | skip-trivial | Documented `&str` constant. |
| 34 | `vocabulary.rs:20` | `pub const SPAN_TOOL_SESSION_OPEN` | skip-trivial | Documented `&str` constant. |
| 35 | `vocabulary.rs:22` | `pub const SPAN_TOOL_INVOKE` | skip-trivial | Documented `&str` constant. |
| 36 | `vocabulary.rs:24` | `pub const SPAN_TOOL_SESSION_CLOSE` | skip-trivial | Documented `&str` constant. |
| 37 | `vocabulary.rs:29` | `pub const EV_RUNTIME_RUN_STARTED` | skip-trivial | Documented `&str` constant. |
| 38 | `vocabulary.rs:31` | `pub const EV_RUNTIME_COMPLETED` | skip-trivial | Documented `&str` constant. |
| 39 | `vocabulary.rs:33` | `pub const EV_RUNTIME_FAILED` | skip-trivial | Documented `&str` constant. |
| 40 | `vocabulary.rs:35` | `pub const EV_RUNTIME_LOOP_TERMINATED` | skip-trivial | Documented `&str` constant. |
| 41 | `vocabulary.rs:37` | `pub const EV_RUNTIME_MAX_TURNS_REACHED` | skip-trivial | Documented `&str` constant. |
| 42 | `vocabulary.rs:39` | `pub const EV_RUNTIME_TURN_STARTED` | skip-trivial | Documented `&str` constant. |
| 43 | `vocabulary.rs:44` | `pub const EV_LLM_REQUEST_BUILT` | skip-trivial | Documented `&str` constant. |
| 44 | `vocabulary.rs:46` | `pub const EV_LLM_RESPONSE_RECEIVED` | skip-trivial | Documented `&str` constant. |
| 45 | `vocabulary.rs:48` | `pub const EV_LLM_TOKEN_USAGE` | skip-trivial | Documented `&str` constant. |
| 46 | `vocabulary.rs:50` | `pub const EV_LLM_STOP_REASON` | skip-trivial | Documented `&str` constant. |
| 47 | `vocabulary.rs:52` | `pub const EV_LLM_TOOL_USE_EMITTED` | skip-trivial | Documented `&str` constant. |
| 48 | `vocabulary.rs:57` | `pub const EV_DISPATCH_TOOL_RESOLVED` | skip-trivial | Documented `&str` constant. |
| 49 | `vocabulary.rs:62` | `pub const EV_CAPABILITY_REQUIRED_LOADED` | skip-trivial | Documented `&str` constant. |
| 50 | `vocabulary.rs:64` | `pub const EV_CAPABILITY_GRANTED_LOADED` | skip-trivial | Documented `&str` constant. |
| 51 | `vocabulary.rs:66` | `pub const EV_CAPABILITY_SATISFIES_CHECK` | skip-trivial | Documented `&str` constant. |
| 52 | `vocabulary.rs:68` | `pub const EV_CAPABILITY_ALLOW` | skip-trivial | Documented `&str` constant. |
| 53 | `vocabulary.rs:70` | `pub const EV_CAPABILITY_DENY` | skip-trivial | Documented `&str` constant. |
| 54 | `vocabulary.rs:75` | `pub const EV_TOOL_ARGS_RECEIVED` | skip-trivial | Documented `&str` constant. |
| 55 | `vocabulary.rs:77` | `pub const EV_TOOL_RESULT_RECEIVED` | skip-trivial | Documented `&str` constant. |
| 56 | `vocabulary.rs:79` | `pub const EV_TOOL_INVOKE_FAILED` | skip-trivial | Documented `&str` constant. |
| 57 | `vocabulary.rs:81` | `pub const EV_TOOL_SESSION_OPEN_FAILED` | skip-trivial | Documented `&str` constant. |
| 58 | `vocabulary.rs:83` | `pub const EV_TOOL_SESSION_CLOSE_FAILED` | skip-trivial | Documented `&str` constant. |
| 59 | `vocabulary.rs:88` | `pub const EV_MESSAGE_ADDED` | skip-trivial | Documented `&str` constant. |
| 60 | `capture.rs:53` | `Captor::new` | skip-feature-gated | Behind `test-fixtures` feature; not reachable from default doctests. |
| 61 | `capture.rs:59` | `Captor::subscriber` | skip-feature-gated | Behind `test-fixtures` feature. |
| 62 | `capture.rs:67` | `Captor::events` | skip-feature-gated | Behind `test-fixtures` feature. |
| 63 | `install.rs:153` | `InstallOptions::cli_default` | include | Constructor fence: assert `format == Human`, `writer == Stderr`. |
| 64 | `install.rs:177` | `InstallOptions::plugin_sdk` | include | Constructor fence: assert `format == Json`, `writer == Stderr`. |
| 65 | `layers/workflow_run_log.rs:74` | `WorkflowRunLogLayer::new` | include | Constructor fence: `new(PathBuf)` + verify `Clone`. |
| 66 | `layers/plugin_recording.rs:91` | `PluginRecordingLayer::new` | include | Constructor fence: `new(PathBuf)` + verify `Clone`. |
| 67 | `layers/plugin_recording.rs:120` | `PluginRecordingLayer::flush` | include | `no_run` fence — async method requiring active tokio runtime and live write cycle; shows call shape inside `#[tokio::main]`. |
| 68 | `otlp.rs:22` | `OtlpEndpoint::from_env` | skip-feature-gated | Behind `feature = "otlp"`. |

**Total:** 68 items — 18 include (16 runnable + 2 no_run), 34 skip-trivial, 7 skip-reexport, 9 skip-feature-gated.

Note: rows 60–68 cover impl-block `pub fn` methods missed by the `git grep` enumeration (git grep's ERE does not support `\s`; methods indented inside `impl` blocks require a plain `grep -nE '^\s+pub '` pass).

## Status log

- 2026-05-27 — tau-workflow classifications + 14 includes (PR-A). `cargo test --doc -p tau-workflow` → 19 passed, 0 failed.
- 2026-05-27 — tau-app classifications + 4 includes (PR-B). `cargo test --doc -p tau-app` → 10 passed, 0 failed. Introduced skip-binary-internal category: tau-app's private serve submodules expose pub items unreachable from doctests; all such items use this classification.
- 2026-05-27 — tau-observe classifications + 18 includes (PR-C). `cargo test --doc -p tau-observe` → 18 passed, 0 failed. Closes round 4 — all 3 in-scope tier-2 crates (tau-workflow, tau-app, tau-observe) now have load-bearing doctest coverage. Note: git grep ERE does not support `\s`; 9 impl-block pub methods were enumerated separately and added as rows 60–68.
