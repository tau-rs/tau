//! Integration: `tau dev -p` triggers one-shot mode and exits — never hangs
//! waiting for stdin like the interactive REPL would.
//!
//! The test exercises the wiring:
//! 1. `-p` routes through `cmd::dev::run` → `session.run_turn` (not REPL).
//! 2. `run_turn` loads the project, attempts to build the runtime, and exits
//!    cleanly (success or failure exit code) rather than hanging.
//!
//! Using an agent with `llm_backend = "anthropic"` (not locally installed in
//! test scope): `run_turn` will fail at plugin load time and propagate the
//! error, causing a non-zero exit. The critical property asserted here is
//! `status.code().is_some()` — the process exited (was not killed by the
//! 20-second timeout).

use assert_fs::prelude::*;

/// Minimal tau.toml with one agent that references an llm backend and an MCP
/// tool via cassette URL. The cassette path is absolute (embedded via concat!
/// macro) so the test project can live in a tmpdir.
fn cassette_project_toml(cassette_path: &std::path::Path) -> String {
    format!(
        r#"
[project]
name = "dev-one-shot-test"
version = "0.0.1"

[tools.weather]
mcp = "cassette:{cassette}"

[agents.forecaster]
display_name = "Forecaster"
package      = "forecaster@^0.1"
llm_backend  = "anthropic"
model        = "claude-3-haiku-20240307"
prompt.system = "You are a weather forecaster."
tool_refs    = ["weather"]
"#,
        cassette = cassette_path.display()
    )
}

#[test]
fn dev_one_shot_exits_gracefully_without_hanging() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");

    // Absolute path to the weather cassette fixture in tau-mcp-tokio.
    let cassette_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl");

    let toml_content = cassette_project_toml(&cassette_src);
    tmp.child("tau.toml")
        .write_str(&toml_content)
        .expect("write tau.toml");

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("tau binary");
    let output = cmd
        .current_dir(tmp.path())
        .args(["dev", ".", "-p", "what is the weather in Paris?"])
        .timeout(std::time::Duration::from_secs(20))
        .output()
        .expect("run tau dev -p");

    // The critical assertion: the process must EXIT (not be killed by timeout).
    // A SIGKILL (or panic on timeout) leaves status.code() == None.
    // Whether it exits 0 (success) or non-zero (plugin load failure because
    // the lockfile / anthropic backend is absent in the test scope) is
    // intentionally not asserted — both are valid graceful exits.
    assert!(
        output.status.code().is_some(),
        "tau dev -p must EXIT cleanly (not be killed by timeout or panic);\
         \n  status = {:?}\n  stdout = {}\n  stderr = {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn dev_one_shot_project_load_error_is_reported_not_hung() {
    // When the project directory doesn't contain tau.toml, the process
    // must exit quickly with a non-zero code and a helpful error message.
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    // Intentionally do NOT write tau.toml.

    let mut cmd = assert_cmd::Command::cargo_bin("tau").expect("tau binary");
    let output = cmd
        .current_dir(tmp.path())
        .args(["dev", ".", "-p", "hello"])
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .expect("run tau dev -p on bad project");

    assert!(
        output.status.code().is_some(),
        "process must exit (not hang) when tau.toml is missing;\
         \n  status = {:?}",
        output.status
    );
    assert!(
        !output.status.success(),
        "exit code must be non-zero when project load fails"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");
    assert!(
        combined.contains("tau.toml") || combined.contains("not found") || combined.contains("No such file") || combined.contains("error"),
        "expected an error mentioning tau.toml or 'not found'; got: stderr={stderr}, stdout={stdout}"
    );
}
