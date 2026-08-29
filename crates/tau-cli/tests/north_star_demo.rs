//! Issue #461: the cross-epic north-star demo fixture, end to end.
//!
//! ONE `[allow]`-governed workflow using Branch + Loop
//! (`tests/fixtures/north-star/tau.toml`), proven across every path the
//! engine ships today:
//!
//! - governance: the governed fixture passes `tau check governance`; the
//!   over-reach twin (`tests/fixtures/north-star-over-reach/tau.toml`, ONLY
//!   delta: `read_temp` claims `fs.read /etc/**` outside the ceiling) fails
//!   it with exit 2,
//! - artifact: `tau build` (NO `--allow-ungoverned` — the `[allow]` section
//!   IS the consent) produces a bundle carrying the Governed verdict,
//! - execution: the pipeline (triage → Branch(route) → Loop(review) →
//!   report) runs end to end via `tau run` and via `tau build` +
//!   `tau run --bundle`, governed (no flags — #620 fixed the run-path
//!   alias resolution via `ProjectConfig::effective_models()`) AND on an
//!   ungoverned variant (`ungoverned_variant()` — the SAME pipeline with
//!   `[allow.models.default]` rewritten to `[models]`, driven with
//!   `--allow-ungoverned` to keep that escape hatch covered),
//! - wasm: `tau build --target wasm-guest` BUILDS this fixture — ADR-0068
//!   (#621) flipped `any-wasi-strict` to execute Branch/Parallel/Loop
//!   in-guest via `run_pipeline`. The build-refusal witness moved to the
//!   Suspend twin (`tests/fixtures/north-star-suspend/tau.toml`, ONLY
//!   delta: one `suspend:human-signoff` step before `report`), refused at
//!   feature-fit because the guest has no `SuspensionStore` channel.
//!
//! ## Control-flow proof without inspecting internals
//!
//! echo-llm replays one canned text for every agent step, and template
//! resolution HARD-ERRORS on unresolved step refs. The final `report` step
//! reads BOTH `${steps.escalate.output}` (only present if the branch's
//! then-arm ran) and `${steps.draft.output}` (only present if the loop body
//! ran), so a completed run proves the Branch arm and the Loop body both
//! executed. The loop's `until` matches "APPROVED" in the canned text —
//! exhaustion (max_iters without the predicate holding) would hard-error.
//!
//! Since #623 the fixture ALSO witnesses entry-agent resolution: only the
//! entry agent (`triage`) carries the marker canned text; every other
//! agent's `[agents.<id>.config]` is a marker-free decoy. The backend is
//! configured from the entry agent, so a bundle run that regressed to the
//! alphabetically-first agent (`oncall`) would replay the decoy, take the
//! branch's otherwise-arm, exhaust the loop, and fail.
//!
//! Harness mirrors `cmd_run_bundle_pipeline.rs`: echo-llm scaffold
//! (.tau/config.toml, package manifest, schema-v6 lockfile written to BOTH
//! `tau.lock` and `tau-lock.toml`) around the on-disk fixture tau.toml.

mod common;

use assert_cmd::Command as AssertCmd;
use predicates::prelude::*;

/// The entry agent's canned text — every step's output, and therefore the
/// pipeline's final rendered message. Carries BOTH control-flow markers:
/// "URGENT" (branch then-arm) and "APPROVED" (loop until-predicate).
const SENTINEL: &str = "URGENT: coolant temperature rising - fan engaged. APPROVED";

