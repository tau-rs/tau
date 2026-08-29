//! Issue #631 / spec §13.6 — definition of done.
//!
//! A governed MCP project whose entry is host-clamped by `[allow.mcp]`
//! renders an amber `clamp:<to>` row: `tau run --bundle` writes
//! `.tau/runs/<id>.jsonl`, and reading it back through the real
//! `tau_trace::parse_line` yields a `ToolCall` carrying
//! `CapabilityVerdict::Clamp`.
//!
//! # Drive path
//!
//! Single entry agent via `run_ir` (no `[[pipeline.steps]]`).
//!
//! **Finding recorded during this task:** a `[[pipeline.steps]]` `run =
//! "tool:<id>"` step can NEVER produce this row. `pipeline.rs`'s
//! `StepRun::Tool` arm calls `dispatcher.invoke(tool_id, &args)`
//! directly — grep the whole file for `trace`/`Trace` and there are
//! zero hits. `TraceEventKind::ToolCall` is emitted only by the
//! agent-turn kernel loop in `tau-runtime-core/src/stream.rs`, reached
//! via `run_agent`/`prepare_agent_run` — either the single-entry-agent
//! `run_ir` path, or a pipeline's `StepRun::Agent` arm. So the e2e MUST
//! drive a real agent whose (scripted) LLM decides to call the tool.
//! That's why this task also extended `echo-llm` with a scripted
//! `tool_calls` config field — no existing subprocess-spawnable LLM
//! backend plugin could emit a `ToolUse` before this change.

mod common;

use assert_cmd::Command as AssertCmd;

/// Build an isolated `TAU_HOME` under `scratch`, pre-creating
/// `config.toml` to defeat the parallel-write race on first use (same
/// rationale as `cmd_build_mcp.rs::make_tau_home`).
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("global");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// Write an empty v7 `tau.lock` (no packages) so `tau build` doesn't
/// exit 3 on the missing-lockfile gate. `tau build` only reads/writes
/// this file for the `[[mcp]]` round-trip after a successful build; it
/// never consults `[[package]]` entries (those live in `tau-lock.toml`,
/// written separately by `common::setup_echo_project` for the
/// plugin-resolution path `tau run` uses).
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

/// The governed project's `tau.toml`: `[tools.weather]` declares a
/// `net.http` envelope over two hosts; `[allow.mcp.weather]` permits
/// only one — the meet narrows, producing a clamp. `[allow.tools.weather]`
/// registers the MCP binding (closed-world) with a ceiling wide enough
/// to cover the tool's own declared envelope (a build-time-only
/// requirement — the RUNTIME narrowing against `[allow.mcp]` is a
/// separate, independent lane per `mcp_capability_plan`'s doc comment).
/// `weather-agent`'s echo-llm `config.tool_calls` scripts turn 0 as a
/// `ToolUse` for `weather.get_forecast`; turn 1 falls back to
/// `canned_text` (`EndTurn`), ending the run.
const TAU_TOML: &str = r#"
[project]
name = "clamp-e2e"
version = "0.0.1"

[allow]
"net.http" = { hosts = ["api.weather.com", "evil.example"] }

[allow.models.echo]
backend = "echo-llm"
model = "m-1"

[allow.mcp.weather]
url = "cassette:./fixtures/weather.jsonl"
hosts = ["api.weather.com"]

[allow.tools.weather]
mcp = "weather"
"net.http" = { hosts = ["api.weather.com", "evil.example"] }

[tools.weather]
mcp = "cassette:./fixtures/weather.jsonl"
capabilities = [{ kind = "net.http", hosts = ["api.weather.com", "evil.example"] }]

[agents.weather-agent]
display_name = "Weather Agent"
package = "echo-llm@^0.1"
model = "echo"
tool_refs = ["weather"]

[agents.weather-agent.config]
canned_text = "done"
tool_calls = [[{ name = "weather.get_forecast", args = { location = "Paris" } }]]
"#;

