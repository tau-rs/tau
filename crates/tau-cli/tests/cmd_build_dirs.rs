//! End-to-end CLI coverage for `[dirs]` directory-based definitions
//! (ADR-0068) through the real `tau` binary.
//!
//! The unit/integration tests that shipped with the feature all stop at
//! `ProjectConfig::from_path`. That left the build pipeline itself
//! untested: `tau build`, `tau verify --bundle` and the MCP contract
//! resolver each loaded `tau.toml` through the *non-scanning*
//! `UncheckedProjectConfig` → `validate()` path, so a dir-defined agent
//! was silently dropped from the bundle and a dir-defined MCP tool never
//! had its contract resolved or pinned.
//!
//! These tests are the regression gate for that: they exercise the feature
//! from the outside, through the commands a user actually runs.
//!
//! Fixture conventions mirror `cmd_build.rs` / `cmd_verify_bundle.rs`: a
//! project tempdir plus a sibling `TAU_HOME` tempdir with `config.toml`
//! pre-created (see project memory `feedback_windows_tau_home_test_pattern`).

#![allow(clippy::needless_raw_string_hashes)]

use assert_cmd::Command;

/// Root manifest declaring both `[dirs]` roots plus one *inline* agent, so
/// each test proves the dir-defined and inline surfaces coexist in one
/// bundle rather than one replacing the other.
const ROOT_TOML: &str = r#"packages = ["anthropic"]

[project]
name = "dirsbuild"
version = "0.1.0"

[dirs]
agents = "agents"
tools  = "tools"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.solo]
display_name = "Solo"
package = "dirsbuild@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"
"#;

/// A dir-defined agent at `agents/strict.md` → engine name `strict`. It
/// references the dir-defined tool `util/echo`, so a build that scanned the
/// agents root but not the tools root would fail loudly on an unknown tool
/// ref rather than pass vacuously.
///
/// The name is deliberately flat: a *nested* agent name (`review/strict`)
/// cannot currently reach a bundle at all — see
/// `nested_agent_name_is_a_known_bundle_gap` below.
const AGENT_MD: &str = "---\n\
display_name: Strict Reviewer\n\
package: dirsbuild@^0.1\n\
model: default\n\
tool_refs: [\"util/echo\"]\n\
---\n\
You are a strict reviewer.\n";

/// A dir-defined native tool at `tools/util/echo.toml` → engine name
/// `util/echo`.
const TOOL_TOML: &str = "native = \"EchoTool\"\ndescription = \"echo back\"\n";

fn write(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// Empty schema-v6 lockfile: no packages, so the build's install-state
/// check is a no-op and the bundle writes cleanly.
fn write_empty_lockfile(root: &std::path::Path) {
    std::fs::write(
        root.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();
}

fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// Stand up a project with `[dirs]`, one dir-defined agent, one
/// dir-defined tool, one inline agent, and an empty lockfile.
fn write_dirs_project(project: &std::path::Path) {
    write(project, "tau.toml", ROOT_TOML);
    write(project, "agents/strict.md", AGENT_MD);
    write(project, "tools/util/echo.toml", TOOL_TOML);
    write_empty_lockfile(project);
}

/// `tau build` must ship the `[dirs]`-defined agent in the bundle, and
/// `tau verify --bundle` must then agree with what was built.
///
/// Pre-fix this failed on the first assertion: `tau_pkg::bundle::build`
/// loaded `tau.toml` without scanning, so `m.agents` held only `solo`.
#[test]
fn build_ships_dir_defined_agent_and_verify_agrees() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_dirs_project(&project);
    let tau_home = make_tau_home(scratch.path());
    let out = project.join("all.tau");

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned", "-o", out.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    let body = std::fs::read_to_string(&out).unwrap();
    let m = tau_pkg::bundle::manifest::BundleManifest::parse_str(&body).unwrap();
    let ids: Vec<&str> = m.agents.iter().map(|a| a.id.as_str()).collect();
    assert!(
        ids.contains(&"strict"),
        "bundle must carry the [dirs]-defined agent; got {ids:?}",
    );
    assert!(
        ids.contains(&"solo"),
        "bundle must still carry the inline agent; got {ids:?}",
    );

    // The IR payload must exist: lowering resolves `tool_refs =
    // ["util/echo"]`, so a payload proves the dir-defined *tool* reached
    // the lowering config too.
    assert!(
        m.ir_payload.is_some(),
        "bundle must embed an IR payload (dir-defined tool ref must resolve)",
    );

    // Build and the re-lowering verify path must agree about the agent set
    // and the source IR hash. If only one of them scanned `[dirs]`, this
    // is where it shows up (AgentSetMismatch / IrSourceDivergence).
    Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--bundle", out.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();
}

/// `tau build --agent strict` must accept a `[dirs]`-defined agent id.
/// Pre-fix the agent filter was matched against a config that never saw the
/// directory, so this exited 2 with `UnknownAgent`.
#[test]
fn build_agent_filter_accepts_dir_defined_agent() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_dirs_project(&project);
    let tau_home = make_tau_home(scratch.path());
    let out = project.join("strict.tau");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "build",
            "--allow-ungoverned",
            "--agent",
            "strict",
            "-o",
            out.to_str().unwrap(),
        ])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    let body = std::fs::read_to_string(&out).unwrap();
    let m = tau_pkg::bundle::manifest::BundleManifest::parse_str(&body).unwrap();
    assert_eq!(m.agents.len(), 1);
    assert_eq!(m.agents[0].id.as_str(), "strict");
    assert_eq!(m.bundle.selected_agents, Some(vec!["strict".to_string()]));
}

