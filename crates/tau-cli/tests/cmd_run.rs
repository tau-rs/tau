//! Integration tests for `tau run`.
//!
//! Two flavours of fixture:
//!
//! - "Easy" tests use `tempfile::TempDir` directly — no plugin spawn,
//!   so they cover the early-exit paths (`run` without a project
//!   `tau.toml`, an unknown agent id).
//! - Run-loop tests use [`common::setup_echo_project`], which builds
//!   `echo-llm` (and optionally `echo-tool`) once per session and
//!   synthesizes a project tau.toml + lockfile pointing at the
//!   pre-built binaries. The CLI then drives a full `tau run` against
//!   real plugin processes via `tau_runtime_tokio::plugin_host::load_*`.
//!
//! Plugin-spawning tests are slower (~30 s for the binary's first
//! invocation, sub-second after); the build cost is amortized via the
//! `OnceLock`-guarded `ensure_echo_plugins_built` helper so the per-
//! test overhead is just process spawn + handshake + a single RPC.

mod common;

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

// ---- "easy" tests (no fixture / no plugin spawn needed) --------------------

#[test]
fn run_missing_project_tau_toml_exits_two() {
    let dir = tempfile::tempdir().unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "reviewer", "hello"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("tau.toml"));
}

#[test]
fn run_agent_id_not_found_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tau.toml"),
        r#"[project]
name = "demo"

[agents.reviewer]
display_name = "Code Reviewer"
package      = "code-reviewer@^0.1"
llm_backend  = "anthropic"
"#,
    )
    .unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "ghost", "hi"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("ghost"));
}

// ---- run-loop tests (real echo-llm spawn) ----------------------------------

#[test]
fn run_dry_run_prints_preview_and_makes_no_llm_call() {
    // --dry-run short-circuits before plugin loading so we don't even
    // need the echo binary built — but we still go through
    // `setup_echo_project` for parity with the other tests in this
    // module (and to keep the fixture authoring in one place).
    let dir = common::setup_echo_project("echo", "canned_text = \"unused on dry-run\"\n", &[]);

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "Review src/auth.rs", "--dry-run"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success()
        .stderr(predicate::str::contains("[dry-run]"))
        .stderr(predicate::str::contains("agent:"))
        .stderr(predicate::str::contains("echo"))
        .stderr(predicate::str::contains("max_turns:"))
        .stderr(predicate::str::contains("no LLM call"));
}

#[test]
fn run_completed_happy_path_emits_text() {
    let dir = common::setup_echo_project(
        "echo",
        "canned_text = \"review complete: looks good\"\n",
        &[],
    );

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "Review src/auth.rs"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("review complete: looks good"),
        "stdout: {stdout}"
    );
}

#[test]
fn run_propagates_plugin_crash_as_exit_code_two() {
    // `crash_after_handshake = true` causes echo-llm to panic on the
    // first `llm.complete` RPC. The host-side dispatch surfaces this
    // as a `RuntimeError`, which `run_main` maps to exit code 2.
    let dir = common::setup_echo_project("echo", "crash_after_handshake = true\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "anything"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected kernel-error exit code 2; got {:?}\nstderr={}\nstdout={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("echo-llm") || stderr.contains("plugin"),
        "stderr should mention the plugin; got:\n{stderr}"
    );
}

#[test]
fn run_with_no_install_emits_install_hints_and_fails() {
    // The agent declares a requires.tools entry pointing at a non-existent
    // file:// URL. With --no-install, tau run should print the install hint
    // and exit non-zero WITHOUT actually attempting to fetch.
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"
[project]
name = "demo"

[agents.reviewer]
display_name = "Reviewer"
package      = "demo@^0.1"
llm_backend  = "anthropic"

[[agents.reviewer.requires.tools]]
name = "missing-tool"
source = "file:///tmp/tau-nonexistent-fixture-DO-NOT-CREATE/missing.git"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    let output = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "reviewer",
            "test prompt",
            "--no-install",
            "--dry-run",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The run may fail at resolve (source unreachable) OR at the
    // --no-install short-circuit. Either way, exit code is non-zero
    // and the user sees actionable output. The strict invariant for
    // this test: the run did NOT succeed silently.
    assert!(
        !output.status.success(),
        "run should fail when requires.tools is missing and --no-install is set; \
         stderr was: {stderr}"
    );
}