fn fixture_toml(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .join("tau.toml");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Derive the ungoverned-but-otherwise-identical variant of the governed
/// fixture: strip the `[allow]` ceiling + `[allow.tools]` registration and
/// move the model alias back to top-level `[models]`. Keeps the
/// `--allow-ungoverned` + `[models]` path witnessed on the SAME pipeline
/// alongside the governed tests.
fn ungoverned_variant(toml: &str) -> String {
    let ceiling = "[allow]\n\"fs.read\" = { paths = [\"/data/incidents/**\"] }\n";
    let models = "[allow.models.default]\nbackend = \"echo-llm\"\nmodel = \"claude-haiku-4-5\"\n";
    let tool_allow =
        "[allow.tools.read_temp]\nnative = \"ReadTemp\"\n\"fs.read\" = { paths = [\"/data/incidents/**\"] }\n";
    for needle in [ceiling, models, tool_allow] {
        assert!(
            toml.contains(needle),
            "fixture drifted — expected block missing:\n{needle}"
        );
    }
    let out = toml
        .replace(ceiling, "")
        .replace(
            models,
            "[models]\ndefault = { backend = \"echo-llm\", model = \"claude-haiku-4-5\" }\n",
        )
        .replace(tool_allow, "");
    assert!(
        !out.lines().any(|l| l.trim_start().starts_with("[allow")),
        "allow tables must be fully stripped; got:\n{out}"
    );
    out
}

/// Echo scaffold (config.toml, echo-llm package manifest, v6 lockfile to
/// BOTH tau.lock and tau-lock.toml) + the given project tau.toml contents.
fn setup_project(project_toml: &str) -> tempfile::TempDir {
    let (echo_llm, _echo_tool) = common::echo_plugins::ensure_echo_plugins_built();
    let dir = tempfile::tempdir().expect("tempdir for north-star project");
    let root = dir.path();

    std::fs::create_dir_all(root.join(".tau")).unwrap();
    // Scope config.toml with required_tier = "none" (no strict/light
    // isolation needed for the toy plugin).
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

    // Per-package manifest for echo-llm. `tau build` enumerates each
    // lockfile package and requires its on-disk tree at
    // `.tau/packages/<name>/<version>/`; the run path reads this manifest
    // during package resolution.
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
# The agents' effective capability grant flows from this manifest
# (resolve_agent_caps): triage's read_temp tool needs fs.read inside it.
capabilities = [{ kind = "fs.read", paths = ["/data/incidents/**"] }]
"#,
    )
    .unwrap();

    // Schema-v6 lockfile recording the echo-llm plugin binary path. The
    // SAME content is written to both filenames: `tau build` reads
    // `tau.lock`; `scope.lockfile_path()` (plugin loader, package
    // resolution) reads `tau-lock.toml`.
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

    std::fs::write(root.join("tau.toml"), project_toml).unwrap();

    dir
}

/// Extract the pipeline-outcome JSON line (`{"outcome": ..., "final_message":
/// ...}`) from a `--json` run's stdout.
fn outcome_line(stdout: &str) -> serde_json::Value {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("outcome").is_some())
        .unwrap_or_else(|| panic!("--json should include an outcome JSON line; stdout: {stdout}"))
}

/// Assert the `--json` outcome line has the pipeline renderer's completed
/// shape: `outcome` + `final_message` (the last leaf step's output) and NO
/// `total_turns` (that field is the single-agent renderer's signature).
fn assert_completed_pipeline_outcome(stdout: &str) {
    let outcome = outcome_line(stdout);
    assert_eq!(outcome["outcome"], "completed");
    assert_eq!(
        outcome["final_message"], SENTINEL,
        "final_message should be the LAST leaf step's output; outcome line: {outcome}"
    );
    assert!(
        outcome.get("total_turns").is_none(),
        "pipeline outcome must NOT carry total_turns (single-agent renderer \
         shape); outcome line: {outcome}"
    );
}

/// The constitution holds: the governed fixture is clean under
/// `tau check governance` (exit 0 — no Error/NeedsSetup findings).
#[test]
fn north_star_check_governance_is_clean() {
    let dir = setup_project(&fixture_toml("north-star"));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .success();
}

/// The gate: the over-reach twin (identical workflow, but `read_temp`
/// declares `fs.read /etc/**` — outside the `[allow]` ceiling) fails
/// `tau check governance` with exit 2.
#[test]
fn north_star_over_reach_twin_fails_check_governance() {
    let dir = setup_project(&fixture_toml("north-star-over-reach"));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2);
}

