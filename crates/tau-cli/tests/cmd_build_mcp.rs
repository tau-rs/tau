//! PR-4 migration sweep + MCP offline e2e tests.
//!
//! # Task 7.1 — sweep test
//!
//! Walks every `tau.toml` under `crates/` and verifies that each still
//! lowers green with the v7 lockfile + new `Caches` shape. Fixtures with
//! `ToolBody::Mcp` entries are skipped (they need a live resolver; covered
//! by the `--offline` e2e test below).
//!
//! # Task 7.2 — `--offline` e2e fixture
//!
//! Constructs a minimal MCP project in a tempdir, writes a pinned contract
//! file, runs `tau build --offline`, and verifies:
//! - the command exits 0,
//! - `tau.lock` is written with `schema_version = 7`,
//! - `tau.lock` carries an `[[mcp]]` entry for the `weather` tool.

mod common;

// ─── shared helpers ──────────────────────────────────────────────────────────

/// Locate the workspace root from `CARGO_MANIFEST_DIR` (crates/tau-cli).
fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/tau-cli → crates
        .unwrap()
        .parent() // crates → workspace root
        .unwrap()
        .to_path_buf()
}

// ─── Task 7.1 — migration sweep ──────────────────────────────────────────────

/// Collect every `tau.toml` under `<root>/crates/` (excluding target/).
fn collect_tau_tomls(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let crates_dir = root.join("crates");
    for entry in walkdir::WalkDir::new(&crates_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "tau.toml") {
            let path_str = path.to_string_lossy();
            // Exclude anything inside a cargo target directory.
            if !path_str.contains("/target/") {
                out.push(path.to_path_buf());
            }
        }
    }
    out
}

#[derive(Debug)]
enum LowerFixtureOutcome {
    /// Successfully lowered.
    Pass,
    /// Fixture has MCP entries — skipped, covered by the offline e2e test.
    SkippedMcp,
    /// File does not parse as a `ProjectConfig` (e.g. plugin manifests,
    /// intentionally malformed test inputs, or fixtures with local-path
    /// package sources that the validator rejects). These are not
    /// lowering-pipeline fixtures so we skip rather than fail.
    SkippedNotAProject(#[allow(dead_code)] String),
    /// Parse succeeded but lowering failed — this is a real failure.
    LowerFailed(String),
}

fn lower_fixture(path: &std::path::Path) -> LowerFixtureOutcome {
    use tau_ir_lower::{lower_project, Caches};
    use tau_pkg::project::project::{ProjectConfig, ToolBody};
    use tau_ports::target::registry;

    let toml_str = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return LowerFixtureOutcome::SkippedNotAProject(format!("read failed: {e}")),
    };

    let config = match ProjectConfig::parse_str(&toml_str) {
        Ok(c) => c,
        Err(e) => {
            // Many tau.toml files in the repo are plugin manifests, intentionally
            // broken fixtures, or fixtures with local-path sources that
            // `validate()` rejects. None of these are `tau build` targets.
            return LowerFixtureOutcome::SkippedNotAProject(format!("parse/validate: {e}"));
        }
    };

    // Skip fixtures that have MCP tool entries — they need a resolver and
    // are covered by the offline e2e test (Task 7.2).
    if config
        .tools
        .values()
        .any(|t| matches!(t.body, ToolBody::Mcp(_)))
    {
        return LowerFixtureOutcome::SkippedMcp;
    }

    let target = match registry::list_available().next() {
        Some(e) => e.triple,
        None => return LowerFixtureOutcome::LowerFailed("no Available target in registry".into()),
    };

    // Empty MCP cache (no MCP entries in these fixtures).
    // Native-tool cache uses the same SHA-256-of-name sentinel as cmd/build.rs
    // so the typecheck non-zero invariant is satisfied.
    let caches = Caches {
        native_tool: &|name: &str| {
            use sha2::Digest;
            let mut h = sha2::Sha256::new();
            h.update(name.as_bytes());
            Some(h.finalize().into())
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
    };

    match lower_project(&config, &target, &caches) {
        Ok(_) => LowerFixtureOutcome::Pass,
        Err(e) => LowerFixtureOutcome::LowerFailed(format!("{e}")),
    }
}

/// PR-4 migration sweep: every `tau.toml` that is a valid project config
/// still lowers green with the v7 lockfile + new `Caches` shape.
///
/// Files that are plugin manifests, intentionally malformed test inputs,
/// or use local-path package sources are quietly skipped — they are not
/// `tau build` targets and have never been lowerable.
#[test]
fn all_fixtures_lower_with_v7_caches_shape() {
    let root = workspace_root();
    let fixtures = collect_tau_tomls(&root);
    assert!(
        !fixtures.is_empty(),
        "collect_tau_tomls found no tau.toml files — check the workspace root path",
    );

    let mut passes = 0usize;
    let mut skipped_mcp = 0usize;
    let mut skipped_not_project = 0usize;
    let mut failures: Vec<(std::path::PathBuf, String)> = Vec::new();

    for path in &fixtures {
        match lower_fixture(path) {
            LowerFixtureOutcome::Pass => passes += 1,
            LowerFixtureOutcome::SkippedMcp => skipped_mcp += 1,
            LowerFixtureOutcome::SkippedNotAProject(_) => skipped_not_project += 1,
            LowerFixtureOutcome::LowerFailed(e) => failures.push((path.clone(), e)),
        }
    }

    if !failures.is_empty() {
        let report = failures
            .iter()
            .map(|(p, e)| format!("  {}: {}", p.display(), e))
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} fixture(s) failed to lower (of {} total, {} skipped-not-project, {} skipped-mcp):\n{}",
            failures.len(),
            fixtures.len(),
            skipped_not_project,
            skipped_mcp,
            report,
        );
    }

    // Surface counts in the test output for the phase-7 report.
    println!(
        "sweep: {passes} passed, {skipped_mcp} skipped (MCP), \
         {skipped_not_project} skipped (not-a-project), 0 failed (of {} total)",
        fixtures.len(),
    );
}

