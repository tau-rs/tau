//! Integration tests for `tau plugin run` and `tau plugin protocol decode`
//! (spec §9 / §10.3 debug tier).
//!
//! Both commands are exercised against the real `echo-llm` binary
//! produced by [`common::echo_plugins::ensure_echo_plugins_built`].
//! The recording -> decode pipeline is end-to-end: a real `tau run`
//! invocation produces a JSONL transcript via `--record-protocol`,
//! and a separate `tau plugin protocol decode` invocation reads that
//! file back.

mod common;

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

#[test]
fn plugin_run_interactive_dispatches_meta_describe() {
    let echo_llm = common::echo_plugins::echo_llm_binary();

    // Drive a single `meta.describe` request through the interactive
    // REPL. The REPL prints the response to stdout and we close the
    // session via `exit`. EOF (closing stdin) also works but `exit`
    // exercises the explicit-quit path.
    let stdin = "meta.describe\nexit\n";
    let assert = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["plugin", "run", echo_llm.to_str().unwrap(), "--interactive"])
        .write_stdin(stdin)
        .assert();

    // `tau plugin run` exits success on a clean session. The status
    // line on stderr confirms the handshake reached the plugin.
    assert
        .success()
        .stderr(predicate::str::contains("echo-llm"))
        .stderr(predicate::str::contains("Connected to plugin"));
}

#[test]
fn plugin_protocol_decode_emits_human_readable_transcript() {
    // Step 1: run a real `tau run` invocation against echo-llm with
    // `--record-protocol <path>` so the host writes a JSONL recording.
    let dir = common::setup_echo_project("echo", "canned_text = \"protocol decode smoke\"\n", &[]);
    let log_path = dir.path().join("wire.log");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "--record-protocol",
            log_path.to_str().unwrap(),
            "run",
            "--allow-ungoverned",
            "echo",
            "ping",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success();

    assert!(
        log_path.exists(),
        "recording file should exist at {}",
        log_path.display()
    );
    let recorded = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        !recorded.trim().is_empty(),
        "recording file should not be empty; contents:\n{recorded}"
    );

    // Step 2: decode the recording and check the transcript surfaces
    // both directions plus the canonical method names.
    let decode = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["plugin", "protocol", "decode", log_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert();

    decode
        .success()
        // dir markers from the recording layer (host->plugin / plugin->host).
        .stdout(predicate::str::contains("h2p"))
        .stdout(predicate::str::contains("p2h"))
        // Wire methods that appear on a happy-path turn after the
        // (un-recorded) handshake completes. run_with_history now
        // delegates to run_streaming_with_history, so the kernel uses
        // the streaming LLM method (llm.stream) rather than the batch
        // method (llm.complete) on every run.
        .stdout(predicate::str::contains("llm.stream"));
}

#[test]
fn plugin_protocol_decode_json_emits_structured_lines() {
    let dir = common::setup_echo_project(
        "echo",
        "canned_text = \"protocol decode json smoke\"\n",
        &[],
    );
    let log_path = dir.path().join("wire.log");

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "--record-protocol",
            log_path.to_str().unwrap(),
            "run",
            "--allow-ungoverned",
            "echo",
            "ping",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .assert()
        .success();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "plugin",
            "protocol",
            "decode",
            log_path.to_str().unwrap(),
            "--json",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "decode --json failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Each non-empty line should parse as a JSON object with the
    // canonical recording fields.
    let mut saw_complete = false;
    let mut line_count = 0usize;
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        line_count += 1;
        let v: serde_json::Value =
            serde_json::from_str(line).expect("each --json line must be valid JSON");
        assert!(v.get("plugin").is_some(), "missing `plugin`: {line}");
        assert!(v.get("dir").is_some(), "missing `dir`: {line}");
        // run_with_history now delegates to run_streaming_with_history,
        // so the kernel uses llm.stream (not llm.complete) on every run.
        if v["method"] == "llm.stream" {
            saw_complete = true;
        }
    }
    assert!(line_count > 0, "decoded transcript was empty");
    assert!(saw_complete, "decoded transcript missing llm.stream");
}

/// How the caller silenced the console for the tau-rs/tau#694 regression
/// tests: `--quiet` (flag path, `tau=warn`) or `RUST_LOG=error` (env path,
/// which overrides the flags entirely).
enum Silencer {
    QuietFlag,
    RustLogError,
}

/// tau-rs/tau#694: `--record-protocol` is an explicit request for a file.
/// Console verbosity must not decide whether that file gets frames.
///
/// This is the second half of #694. `PluginRecordingLayer` was already
/// wrapped in a per-layer `filter_fn` + `max_level_hint(TRACE)` commented
/// as bypassing the global `EnvFilter` — it never did. `install()` layered
/// the `EnvFilter` on top of the registry, where it is a *global* filter,
/// and a global filter still gates layers that carry their own per-layer
/// filter. Under `--quiet` (`tau=warn`) the INFO-level `tau::plugin::frame`
/// events were dropped before the recording layer saw them, leaving an
/// empty recording and a silently useless `tau plugin protocol decode`.
fn assert_protocol_recording_survives(silencer: Silencer) {
    let dir = common::setup_echo_project("echo", "canned_text = \"quiet frames\"\n", &[]);
    let log_path = dir.path().join("wire.log");

    let mut cmd = AssertCmd::cargo_bin("tau").unwrap();
    cmd.current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"));
    match silencer {
        Silencer::QuietFlag => {
            cmd.arg("--quiet");
            cmd.env_remove("RUST_LOG");
        }
        Silencer::RustLogError => {
            cmd.env("RUST_LOG", "error");
        }
    }
    cmd.args([
        "--record-protocol",
        log_path.to_str().unwrap(),
        "run",
        "--allow-ungoverned",
        "echo",
        "ping",
    ])
    .assert()
    .success();

    let recorded = std::fs::read_to_string(&log_path).unwrap_or_else(|e| {
        panic!(
            "recording {} must exist with the console silenced: {e}",
            log_path.display()
        )
    });
    let frames: Vec<serde_json::Value> = recorded
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each recorded line is JSON"))
        .collect();
    assert!(
        !frames.is_empty(),
        "recording must not be empty; contents:\n{recorded}"
    );
    // Both directions of a real turn, not just the opening frame.
    assert!(
        frames.iter().any(|f| f["dir"] == "h2p"),
        "recording missing a host->plugin frame; contents:\n{recorded}"
    );
    assert!(
        frames.iter().any(|f| f["dir"] == "p2h"),
        "recording missing a plugin->host frame; contents:\n{recorded}"
    );
    assert!(
        frames.iter().any(|f| f["method"] == "llm.stream"),
        "recording missing the llm.stream call; contents:\n{recorded}"
    );
}

#[test]
fn record_protocol_writes_frames_even_when_quiet() {
    assert_protocol_recording_survives(Silencer::QuietFlag);
}

#[test]
fn record_protocol_writes_frames_under_rust_log_error() {
    assert_protocol_recording_survives(Silencer::RustLogError);
}