/// KNOWN GAP (pinned, not endorsed): an agent whose name contains `/` —
/// which is exactly what a nested `agents/review/strict.md` produces, and
/// the headline example in the `[dirs]` how-to — cannot be represented in a
/// bundle. `BundleAgent.id` is a `tau_domain::AgentId`, whose grammar is
/// `[a-z0-9-]` (no `/`, and no `_` either, though the dirs scanner accepts
/// both). `tau build` therefore exits 2 during manifest assembly.
///
/// This is pre-existing — an inline `[agents."review/strict"]` hits the same
/// wall, and `tau resolve` is worse: it *panics* at
/// `cmd/resolve_helpers.rs:68` (`expect("AgentId from validated entry")`).
/// It was invisible until the build pipeline became `[dirs]`-aware, because
/// nothing ever fed a `/`-named agent to it.
///
/// This test pins the current boundary so it can't shift silently. When the
/// `AgentId` grammar is widened (a `tau-domain` identity decision, ADR
/// territory — out of scope for the wiring fix that exposed it), delete this
/// test and extend `build_ships_dir_defined_agent_and_verify_agrees` to a
/// nested name instead.
#[test]
fn nested_agent_name_is_a_known_bundle_gap() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write(&project, "tau.toml", ROOT_TOML);
    write(&project, "agents/review/strict.md", AGENT_MD);
    write(&project, "tools/util/echo.toml", TOOL_TOML);
    write_empty_lockfile(&project);
    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicates::str::contains("review/strict"))
        .stderr(predicates::str::contains("invalid id"));
}

/// A `[dirs]`-defined MCP tool must have its contract resolved and pinned
/// exactly like an inline `[tools.X]` one.
///
/// Pre-fix `resolve_mcp_cache` loaded `tau.toml` without scanning, so the
/// entry list was empty: nothing read the pin, the lockfile carried no
/// `[[mcp]]` row, and (once lowering became dirs-aware) the build failed
/// outright on an unresolved MCP contract. The nested pin path
/// `.tau/mcp/util/weather.contract.json` is also the only end-to-end
/// exercise of the path-named-entry nesting.
#[test]
fn offline_build_resolves_dir_defined_mcp_tool() {
    use std::collections::BTreeMap;
    use tau_mcp::contract::pinned::PinnedContract;
    use tau_mcp::contract::server_contract::{ContractTool, ServerContract};
    use tau_mcp::protocol::initialize::ServerInfo;
    use tau_mcp::protocol::tools::McpToolInputSchema;

    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("mcpdirs");
    std::fs::create_dir(&project).unwrap();

    write(
        &project,
        "tau.toml",
        r#"[project]
name = "mcpdirs"
version = "0.1.0"

[dirs]
tools = "tools"
"#,
    );
    write(
        &project,
        "tools/util/weather.toml",
        r#"mcp = "https://mcp.example.com/weather"
capabilities = [{ kind = "net.http", hosts = ["api.weather.com"] }]
"#,
    );
    std::fs::write(
        project.join("tau.lock"),
        r#"schema_version = 7
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();

    // Pinned contract at the *nested* path the `util/weather` entry name
    // implies.
    let contract = ServerContract {
        protocol_version: "2025-03-26".into(),
        server_info: ServerInfo {
            name: "mock-weather".into(),
            version: "0.0.0".into(),
            additional: BTreeMap::new(),
        },
        tools: vec![ContractTool {
            name: "get_forecast".into(),
            description: Some("Get weather forecast".into()),
            input_schema: McpToolInputSchema(serde_json::json!({
                "type": "object",
                "properties": { "lat": {"type": "number"} },
                "required": ["lat"]
            })),
            caps: vec![],
        }],
    };
    let pinned = PinnedContract::from_parts("https://mcp.example.com/weather".into(), contract)
        .expect("build PinnedContract");
    let pin_dir = project.join(".tau").join("mcp").join("util");
    std::fs::create_dir_all(&pin_dir).unwrap();
    std::fs::write(
        pin_dir.join("weather.contract.json"),
        serde_json::to_vec_pretty(&pinned).expect("serialize pinned contract"),
    )
    .unwrap();

    let tau_home = make_tau_home(scratch.path());

    Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned", "--offline"])
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();

    let lock = std::fs::read_to_string(project.join("tau.lock")).unwrap();
    assert!(
        lock.contains("util/weather"),
        "tau.lock must carry an [[mcp]] entry for the dir-defined tool; got:\n{lock}",
    );
}
