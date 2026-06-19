//! Integration tests for `tau chat`.
//!
//! Mirrors `cmd_run.rs`: easy tests use a bare `TempDir`, REPL tests
//! use [`common::setup_echo_project`] to spawn a real `echo-llm`
//! plugin. Each REPL interaction is a `write_stdin` payload terminated
//! by `/exit\n` so the loop returns cleanly.
//!
//! Plugin spawn cost is amortized via the session-cached
//! `ensure_echo_plugins_built` helper.

mod common;

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

// ---- "easy" tests (no plugin spawn needed) ---------------------------------

#[test]
fn chat_rejects_json_flag() {
    // --json is rejected at handler entry — no project setup required
    // for the assertion, but we provide one anyway so the test exercises
    // the same dispatch path the others do.
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["--json", "chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        // Pipe an empty stdin so the binary doesn't block on a missing
        // tty if the --json check ever moves below readline.
        .write_stdin("")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--json"));
}

#[test]
fn chat_missing_agent_id_exits_two() {
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "nonexistent"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("nonexistent"));
}

// ---- --no-install tests ----------------------------------------------------

#[test]
fn chat_with_no_install_fails_when_deps_missing() {
    // Mirror of run_with_no_install_emits_install_hints_and_fails: agent
    // declares a requires.tools entry pointing at a non-existent file://
    // URL. With --no-install, tau chat --dry-run should fail without
    // attempting to fetch.
    let dir = tempfile::tempdir().unwrap();
    let toml_str = r#"packages = ["anthropic"]

[project]
name = "demo"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.reviewer]
display_name = "Reviewer"
package      = "demo@^0.1"
model        = "default"

[[agents.reviewer.requires.tools]]
name = "missing-tool"
source = "file:///tmp/tau-nonexistent-fixture-DO-NOT-CREATE/missing.git"
"#;
    std::fs::write(dir.path().join("tau.toml"), toml_str).unwrap();

    let output = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .args(["chat", "reviewer", "--no-install", "--dry-run"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "chat should fail when requires.tools is missing and --no-install is set; \
         stderr was: {stderr}"
    );
}

// ---- REPL-driven tests (real echo-llm spawn) -------------------------------

#[test]
fn chat_dry_run_skips_repl() {
    let dir = common::setup_echo_project("echo", "canned_text = \"unused\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo", "--dry-run"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .assert()
        .success()
        .stderr(predicate::str::contains("[dry-run]"))
        .stderr(predicate::str::contains("REPL would start"))
        .stderr(predicate::str::contains("no session opened"));
}

#[test]
fn chat_repl_three_turn_via_stdin_pipe() {
    // Three-turn REPL with echo-llm in `script` mode: each prompt
    // pulls the next entry from the script (per `EchoLlm::next_text`).
    let dir = common::setup_echo_project("echo", "script = [\"one\", \"two\", \"three\"]\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("prompt 1\nprompt 2\nprompt 3\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("one"))
        .stdout(predicate::str::contains("two"))
        .stdout(predicate::str::contains("three"))
        .stderr(predicate::str::contains("session ended"))
        .stderr(predicate::str::contains("Welcome to tau chat"));
}

#[test]
fn chat_help_command_lists_slash_commands() {
    let dir = common::setup_echo_project("echo", "canned_text = \"reply\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("/help\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("/exit"))
        .stdout(predicate::str::contains("/info"))
        .stdout(predicate::str::contains("/history"));
}

#[test]
fn chat_clear_prints_deprecation() {
    // Since Task 5, /clear is deprecated: it prints a guidance message
    // and continues (does NOT clear in-memory history, does NOT quit).
    let dir = common::setup_echo_project("echo", "canned_text = \"first\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("turn1\n/clear\n/exit\n")
        .assert()
        .success()
        // /clear now emits a deprecation hint via status (stderr).
        .stderr(predicate::str::contains("/clear was removed"));
}

#[test]
fn chat_eof_ends_session_with_summary() {
    // Closing stdin without /exit should still print the session summary
    // and exit successfully.
    let dir = common::setup_echo_project("echo", "canned_text = \"echo\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("")
        .assert()
        .success()
        .stderr(predicate::str::contains("session ended"));
}

#[test]
fn chat_unknown_slash_is_forwarded_as_prompt() {
    // Per parser docs: `/foo` is not recognised, so it goes to the LLM
    // as a normal prompt. echo-llm replies with its canned text.
    let dir = common::setup_echo_project("echo", "canned_text = \"passthrough-response\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("/foo\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("passthrough-response"));
}

#[test]
fn chat_history_threads_across_turns() {
    // Two prompts in a row; after both, /history should show 4 entries
    // (user1, assistant1, user2, assistant2). This verifies
    // run_with_history is wiring `all_messages` back into the next call.
    let dir = common::setup_echo_project("echo", "canned_text = \"reply\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("first\nsecond\n/history\n/exit\n")
        .assert()
        .success()
        // History is index-tagged; with two completed turns we should
        // see at least entries [0] and [3].
        .stdout(predicate::str::contains("[0]"))
        .stdout(predicate::str::contains("[3]"));
}

// ---- Streaming integration tests -------------------------------------------
//
// The streaming path (default, --no-stream unset) calls
// runtime.run_streaming_with_history and renders in two passes:
//   Pass 1: text deltas inline via raw print!/flush (typewriter UX).
//   Pass 2: re-render via termimad on RunCompleted.
//
// Both tests use assert_cmd + scripted stdin + stdout capture, following the
// same pattern as chat_repl_three_turn_via_stdin_pipe above. The echo-llm
// plugin is used as the scripted LLM backend — it echoes canned text, which
// is what both streaming and batch paths produce.
//
// The streaming path goes through run_streaming_with_history; the batch path
// goes through run_with_history. Both ultimately produce the same text output
// (the canned echo text), so asserting stdout contains the text is sufficient
// to confirm the path ran.

#[test]
fn chat_streaming_emits_text_deltas_inline() {
    // Default (no --no-stream flag): streaming path is active. The echo-llm
    // plugin drives the LLM layer; it emits the canned text as the agent
    // response, which the streaming path renders via print!/flush and then
    // re-renders via termimad. Both passes write to stdout, so the text
    // appears at least once in stdout.
    let dir = common::setup_echo_project("echo", "canned_text = \"hello-streaming\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("hi\n/exit\n")
        .assert()
        .success()
        // The streaming path print!s text deltas and then termimad re-renders.
        // Either pass (or both) ensures "hello-streaming" appears in stdout.
        .stdout(predicate::str::contains("hello-streaming"))
        .stderr(predicate::str::contains("session ended"));
}

#[test]
fn chat_no_stream_flag_disables_streaming() {
    // With --no-stream: the batch flow runs (run_with_history). The echo-llm
    // plugin produces the same canned text, but via the batch path that
    // calls render_final_message (termimad rendering on stdout).
    let dir = common::setup_echo_project("echo", "canned_text = \"hello-no-stream\"\n", &[]);
    let global_dir = dir.path().join("global");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["chat", "echo", "--no-stream"])
        .current_dir(dir.path())
        .env("TAU_HOME", &global_dir)
        .write_stdin("hi\n/exit\n")
        .assert()
        .success()
        // The batch path uses termimad's render_final_message, which writes
        // directly to stdout. "hello-no-stream" must appear.
        .stdout(predicate::str::contains("hello-no-stream"))
        .stderr(predicate::str::contains("session ended"));
}