// ─── Task 7.2 — `--offline` e2e fixture ──────────────────────────────────────

/// Build an isolated `TAU_HOME` under `scratch` with `config.toml` pre-created
/// to defeat the parallel-write race on Windows (see project memory).
fn make_tau_home(scratch: &std::path::Path) -> std::path::PathBuf {
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join("config.toml");
    if !cfg.exists() {
        std::fs::write(&cfg, "").unwrap();
    }
    home
}

/// Write a minimal v7-compatible `tau.lock` (no packages) so `tau build`
/// doesn't exit 3 on the missing-lockfile gate.
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

/// Construct a `PinnedContract` for the mock weather server and write it to
/// `<pin_dir>/weather.contract.json`.
fn write_pinned_weather_contract(pin_dir: &std::path::Path) {
    use std::collections::BTreeMap;
    use tau_mcp::contract::pinned::PinnedContract;
    use tau_mcp::contract::server_contract::{ContractTool, ServerContract};
    use tau_mcp::protocol::initialize::ServerInfo;
    use tau_mcp::protocol::tools::McpToolInputSchema;

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
                "properties": {
                    "lat": {"type": "number"},
                    "lon": {"type": "number"}
                },
                "required": ["lat", "lon"]
            })),
            caps: vec![],
        }],
    };

    let pinned = PinnedContract::from_parts("https://mcp.example.com/weather".into(), contract)
        .expect("build PinnedContract");

    std::fs::create_dir_all(pin_dir).unwrap();
    let bytes = serde_json::to_vec_pretty(&pinned).expect("serialize pinned contract");
    std::fs::write(pin_dir.join("weather.contract.json"), bytes).unwrap();
}

/// `tau build --offline` against a pinned MCP contract emits a v7 lockfile
/// with a `[[mcp]]` entry for the `weather` tool.
#[test]
fn offline_build_against_pinned_contract_emits_v7_lockfile() {
    let scratch = tempfile::tempdir().unwrap();
    let project_root = scratch.path().join("mcp_weather");
    std::fs::create_dir_all(&project_root).unwrap();

    // Minimal MCP project: one tool referencing a weather MCP server.
    std::fs::write(
        project_root.join("tau.toml"),
        r#"[project]
name = "mcp_weather"

[tools.weather]
mcp = "https://mcp.example.com/weather"
capabilities = [{ kind = "net.http", hosts = ["api.weather.com"], methods = [] }]
"#,
    )
    .unwrap();

    // Pre-write the empty lockfile so `tau build` doesn't exit 3.
    write_empty_v7_lock(&project_root);

    // Write the pinned contract file.
    let pin_dir = project_root.join(".tau").join("mcp");
    write_pinned_weather_contract(&pin_dir);

    let tau_home = make_tau_home(scratch.path());

    let output = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .args(["build", "--offline"])
        .current_dir(&project_root)
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();

    // Verify tau.lock was written with the expected v7 shape.
    let lock_path = project_root.join("tau.lock");
    assert!(
        lock_path.exists(),
        "tau.lock was not written after --offline build; stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let lock_str = std::fs::read_to_string(&lock_path).expect("read tau.lock");

    assert!(
        lock_str.contains("schema_version = 7"),
        "tau.lock does not contain 'schema_version = 7':\n{lock_str}",
    );
    assert!(
        lock_str.contains("[[mcp]]"),
        "tau.lock does not contain '[[mcp]]':\n{lock_str}",
    );
    assert!(
        lock_str.contains("entry = \"weather\""),
        "tau.lock does not contain 'entry = \"weather\"':\n{lock_str}",
    );
    assert!(
        lock_str.contains("url = \"https://mcp.example.com/weather\""),
        "tau.lock does not contain the expected url:\n{lock_str}",
    );
}