/// Cassette covering the real handshake (`initialize` + `tools/list`) —
/// lines copied verbatim from
/// `crates/tau-mcp-tokio/tests/fixtures/weather_minimal_cassette.jsonl`
/// — PLUS a `tools/call` round trip that fixture doesn't have. That
/// fixture was built for pin-only tests (contract discovery); a real
/// agent-driven run additionally calls the tool, so the cassette needs
/// a recorded response for it or `CassetteTransport` returns a hard
/// `NoMatch` error (which the kernel treats as a fatal tool error, not
/// a traced row — see `stream.rs`'s `Err(err) => { ... return; }` arm).
fn cassette_jsonl() -> String {
    [
        r#"{"version":1}"#,
        r#"{"dir":"in","kind":"request","id":0,"method":"initialize","payload":{"clientInfo":{"name":"tau","version":"0.0.0"},"protocolVersion":"2025-03-26"}}"#,
        r#"{"dir":"out","kind":"response","id":0,"payload":{"protocolVersion":"2025-03-26","serverInfo":{"name":"weather-minimal","version":"0.0.0"}}}"#,
        r#"{"dir":"in","kind":"request","id":1,"method":"tools/list","payload":{}}"#,
        r#"{"dir":"out","kind":"response","id":1,"payload":{"tools":[{"name":"get_forecast","description":"Get weather forecast","inputSchema":{"type":"object","properties":{"location":{"type":"string"}}}}]}}"#,
        r#"{"dir":"in","kind":"request","id":2,"method":"tools/call","payload":{"name":"get_forecast","arguments":{"location":"Paris"}}}"#,
        r#"{"dir":"out","kind":"response","id":2,"payload":{"content":[{"type":"text","text":"Sunny, 72F"}]}}"#,
    ]
    .join("\n")
}