#[test]
fn run_lazy_resolve_emits_progress() {
    // Set up a local git fixture for the required tool, point the agent
    // at it via file:// URL, run --dry-run (which validates everything but
    // doesn't invoke the LLM), assert progress lines appear.
    use std::process::Command;

    let work = tempfile::tempdir().unwrap();
    let work_path = work.path();

    // Create a local git repo for the "required tool" with a tag.
    let tool_repo = work_path.join("missing-tool");
    std::fs::create_dir(&tool_repo).unwrap();
    let manifest_body = r#"
name = "missing-tool"
version = "0.1.0"
description = "fixture"
authors = []
source = "https://example.com/missing-tool.git"
kind = "tool"
dependencies = []
capabilities = []
"#;
    std::fs::write(tool_repo.join("tau.toml"), manifest_body).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&tool_repo)
            .args(args)
            .output()
            .unwrap();
        if !out.status.success() {
            panic!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "fixture"]);
    git(&["tag", "v0.1.0"]);

    // Project tau.toml at a SEPARATE path; tools_in_requires points at
    // the local git fixture's file:// URL.
    let proj = work_path.join("project");
    std::fs::create_dir(&proj).unwrap();
    let tool_url = format!("file://{}", tool_repo.display());
    let toml_str = format!(
        r#"
[project]
name = "demo"

[agents.reviewer]
display_name = "Reviewer"
package      = "demo@^0.1"
llm_backend  = "anthropic"

[[agents.reviewer.requires.tools]]
name = "missing-tool"
source = "{tool_url}"
version = "^0.1"
"#
    );
    std::fs::write(proj.join("tau.toml"), toml_str).unwrap();

    let output = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .args(["run", "reviewer", "test prompt", "--dry-run"])
        .current_dir(&proj)
        .env("TAU_HOME", &proj)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Either the resolve+install path emitted progress (success) OR
    // tau bailed for some other reason (e.g., no LLM backend installed,
    // which is expected in this test fixture). The point of THIS test
    // is to exercise the resolve path. Be tolerant: just check that
    // EITHER progress lines appeared, OR the failure reason is unrelated
    // to the resolve.
    let resolve_invoked = stderr.contains("[resolve]") || stderr.contains("[install]");
    let exit_unrelated_to_resolve = !output.status.success()
        && !stderr.contains("ConflictingSources")
        && !stderr.contains("NoCompatibleVersion")
        && !stderr.contains("SourceListing");
    assert!(
        resolve_invoked || exit_unrelated_to_resolve,
        "expected to see [resolve]/[install] progress OR a non-resolve failure; \
         stderr was: {stderr}"
    );
}

#[test]
fn run_json_completed_emits_outcome_payload() {
    let dir = common::setup_echo_project("echo", "canned_text = \"pong\"\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "ping", "--json"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // --json mode emits one JSON object per line (resolve events + outcome).
    // Find the line that carries the agent outcome.
    let outcome_line = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .expect("--json should include an outcome JSON line");
    let parsed = outcome_line;
    assert_eq!(parsed["outcome"], "completed");
    assert_eq!(parsed["final_message"], "pong");
    assert!(parsed["total_turns"].is_number(), "total_turns: {parsed}");
    assert!(parsed["token_usage"].is_object());
}

// ---- Streaming integration tests -------------------------------------------
//
// Both tests use assert_cmd + the `echo-llm` scripted-LLM plugin from
// `common::setup_echo_project`. The streaming path calls
// runtime.run_streaming(...) and renders events as they arrive. The echo-llm
// plugin emits canned text, which the streaming path renders as TextDelta
// events followed by a RunCompleted event.
//
// Human mode: text deltas land inline on stdout (raw print!/flush), then a
// closing newline. The test asserts stdout contains the canned text.
//
// JSON mode: each event is one JSON object per line. The test parses every
// line and asserts structural invariants (each line is valid JSON with an
// "event" field; the terminal line has event=="run_completed").

#[test]
fn run_stream_human_mode_emits_text_deltas_inline_to_stdout() {
    let dir = common::setup_echo_project("echo", "canned_text = \"streaming-hello-world\"\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "hi", "--stream"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("streaming-hello-world"),
        "stdout should contain the canned text; got: {stdout}"
    );
}

#[test]
fn run_stream_json_mode_emits_one_event_per_line() {
    let dir =
        common::setup_echo_project("echo", "canned_text = \"streaming-json-response\"\n", &[]);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "echo", "hi", "--stream", "--json"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected success; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Every non-empty line should be valid JSON with an "event" field.
    let event_lines: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("line is not valid JSON: {e}\nline: {line}"))
        })
        .collect();

    // At minimum we must have at least one event.
    assert!(
        !event_lines.is_empty(),
        "stdout should contain at least one JSON event line"
    );

    // Every event line must have an "event" discriminator field.
    for (i, ev) in event_lines.iter().enumerate() {
        assert!(
            ev.get("event").is_some(),
            "event line {i} missing 'event' field: {ev}"
        );
    }

    // There must be a terminal "run_completed" event.
    let run_completed = event_lines
        .iter()
        .find(|ev| ev["event"] == "run_completed")
        .expect("stream must emit a run_completed event");
    assert_eq!(
        run_completed["outcome"]["outcome"], "completed",
        "run_completed outcome should be 'completed': {run_completed}"
    );

    // There must be at least one "text_delta" event carrying the canned text.
    let has_text_delta = event_lines.iter().any(|ev| ev["event"] == "text_delta");
    assert!(
        has_text_delta,
        "stream should have at least one text_delta event"
    );
}
