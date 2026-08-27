//! CLI integration tests for `tau workflow ...`.

mod common;

use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn workflow_list_prints_each_toml_basename() {
    let dir = TempDir::new().unwrap();
    let wf_dir = dir.path().join("workflows");
    fs::create_dir_all(&wf_dir).unwrap();
    fs::write(wf_dir.join("alpha.toml"), b"[workflow]\n").unwrap();
    fs::write(wf_dir.join("beta.toml"), b"[workflow]\n").unwrap();

    let assert = Command::cargo_bin("tau")
        .unwrap()
        .arg("workflow")
        .arg("list")
        .current_dir(dir.path())
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("alpha"), "missing alpha; got {out}");
    assert!(out.contains("beta"), "missing beta; got {out}");
}

#[test]
fn workflow_list_handles_no_workflows_dir() {
    let dir = TempDir::new().unwrap();
    Command::cargo_bin("tau")
        .unwrap()
        .arg("workflow")
        .arg("list")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("No workflows/ directory"));
}

/// Strip CSI escape sequences (`ESC [ … final-byte`) so assertions can match
/// the logical text of a `tracing` fmt line regardless of ANSI styling.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // Skip `[` … up to and including the first byte in 0x40..=0x7e.
        if chars.next() != Some('[') {
            continue;
        }
        for c in chars.by_ref() {
            if ('\u{40}'..='\u{7e}').contains(&c) {
                break;
            }
        }
    }
    out
}

/// `tau workflow run` drives a one-step workflow through a real `echo-llm`
/// subprocess and emits the step record the run log is built from.
///
/// Covers the CLI wiring that `tau-workflow`'s library-level
/// `linear_workflow_runs_two_agent_steps_and_persists_jsonl` cannot: arg
/// parsing, `build_agents_map` against a real `tau.toml`, exit code 0, the
/// reply on stdout, the `run_id: ` stderr line, and the
/// `tau::workflow::step` event carrying the step's fields.
///
/// Un-`#[ignore]`d 2026-08-23: this was a `todo!()` stub waiting on the
/// `common::setup_echo_project` helper, which has long since stabilised.
/// Writing it immediately surfaced tau-rs/tau#650 — the on-disk JSONL run
/// log is missing or zero bytes on every run, because
/// `WorkflowRunLogLayer::on_event` writes from a detached `tokio::spawn`
/// that is dropped at runtime shutdown. The event itself is emitted
/// correctly, so that is what this test asserts; the on-disk assertions are
/// held back for #650's fix (see the commented block at the end).
#[test]
fn workflow_run_emits_step_record_and_succeeds() {
    let dir = common::setup_echo_project("echo", "canned_text = \"echoed: hello\"\n", &[]);
    let root = dir.path();

    let wf_dir = root.join("workflows");
    fs::create_dir_all(&wf_dir).unwrap();
    fs::write(
        wf_dir.join("echo-pipeline.toml"),
        r#"[workflow]
description = "single-step echo pipeline"

[[steps]]
id = "first"
kind = "agent.run"
agent = "echo"
input = "${input}"
"#,
    )
    .unwrap();

    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["workflow", "run", "echo-pipeline", "--input", "hello"])
        .current_dir(root)
        .env("TAU_HOME", root.join("global"))
        // The step assertions below read the default `tau=info` stderr
        // output; an inherited RUST_LOG would change the filter.
        .env_remove("RUST_LOG")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "expected exit 0; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("echoed: hello"),
        "expected the echo-llm reply on stdout; stdout={stdout}"
    );

    // `run_id: <ulid>` goes to stderr (see cmd/workflow/run.rs).
    let run_id = stderr
        .lines()
        .find_map(|l| l.strip_prefix("run_id: "))
        .map(str::trim)
        .unwrap_or_else(|| panic!("expected a `run_id: ` line on stderr; stderr={stderr}"));
    assert_eq!(run_id.len(), 26, "run_id must be a ULID; got {run_id:?}");

    // The fmt layer styles field names with ANSI escapes even when stderr is
    // a pipe, so `step_id=first` is not a literal substring of the raw bytes.
    let stderr_plain = strip_ansi(&stderr);

    // The step record reaches the tracing pipeline: `RunLog::append` emits it
    // on `target = tau::workflow::step`, which is what the run-log layer
    // consumes. Asserted on stderr (the default `tau=info` fmt layer) rather
    // than on the JSONL file — see the TODO below.
    let step_line = stderr_plain
        .lines()
        .find(|l| l.contains("tau::workflow::step"))
        .unwrap_or_else(|| panic!("expected a tau::workflow::step event; stderr={stderr}"));
    for field in [
        "step_id=first",
        "kind=agent.run",
        "status=ok",
        "output=echoed: hello",
        &format!("run_id={run_id}"),
    ] {
        assert!(
            step_line.contains(field),
            "step event missing {field:?}; line={step_line}"
        );
    }

    // TODO(tau-rs/tau#650): assert the on-disk run log once the layer flushes
    // before process exit. Today `<scope>/.tau/workflow-runs/echo-pipeline-
    // <run-id>.jsonl` is written from a detached `tokio::spawn` that is
    // dropped at runtime shutdown, so it is missing or zero bytes.
    //
    // let log_path = root
    //     .join(".tau")
    //     .join("workflow-runs")
    //     .join(format!("echo-pipeline-{run_id}.jsonl"));
    // let log = fs::read_to_string(&log_path).unwrap();
    // let record: serde_json::Value =
    //     serde_json::from_str(log.lines().next().expect("at least one JSONL line")).unwrap();
    // assert_eq!(record["step_id"], "first");
    // assert_eq!(record["kind"], "agent.run");
    // assert_eq!(record["status"], "ok");
}

#[test]
fn workflow_log_pretty_prints_records() {
    let dir = TempDir::new().unwrap();
    let scope_dir = dir.path().join(".tau").join("workflow-runs");
    fs::create_dir_all(&scope_dir).unwrap();

    // Write a single JSONL line representing one completed step.
    let line = serde_json::json!({
        "ts": "2026-05-12T14:23:01.123Z",
        "run_id": "01HKZTEST",
        "step_id": "first",
        "step_index": 0,
        "kind": "agent.run",
        "input": "hello",
        "output": "world",
        "started_at": "2026-05-12T14:22:55.001Z",
        "ended_at":   "2026-05-12T14:23:01.123Z",
        "duration_ms": 6122,
        "status": "ok"
    });
    fs::write(scope_dir.join("echo-01HKZTEST.jsonl"), format!("{line}\n")).unwrap();

    let assert = Command::cargo_bin("tau")
        .unwrap()
        .args(["workflow", "log", "01HKZTEST"])
        .current_dir(dir.path())
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("01HKZTEST"), "missing run id; got {out}");
    assert!(out.contains("first"), "missing step id; got {out}");
    assert!(out.contains("hello"), "missing input; got {out}");
    assert!(out.contains("world"), "missing output; got {out}");
}