// GATED, NOT ABANDONED. This is issue #631's definition-of-done test and it
// is correct as written, but it cannot pass today for two pre-existing
// production reasons that are upstream of #631 and outside its scope:
//
//  1. `st.caps` is always empty. `ServerContract::from_handshake` is called
//     with a hardcoded `|_| Vec::new()` capability extractor
//     (`tau-mcp-tokio/src/host_lifecycle/handshake.rs:99`), and
//     `drive_handshake` is the single path behind every `mcp_open` variant.
//     `setup_mcp_runtime` then meets `&st.caps` against the plan, so the
//     declared set is always empty and `tool_effective_capabilities` returns
//     `None` — no clamp can ever be computed from a real handshake. The IR's
//     `ToolImpl::Mcp` already carries an authoritative per-tool
//     `capability_subset`, which is the natural source instead.
//
//  2. `tau run --bundle` cannot run ANY MCP-tool project.
//     `verify_bundle_against_source` (`tau-cli/src/cmd/run.rs:1091`)
//     re-lowers with an empty MCP contract cache, so such a bundle always
//     fails `IrSourceDivergence` before reaching the interpreter. The
//     function's own doc comment documents this and calls the fix a tracked
//     follow-up.
//
// Everything between those two ends — the clamp producer, the dispatcher
// forwarding, the IR trace sink, the run-log writer and the reader — is
// implemented and covered by tests at every level below this one. Un-ignore
// this test once both findings are fixed; it should then pass unchanged.
#[ignore = "blocked by two pre-existing production bugs upstream of #631 — see the comment above"]
#[test]
fn governed_clamped_mcp_run_writes_a_clamp_row() {
    // `common::setup_echo_project` builds `.tau/config.toml`, the
    // echo-llm package manifest under `.tau/packages/echo-llm/0.1.0/`,
    // and `tau-lock.toml` (schema v4, `[package.plugin]` pointing at
    // the real built `echo-llm` binary) — the exact scaffolding `tau
    // run`'s plugin loader needs to spawn a real LLM backend process.
    // Its own `tau.toml` is a fixed ungoverned shape we don't want, so
    // we overwrite it below with our governed MCP fixture; the agent id
    // ("weather-agent") differs from its default, which is fine — the
    // lockfile package entry is keyed by package name ("echo-llm"), not
    // agent id.
    let tmp = common::setup_echo_project("weather-agent", "", &[]);

    // `setup_echo_project` writes the echo-llm package manifest with
    // `capabilities = []` (it needs none for its own tests). Our agent's
    // effective grant comes from THIS manifest (governance L2: a tool's
    // declared caps must be a subset of the agent's effective grant),
    // so it must actually grant the `net.http` envelope `[tools.weather]`
    // declares — overwrite it.
    std::fs::write(
        tmp.path().join(".tau/packages/echo-llm/0.1.0/tau.toml"),
        r#"name = "echo-llm"
version = "0.1.0"
description = "echo plugin fixture"
authors = ["tester <test@example.com>"]
source = "https://example.com/echo-llm.git"
kind = "llm-backend"
dependencies = []
capabilities = [{ kind = "net.http", hosts = ["api.weather.com", "evil.example"] }]
"#,
    )
    .expect("overwrite echo-llm package manifest");

    // Cassette-backed MCP server (no process spawn ⇒ no sandbox gate).
    let fixtures_dir = tmp.path().join("fixtures");
    std::fs::create_dir_all(&fixtures_dir).expect("create fixtures dir");
    std::fs::write(fixtures_dir.join("weather.jsonl"), cassette_jsonl())
        .expect("write cassette fixture");

    // Governed project tau.toml (overwrites setup_echo_project's).
    std::fs::write(tmp.path().join("tau.toml"), TAU_TOML).expect("write tau.toml");

    // `tau build` reads/writes `tau.lock` (separate from `tau-lock.toml`);
    // pre-seed it empty so the missing-lockfile gate doesn't fire.
    write_empty_v7_lock(tmp.path());

    let tau_home = make_tau_home(tmp.path());

    // 1. Pin the contract via the real `tau mcp pin` path (cassette
    //    transport handshake) so the contract hash `tau build --offline`
    //    reads is self-consistent with what `setup_mcp_runtime` will see
    //    when it re-opens the same cassette at run time.
    AssertCmd::cargo_bin("tau")
        .expect("bin")
        .args(["mcp", "pin", "weather"])
        .current_dir(tmp.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    // 2. Build the governed bundle (no --allow-ungoverned: `[allow]` IS
    //    the consent). `--offline` reads the pin just written instead of
    //    re-dialing the cassette.
    let build_output = AssertCmd::cargo_bin("tau")
        .expect("bin")
        .args(["build", "--offline"])
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

    // 3. Run the bundle. The entry agent id is a positional argument;
    //    the prompt text is irrelevant (echo-llm ignores the incoming
    //    request and replays its scripted turns regardless).
    let run_output = AssertCmd::cargo_bin("tau")
        .expect("bin")
        .args(["run", "--bundle", &bundle_path, "weather-agent", "go"])
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

    // 4. The DoD assertion: read the run log through the REAL reader.
    let runs_dir = tmp.path().join(".tau").join("runs");
    let log = std::fs::read_dir(&runs_dir)
        .expect("`.tau/runs` must exist after a bundle run")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .expect("a run log jsonl must have been written");

    let contents = std::fs::read_to_string(&log).expect("read run log");
    let events: Vec<tau_ports::TraceEvent> = contents
        .lines()
        .filter_map(|l| tau_trace::parse_line(l).expect("every written line must parse"))
        .collect();

    let clamp = events
        .iter()
        .find_map(|e| match &e.kind {
            tau_ports::TraceEventKind::ToolCall {
                tool_name,
                capability: Some(tau_ports::CapabilityVerdict::Clamp { to }),
                ..
            } => Some((tool_name.clone(), to.clone())),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("no clamp ToolCall row in the run log; events were: {events:#?}")
        });

    assert!(clamp.0.contains("get_forecast"), "got tool {:?}", clamp.0);
    assert_eq!(
        clamp.1, "api.weather.com",
        "the row must name the surviving host from the [allow.mcp] ceiling"
    );
}
