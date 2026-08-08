mod common;

use std::path::Path;

/// EPIC 5.3 acceptance: the same agent authored in TOML, TS, and Python lowers
/// to byte-identical canonical IR. TOML and TS run in-process; Python is
/// executed live via python3 (skipped, with the TOML==TS assertion still
/// enforced, when python3 is unavailable — unless `TAU_REQUIRE_PYTHON3=1`, in
/// which case the missing interpreter is a hard failure; see `common`).
fn assert_three_way_equal(fixture_name: &str) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    tau_sdk_codegen::generate_into(repo_root, tmp.path()).expect("generate SDK");

    let fixture = manifest.join("tests/fixtures").join(fixture_name);

    // TOML
    let toml = std::fs::read_to_string(fixture.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    // TS (in-process via swc)
    let ts_src = std::fs::read_to_string(fixture.join("project.ts")).unwrap();
    let ts_cfg = tau_ts_extract::extract_project(&ts_src, &fixture.join("project.ts"))
        .expect("extract project.ts");
    let ts_bytes = common::lower_config_bytes(&ts_cfg);

    assert_eq!(
        toml_bytes, ts_bytes,
        "TOML and TS must lower to identical IR ({fixture_name})"
    );

    // Python (live)
    let sdk_python = tmp.path().join("sdk/python");
    match common::run_python_toml(&fixture.join("project.py"), Some(&sdk_python)) {
        None => eprintln!("SKIP: python3 unavailable; TOML==TS still asserted ({fixture_name})"),
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(
                toml_bytes, py_bytes,
                "TOML and Python must lower to identical IR ({fixture_name})"
            );
        }
    }
}

/// Minimal case: an agent with just a model reference.
#[test]
fn basic_agent_lowers_to_identical_ir() {
    assert_three_way_equal("basic_agent");
}

/// Richer case: an agent with a native tool + an `fs.read` capability + a
/// system prompt — the surfaces lowering actually does structural work on.
#[test]
fn tool_agent_lowers_to_identical_ir() {
    assert_three_way_equal("tool_agent");
}