/// Artifact path, governed: `tau build` (no `--allow-ungoverned`) succeeds
/// and the bundle records the Governed verdict (ADR-0057: governance is a
/// build-time property carried by the artifact).
#[test]
fn north_star_builds_governed_bundle() {
    let dir = setup_project(&fixture_toml("north-star"));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let out = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let bundle = String::from_utf8(out.stdout).unwrap().trim().to_string();

    let manifest = tau_pkg::bundle::manifest::BundleManifest::parse_str(
        &std::fs::read_to_string(&bundle).unwrap(),
    )
    .expect("bundle manifest parses");
    assert_eq!(
        manifest.governance.map(|g| g.verdict),
        Some(tau_pkg::bundle::GovernanceVerdict::Governed),
        "north-star bundle must carry the Governed verdict"
    );
}

/// Wasm path: control-flow now executes in-guest (ADR-0068), so the
/// refusal witness moves to the Suspend twin — the guest has no durable
/// suspend channel, and feature-fit refuses BEFORE any artifact exists.
#[test]
fn north_star_wasm_guest_build_is_refused_at_feature_fit() {
    let dir = setup_project(&fixture_toml("north-star-suspend"));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build", "--target", "wasm-guest"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("feature-fit"))
        .stderr(predicate::str::contains("Suspend"));
}

