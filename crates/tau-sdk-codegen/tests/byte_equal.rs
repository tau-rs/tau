mod common;

use std::path::Path;

/// EPIC 5.3 acceptance: the same agent authored in TOML, TS, and Python lowers
/// to byte-identical canonical IR. TOML and TS run in-process; Python is
/// executed live via python3 (skipped, with the TOML==TS assertion still
/// enforced, when python3 is unavailable).
#[test]
fn toml_ts_python_lower_to_identical_ir() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    tau_sdk_codegen::generate_into(repo_root, tmp.path()).expect("generate SDK");

    let fixture = manifest.join("tests/fixtures/basic_agent");

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
        "TOML and TS must lower to identical IR"
    );

    // Python (live)
    let sdk_python = tmp.path().join("sdk/python");
    match common::run_python_toml(&fixture.join("project.py"), Some(&sdk_python)) {
        None => eprintln!("SKIP: python3 unavailable; TOML==TS still asserted"),
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(
                toml_bytes, py_bytes,
                "TOML and Python must lower to identical IR"
            );
        }
    }
}
