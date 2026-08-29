//! Integration tests for the global `--log-non-blocking` flag.
//!
//! Two contracts live here:
//!
//! - Sub-project E Task 4: `--log-non-blocking` requires `--log-file`.
//!   When the invocation supplies the former without the latter, the
//!   CLI must exit with a non-zero status AND a recognizable message on
//!   stderr.
//! - tau-rs/tau#699: choosing the non-blocking file sink must not change
//!   which on-disk artifacts a run produces. The run log and the
//!   `--record-protocol` recording are asserted by content, not by exit
//!   code.
//!
//! Every test scrubs `TAU_LOG_FILE` / `TAU_LOG_NON_BLOCKING` /
//! `TAU_LOG_ROTATION` from the inherited env so a developer's shell
//! export cannot mask the configuration under test. `TAU_HOME` is
//! pointed at a per-test tempdir to keep us off the user's real `~/.tau`
//! and to avoid the Windows config-write race documented in memory.

mod common;

use assert_cmd::Command;

#[test]
fn non_blocking_without_log_file_exits_with_error_message() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("tau")
        .unwrap()
        .args(["--log-non-blocking", "list", "packages"])
        .env("HOME", tmp.path())
        .env("TAU_HOME", tmp.path())
        .env_remove("TAU_LOG_FILE")
        .env_remove("TAU_LOG_NON_BLOCKING")
        .env_remove("TAU_LOG_ROTATION")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "--log-non-blocking requires --log-file",
        ));
}

/// tau-rs/tau#699: `--log-non-blocking` must not decide whether the
/// on-disk run log is written.
///
/// `install` takes an early return into `install_non_blocking_inner`
/// whenever `--log-non-blocking --log-file <f>` is in play, and that
/// inner path used to build `registry()` + the fmt layer and nothing
/// else — `extra_layers` was dropped on the floor. The
/// `WorkflowRunLogLayer` `tau workflow run` attaches therefore never ran,
/// so `<scope>/.tau/workflow-runs/*.jsonl` was never written and
/// `tau workflow resume` replayed nothing and re-ran completed steps.
/// That is the damage of #650 reached by a second route, and it was
/// silent: flag accepted, exit 0, artifact absent.
///
/// The assertion is on the parsed JSONL, deliberately: the bug class
/// here is a green run that wrote nothing, so an exit-code check proves
/// nothing at all.
///
/// This asserts nothing about the `--log-file` fmt output. `tau-cli`'s
/// `tracing::install_with_extra_layers` does `std::mem::forget` on the
/// `InstallGuard`, so the appender's `WorkerGuard` never drops and
/// trailing fmt lines can be lost at exit. The run log does not go
/// through the non-blocking appender at all — `WorkflowRunLogLayer`
/// writes with sync `std::fs` + `sync_data` per line (#650/#693) — so
/// worker-thread flush timing cannot affect this assertion.
#[test]
fn non_blocking_log_file_still_writes_the_workflow_run_log() {
    let dir = common::setup_echo_project("echo", "canned_text = \"echoed: nb\"\n", &[]);
    let root = dir.path();

    let wf_dir = root.join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    std::fs::write(
        wf_dir.join("nb-pipeline.toml"),
        r#"[workflow]
description = "single-step pipeline run with a non-blocking file sink"

[[steps]]
id = "first"
kind = "agent.run"
agent = "echo"
input = "${input}"
"#,
    )
    .unwrap();

    let fmt_log = root.join("tau.log");
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args([
            "--log-non-blocking",
            "--log-file",
            fmt_log.to_str().unwrap(),
            "workflow",
            "run",
            "nb-pipeline",
            "--input",
            "nb",
        ])
        .current_dir(root)
        .env("TAU_HOME", root.join("global"))
        .env_remove("RUST_LOG")
        .env_remove("TAU_LOG_FILE")
        .env_remove("TAU_LOG_NON_BLOCKING")
        .env_remove("TAU_LOG_ROTATION")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "expected exit 0; stdout={stdout}\nstderr={stderr}"
    );

    // With `--log-file` set the fmt layer writes to the file, but
    // `run_id: <ulid>` is a bare `eprintln!` in `cmd/workflow/run.rs`,
    // so it still reaches stderr and still names the run log to read.
    let run_id = stderr
        .lines()
        .find_map(|l| l.strip_prefix("run_id: "))
        .map(str::trim)
        .unwrap_or_else(|| panic!("expected a `run_id: ` line on stderr; stderr={stderr}"));

    let log_path = root
        .join(".tau")
        .join("workflow-runs")
        .join(format!("nb-pipeline-{run_id}.jsonl"));
    let log = std::fs::read_to_string(&log_path).unwrap_or_else(|e| {
        panic!("run log {log_path:?} must exist after a --log-non-blocking run: {e}")
    });
    let record: serde_json::Value = serde_json::from_str(log.lines().next().unwrap_or_else(|| {
        panic!("run log {log_path:?} must carry at least one JSONL line; got {log:?}")
    }))
    .expect("run log line must be valid JSON");
    assert_eq!(record["step_id"], "first", "record={record}");
    assert_eq!(record["kind"], "agent.run", "record={record}");
    assert_eq!(record["status"], "ok", "record={record}");
}

/// tau-rs/tau#699, second sink: `--record-protocol` writes through the
/// same `extra_layers` slot the non-blocking path used to discard, so a
/// `--log-non-blocking` run produced an empty (or absent) recording while
/// still exiting 0.
///
/// `PluginRecordingLayer` writes from a `tokio::spawn` with an explicit
/// `flush()` on exit rather than through the non-blocking appender, so
/// worker-thread flush timing does not affect this assertion either.
#[test]
fn non_blocking_log_file_still_writes_the_protocol_recording() {
    let dir = common::setup_echo_project("echo", "canned_text = \"nb protocol\"\n", &[]);
    let root = dir.path();
    let wire_log = root.join("wire.log");
    let fmt_log = root.join("tau.log");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "--log-non-blocking",
            "--log-file",
            fmt_log.to_str().unwrap(),
            "--record-protocol",
            wire_log.to_str().unwrap(),
            "run",
            "--allow-ungoverned",
            "echo",
            "ping",
        ])
        .current_dir(root)
        .env("TAU_HOME", root.join("global"))
        .env_remove("RUST_LOG")
        .env_remove("TAU_LOG_FILE")
        .env_remove("TAU_LOG_NON_BLOCKING")
        .env_remove("TAU_LOG_ROTATION")
        .assert()
        .success();

    let recorded = std::fs::read_to_string(&wire_log).unwrap_or_else(|e| {
        panic!("protocol recording {wire_log:?} must exist after a --log-non-blocking run: {e}")
    });
    assert!(
        !recorded.trim().is_empty(),
        "protocol recording must not be empty under --log-non-blocking; contents:\n{recorded}"
    );
}
