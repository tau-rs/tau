//! Issue #631 / spec §13.5 — executing coverage of the run-log write.
//!
//! Before this test, nothing in the workspace exercised writer → envelope
//! → reader for a real `tau run --bundle` invocation: the only test that
//! asserts `.tau/runs/<id>.jsonl` end-to-end is
//! `clamp_row_e2e.rs::governed_clamped_mcp_run_writes_a_clamp_row`, which is
//! `#[ignore]`d (gated on #712/#714, both about MCP handshakes). That left
//! §13.5's writer/reader half unproven by any test that actually runs.
//!
//! This test drives the SAME `tau build` → `tau run --bundle` path with a
//! plain agent-only (no MCP, no tool_refs) bundle, so neither #712 nor #714
//! applies — it asserts the run log is written, non-empty, and that every
//! line round-trips through the real `tau_trace::parse_line`, including at
//! least one `Turn` event (the one kind an agent-only run is guaranteed to
//! produce). It does NOT assert a clamp row — that remains the gated test's
//! job.

mod common;

use assert_cmd::Command as AssertCmd;

/// Build an isolated `TAU_HOME` under `scratch`, pre-creating
/// `config.toml` to defeat the parallel-write race on first use. Mirrors
/// `clamp_row_e2e.rs::make_tau_home`.
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("global");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// Write an empty v7 `tau.lock` (no packages) so `tau build` doesn't exit 3
/// on the missing-lockfile gate. Mirrors `clamp_row_e2e.rs::write_empty_v7_lock`.
fn write_empty_v7_lock(project_root: &std::path::Path) {
    std::fs::write(
        project_root.join("tau.lock"),
        r#"schema_version = 7
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();
}

#[test]
fn bundle_run_writes_a_readable_run_log_with_a_turn_event() {
    // `common::setup_echo_project` builds an ungoverned, MCP-free,
    // single-agent project (`.tau/config.toml`, the echo-llm package
    // manifest, and `tau-lock.toml`) — no tool_refs, so the agent never
    // calls a tool; it just answers with `canned_text` and ends the turn.
    let tmp = common::setup_echo_project("echo-agent", "canned_text = \"hello\"", &[]);

    // `tau build` reads/writes `tau.lock` (separate from `tau-lock.toml`
    // written by `setup_echo_project`); pre-seed it empty so the
    // missing-lockfile gate doesn't fire.
    write_empty_v7_lock(tmp.path());

    let tau_home = make_tau_home(tmp.path());

    // 1. Build the bundle. No `[allow]` in this project's tau.toml, so the
    //    build must be explicitly marked ungoverned.
    let build_output = AssertCmd::cargo_bin("tau")
        .expect("bin")
        .args(["build", "--allow-ungoverned"])
        .current_dir(tmp.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let bundle_path = String::from_utf8(build_output.stdout)
        .expect("utf8 stdout")
        .trim()
        .to_string();
    assert!(
        !bundle_path.is_empty(),
        "tau build must print the bundle path to stdout"
    );

    // 2. Run the bundle (no --tui: this test only cares about the
    //    persisted JSONL, not the live channel).
    let run_output = AssertCmd::cargo_bin("tau")
        .expect("bin")
        .args([
            "run",
            "--allow-ungoverned",
            "--bundle",
            &bundle_path,
            "echo-agent",
            "hello there",
        ])
        .current_dir(tmp.path())
        .env("TAU_HOME", &tau_home)
        .output()
        .expect("spawn tau run");
    assert!(
        run_output.status.success(),
        "tau run --bundle must succeed; stdout={}\nstderr={}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr),
    );

    // 3. The DoD assertion: a run log exists, is non-empty, and every line
    //    round-trips through the REAL reader.
    let runs_dir = tmp.path().join(".tau").join("runs");
    let log = std::fs::read_dir(&runs_dir)
        .expect("`.tau/runs` must exist after a bundle run")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .expect("a run log jsonl must have been written");

    let contents = std::fs::read_to_string(&log).expect("read run log");
    assert!(!contents.trim().is_empty(), "the run log must be non-empty");

    let events: Vec<tau_ports::TraceEvent> = contents
        .lines()
        .filter_map(|l| tau_trace::parse_line(l).expect("every written line must parse"))
        .collect();
    assert!(
        !events.is_empty(),
        "every parsed line must be a TraceEvent (or the file had zero lines)"
    );

    let has_turn_event = events
        .iter()
        .any(|e| matches!(e.kind, tau_ports::TraceEventKind::Turn { .. }));
    assert!(
        has_turn_event,
        "an agent-only run must emit at least one Turn trace event; events were: {events:#?}"
    );
}
