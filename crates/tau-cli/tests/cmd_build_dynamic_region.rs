//! Integration test: `tau build` on a well-formed EPIC 4.5 dynamic region
//! (Task 8 conformance Fixture A).
//!
//! Mirrors `cmd_build.rs`'s `write_minimal_project`/`make_tau_home` harness
//! (isolated project tempdir + sibling `TAU_HOME` tempdir + minimal
//! schema-v6 lockfile) and reuses the exact well-formed-region TOML shape
//! from `governance.rs::well_formed_region_is_clean_and_note_is_gone`
//! (spawn kind's caps ⊆ region ceiling ⊆ root `[allow]`): a zero-*locked*-
//! package project whose only pipeline step is a `[pipeline.steps.dynamic]`
//! region. A dynamic region's owner agent is required (EPIC 4.5), so the
//! project declares one `[agents.coordinator]` — its package is never
//! materialized (no capability overrides on it), so `tau build`'s "verify
//! every locked package is materialized" step (driven by the lockfile, not
//! `[agents.*]`) is still a no-op against the empty lockfile below.
//!
//! Asserts `tau build` succeeds and the bundle's embedded IR payload
//! decodes to a module whose sole pipeline step is `StepRun::Dynamic`.

use assert_cmd::Command;

fn write_dynamic_region_project(root: &std::path::Path) {
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "dynamic-region-demo"
version = "0.1.0"

[allow]
"net.http" = { hosts = ["api.crawler.test"] }

[allow.models.fast]
backend = "coordbackend"
model = "m-1"

[agents.coordinator]
display_name = "Coordinator"
package      = "coordbackend@^0.1"
model        = "fast"

[agent.kinds.researcher]
capabilities = { "net.http" = { hosts = ["api.crawler.test"] } }
prompt       = "You are a researcher."
model        = "fast"

[[pipeline.steps]]
id = "fanout"

[pipeline.steps.dynamic]
spawns = ["researcher"]
ceiling = { "net.http" = { hosts = ["api.crawler.test"] } }
max_spawns = 4
max_concurrency = 2
agent = "coordinator"
"#,
    )
    .unwrap();
    // Empty schema-v6 lockfile: zero locked packages, so `tau build`'s
    // "verify every locked package is materialized" step is a no-op (it
    // walks `_lockfile.packages`, not `[agents.*]`).
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

#[test]
fn well_formed_dynamic_region_builds_and_ir_has_dynamic_step() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_dynamic_region_project(&project);
    let tau_home = make_tau_home(scratch.path());

    let output = Command::cargo_bin("tau")
        .unwrap()
        .arg("build")
        .current_dir(&project)
        .env("TAU_HOME", &tau_home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tau build should succeed for a well-formed dynamic region\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let bundle_path = stdout.trim();
    assert!(
        std::path::Path::new(bundle_path).exists(),
        "bundle file should exist at {bundle_path:?}"
    );

    // Decode the bundle's embedded IR payload and assert it carries a
    // `StepRun::Dynamic` pipeline step (not just that the build exited 0).
    let bundle_toml = std::fs::read_to_string(bundle_path).unwrap();
    let manifest = tau_pkg::bundle::manifest::BundleManifest::parse_str(&bundle_toml)
        .expect("bundle manifest parses");
    let ir_payload = manifest
        .ir_payload
        .expect("well-formed project must produce an IR payload");
    let ir_bytes = hex_decode(&ir_payload.canonical_ir_bytes_hex);
    let module = tau_ir::from_canonical_bytes(&ir_bytes).expect("canonical IR decodes");
    let pipeline = module
        .workflow
        .pipeline
        .expect("module must carry the authored pipeline");
    assert_eq!(pipeline.steps.len(), 1, "expected the single 'fanout' step");
    assert!(
        matches!(
            pipeline.steps[0].run,
            tau_ir::pipeline::StepRun::Dynamic { .. }
        ),
        "expected StepRun::Dynamic, got {:?}",
        pipeline.steps[0].run
    );
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