/// Dev path, execution witness on the ungoverned variant of the SAME
/// pipeline (keeps `--allow-ungoverned` + `[models]` covered; the governed
/// twin is `north_star_runs_governed_in_dev`): triage → Branch(route)/then
/// → Loop(review) → report, exit 0, pipeline-shaped completed outcome
/// carrying the final step's output.
#[test]
fn north_star_pipeline_executes_in_dev() {
    let dir = setup_project(&ungoverned_variant(&fixture_toml("north-star")));

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        // Entry agent id is the first pipeline step (`triage`); its
        // echo-llm backend is shared by every pipeline agent step.
        .args([
            "run",
            "--allow-ungoverned",
            "triage",
            "coolant alarm",
            "--json",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        // Force the mock sandbox so the toy plugin spawns natively (see
        // run_pipeline.rs for the rationale).
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "north-star pipeline run must succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    assert_completed_pipeline_outcome(&String::from_utf8(output.stdout).unwrap());
}

/// Artifact path, execution witness on the ungoverned variant (governed
/// twin: `north_star_governed_bundle_roundtrip`): `tau build` +
/// `tau run --bundle` replay the same Branch+Loop pipeline to the same
/// outcome.
#[test]
fn north_star_pipeline_bundle_roundtrip() {
    let dir = setup_project(&ungoverned_variant(&fixture_toml("north-star")));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let out = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build", "--allow-ungoverned"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let bundle = String::from_utf8(out.stdout).unwrap().trim().to_string();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--allow-ungoverned",
            "--bundle",
            &bundle,
            "triage",
            "coolant alarm",
            "--json",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "bundle north-star run must succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    assert_completed_pipeline_outcome(&String::from_utf8(output.stdout).unwrap());
}

/// Dev-path governed run (#620): the governed fixture runs WITHOUT
/// `--allow-ungoverned` and renders the final leaf step's output.
#[test]
fn north_star_runs_governed_in_dev() {
    let dir = setup_project(&fixture_toml("north-star"));

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["run", "triage", "coolant alarm", "--json"])
        .current_dir(dir.path())
        .env("TAU_HOME", dir.path().join("global"))
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "governed north-star run must succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    assert_completed_pipeline_outcome(&String::from_utf8(output.stdout).unwrap());
}

/// Artifact-path governed run (#620): governed build + governed bundle
/// run, no flags, same outcome.
#[test]
fn north_star_governed_bundle_roundtrip() {
    let dir = setup_project(&fixture_toml("north-star"));
    let tau_home = dir.path().join("global");
    std::fs::create_dir_all(&tau_home).unwrap();

    let out = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args(["build"])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .assert()
        .success()
        .get_output()
        .clone();
    let bundle = String::from_utf8(out.stdout).unwrap().trim().to_string();

    let output = AssertCmd::cargo_bin("tau")
        .unwrap()
        .args([
            "run",
            "--bundle",
            &bundle,
            "triage",
            "coolant alarm",
            "--json",
        ])
        .current_dir(dir.path())
        .env("TAU_HOME", &tau_home)
        .env("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "governed bundle run must succeed; stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    assert_completed_pipeline_outcome(&String::from_utf8(output.stdout).unwrap());
}

/// Wasm execution leg (#621 DoD, ADR-0068): the SAME Branch+Loop fixture
/// builds for wasm AND the guest executes it to the SAME terminal outcome
/// as the dev leg.
///
/// Completion is itself the control-flow witness — `report`'s input
/// template reads `${steps.escalate.output}` (produced only if the
/// Branch's then-arm ran) and `${steps.draft.output}` (only if the Loop
/// body ran), and template resolution hard-errors on unresolved refs. So
/// a returned payload proves both ran IN-GUEST, via `run_pipeline`.
///
/// The cassette gives each turn a DISTINCT text (the dev leg's echo-llm
/// replays one canned text for every agent, which cannot distinguish which
/// step's output came back). Here the payload identifies the step: only
/// `report`'s turn returns `WASM-FINAL-REPORT`, so the assertion pins the
/// last-leaf selection (`Pipeline::final_leaf_step_id` + the store lookup in
/// the guest) and the step ORDER, on top of the control-flow proof.
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn north_star_wasm_guest_executes_same_workflow_same_terminal_outcome() {
    let dir = setup_project(&fixture_toml("north-star"));

    // Lowering now ADMITS Branch+Loop for any-wasi-strict (the flip this
    // issue is about); before ADR-0068 this returned FeatureUnsupported.
    let (_module, ir_bytes) = tau_cli::cmd::build_wasm::lower_to_wasm_ir(dir.path())
        .expect("wasm lowering admits Branch+Loop (ADR-0068)");
    let component = common::wasm_component::build_component_with_ir(&ir_bytes);

    // The host cassette stands in for the echo-llm plugin: the guest's
    // `HostLlmBackend` pops one canned completion per agent turn, in order
    // (`VecDeque::pop_front`). Four agent turns run — triage, escalate
    // (Branch then-arm), draft (Loop body), report (last leaf) — and each
    // gets its own text. The markers are load-bearing: "URGENT" drives the
    // Branch's `matches` predicate onto the then-arm, and "APPROVED"
    // satisfies the Loop's `until` on the first iteration (exhaustion would
    // hard-error). Running short of responses surfaces as a host error.
    let response = |text: &str| {
        serde_json::json!({
            "text": text,
            "tool_uses": [],
            "stop_reason": "EndTurn",
            "usage": null,
        })
        .to_string()
    };
    let (payload, _events) = tau_wasm_host::run_component(
        &component,
        "incident: coolant temperature rising",
        vec![
            response("URGENT: coolant temperature rising - fan engaged"),
            response("escalated to the on-call rotation"),
            response("incident summary drafted - APPROVED"),
            response("WASM-FINAL-REPORT"),
        ],
    )
    .expect("guest executes the Branch+Loop pipeline");

    assert_eq!(
        payload, "WASM-FINAL-REPORT",
        "guest payload must be the LAST leaf step's output (report's turn), \
         not an earlier step's — same last-leaf contract the dev leg renders \
         as final_message"
    );

    // #689 positive control. This fixture reaches `matches` from BOTH the
    // Branch condition and the Loop's `until`, so it is the shape that must
    // still link the full engine after the goal-predicate gate — the run
    // above already proves the predicate evaluated correctly.
    //
    // It also keeps the gate's NEGATIVE assertions honest: those search the
    // name section for regex symbols, which would find nothing in a build
    // that stripped names, passing vacuously. This assertion fails first in
    // that case, so the pair cannot silently stop testing anything.
    assert!(
        common::wasm_component::links_regex_engine(&component),
        "a component whose IR reaches `matches` must still link the regex \
         engine; finding none here means either the #689 gate over-pruned or \
         the build stopped emitting a name section (which would make every \
         `!links_regex_engine` assertion vacuous)"
    );
}
