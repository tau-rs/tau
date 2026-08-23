//! End-to-end test for native-tool execution on the IR run path (#639).
//!
//! `[tools.<id>] native = "..."` lowers to `ToolImpl::Native`, and every
//! entry in `agent.tool_refs` becomes an LLM-visible `DispatcherTool`
//! (`tau-runtime-core/src/interpreter/agent_loop.rs`) whose Native arm
//! forwards to the host's `ToolDispatcher`. Native bodies live in the
//! statically-linked `tau-native-tools` crate, NOT the plugin registry, so
//! before #639 `ForwardingDispatcher` had no way to serve the call and the
//! run died with `no tool registered for IR ToolId "read_temp"`.
//!
//! The echo-llm fixture plugin's `canned_tool_call` config makes the model
//! emit exactly one tool-use block on its first turn, so this test drives
//! the real round trip — agent loop → dispatcher → `tau_native_tools::invoke`
//! → tool result → second turn — through the built bundle and the real
//! `tau` binary. A dispatcher failure surfaces as a `ToolError` that fails
//! the run, so a completed 2-turn outcome IS the witness that the native
//! body was served.
//!
//! Harness mirrors `cmd_run_bundle_entry.rs` (echo-llm scaffold, schema-v6
//! lockfile written to BOTH `tau.lock` and `tau-lock.toml`).

#![allow(clippy::needless_raw_string_hashes)]

mod common;

use assert_cmd::Command as AssertCmd;

/// Author a single-agent project whose agent references the native
/// `read_temp` tool and whose echo-llm backend calls it on turn 1.
fn setup_native_tool_project() -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for native-tool project");
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        r#"schema_version = 2
kind = "project"
created_at = "2026-05-01T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "none"
"#,
    )
    .unwrap();

    // The agent's effective capability grant flows from the package
    // manifest (resolve_agent_caps): read_temp declares fs.read, so the
    // manifest must cover it.
    let pkg_dir = root
        .join(".tau")
        .join("packages")
        .join("echo-llm")
        .join("0.1.0");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("tau.toml"),
        r#"name = "echo-llm"
version = "0.1.0"
description = "echo plugin fixture"
authors = ["tester <test@example.com>"]
source = "https://example.com/echo-llm.git"
kind = "llm-backend"
dependencies = []
capabilities = [{ kind = "fs.read", paths = ["/data/incidents/**"] }]
"#,
    )
    .unwrap();

    let now = "2026-04-28T00:00:00Z";
    let zero_sha = "0".repeat(40);
    let llm_path = echo_llm
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let lockfile = format!(
        r#"schema_version = 6
generated_by_tau_version = "0.0.0"
generated_at = "{now}"

[[package]]
name = "echo-llm"
active_version = "0.1.0"
source = "https://example.com/echo-llm.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "{zero_sha}"
sha256 = ""
installed_at = "{now}"

[package.plugin]
binary_path = "{llm_path}"
built_at = "{now}"

[package.plugin.manifest]
provides = "llm_backend"
kind = "rust-cargo"
bin = "echo-llm"
"#
    );
    std::fs::write(root.join("tau.lock"), &lockfile).unwrap();
    std::fs::write(root.join("tau-lock.toml"), &lockfile).unwrap();

    // One agent, one native tool, no pipeline (single-entry-agent
    // `run_ir` path). `canned_tool_call` makes turn 1 a tool call;
    // turn 2 returns `canned_text` and ends the run.
    let project_toml = r#"[project]
name = "native-tool-demo"
version = "0.1.0"

[models]
default = { backend = "echo-llm", model = "claude-haiku-4-5" }

[agents.sensor]
display_name = "Sensor"
package      = "echo-llm@^0.1"
model        = "default"
tool_refs    = ["read_temp"]

[agents.sensor.prompt]
system = "Read the sensor."

[agents.sensor.config]
canned_text = "TOOL-ROUNDTRIP-DONE"
canned_tool_call = { name = "read_temp", input = {} }

[tools.read_temp]
native      = "ReadTemp"
description = "Read the incident sensor temperature."
capabilities = [{ kind = "fs.read", paths = ["/data/incidents/sensors/**"] }]
"#;
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

fn build_bundle(project: &std::path::Path, tau_home: &std::path::Path) -> std::path::PathBuf {
    let out = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned"])
        .current_dir(project)
        .env("TAU_HOME", tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let path = String::from_utf8(out.stdout).unwrap().trim().to_string();
    std::path::PathBuf::from(path)
}

#[test]
fn bundle_run_executes_native_tool_call() {
    let dir = setup_native_tool_project();
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let bundle = build_bundle(dir.path(), &tau_home);

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--allow-ungoverned",
            "--bundle",
            bundle.to_str().unwrap(),
            "sensor",
            "what is the temperature?",
            "--json",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "native tool call must be served by the dispatcher; stderr={stderr}\nstdout={}",
        String::from_utf8_lossy(&output.stdout),
    );
    // The pre-#639 failure mode, pinned so a regression is unambiguous.
    assert!(
        !stderr.contains("no tool registered for IR ToolId"),
        "native tool must not fall through to the unknown-tool error; stderr={stderr}"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let outcome = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"));

    assert_eq!(outcome["outcome"], "completed");
    assert_eq!(
        outcome["final_message"], "TOOL-ROUNDTRIP-DONE",
        "second turn's text ends the run; outcome line: {outcome}"
    );
    // Two turns: the tool-use turn plus the post-result turn. One turn
    // would mean the tool call never happened (fixture drift).
    assert_eq!(
        outcome["total_turns"], 2,
        "expected a tool round trip (2 turns); outcome line: {outcome}"
    );
}
