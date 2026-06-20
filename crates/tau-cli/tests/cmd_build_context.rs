//! Integration test: `tau build` rejects an invalid context pipeline at
//! build time and surfaces the diagnostic through the CLI.
//!
//! The IR typecheck `check_context` (run during IR lowering) rejects a
//! context pipeline whose last step is not `fit_budget` with
//! `IrError::ContextFitBudgetNotLast`. `tau build` lowers the project IR
//! (via `lower_ir`), so a typecheck failure must abort the build with a
//! non-zero exit and a readable message — not be silently dropped.
//!
//! `tau check` does NOT lower the IR (its category checks operate on the
//! parsed `ProjectConfig`, not the lowered module), so `tau build` is the
//! verb that triggers `check_context`. See the test-file name.
//!
//! Each test stands up an isolated project tempdir plus a sibling
//! `TAU_HOME` tempdir (with `config.toml` pre-created) so global-scope
//! writes never race on Windows runners — matching the pattern in
//! `cmd_build.rs` / `feedback_windows_tau_home_test_pattern`.

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;
use predicates::prelude::*;

/// A `tau.toml` whose agent declares a context pipeline where `fit_budget`
/// is the FIRST step rather than the last. `check_context` must reject it
/// with `ContextFitBudgetNotLast`.
const TAU_TOML_FIT_BUDGET_NOT_LAST: &str = r#"packages = ["mock-llm"]

[project]
name = "ctx-bad"
version = "0.1.0"

[models]
m = { backend = "mock-llm", model = "claude-haiku-4-5" }

[agents.a]
display_name = "A"
package = "demo@^0.1"
model = "m"

[[agents.a.context.pipeline]]
transformer = "fit_budget"
[agents.a.context.steps.fit_budget]
max_tokens = 4000

[[agents.a.context.pipeline]]
transformer = "trim_old"
[agents.a.context.steps.trim_old]
keep_last_turns = 4
"#;

/// Empty schema-v6 lockfile: no packages, so the install-verify step is a
/// no-op and the build proceeds to IR lowering (where `check_context` runs).
const EMPTY_V6_LOCK: &str = r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#;

/// Build an isolated `TAU_HOME` tempdir under `scratch` with `config.toml`
/// pre-created so parallel tests don't race on its initial write.
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// `tau build` must reject a context pipeline whose last step is not
/// `fit_budget`: exit code 2, and stderr carries the `check_context`
/// diagnostic ("the last context step must be `fit_budget`").
#[test]
fn build_rejects_context_pipeline_with_fit_budget_not_last() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("tau.toml"), TAU_TOML_FIT_BUDGET_NOT_LAST).unwrap();
    std::fs::write(project.join("tau.lock"), EMPTY_V6_LOCK).unwrap();

    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("fit_budget"))
        .stderr(predicate::str::contains("last context step"));
}
