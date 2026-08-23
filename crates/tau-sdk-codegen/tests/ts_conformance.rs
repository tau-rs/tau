mod common;

use std::path::Path;

/// The fixture project.ts, authored against the generated @tau/sdk factory
/// surface, must extract + lower to the same canonical IR as the tau.toml.
#[test]
fn ts_fixture_lowers_equal_to_toml() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic_agent");

    let toml = std::fs::read_to_string(fixture.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    let ts_src = std::fs::read_to_string(fixture.join("project.ts")).unwrap();
    let ts_cfg = tau_ts_extract::extract_project(&ts_src, &fixture.join("project.ts"))
        .expect("extract project.ts");
    let ts_bytes = common::lower_config_bytes(&ts_cfg);

    assert_eq!(toml_bytes, ts_bytes, "TS fixture must lower equal to TOML");
}

/// The generated @tau/sdk source must declare each factory the fixture uses.
#[test]
fn generated_ts_declares_fixture_factories() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    tau_sdk_codegen::generate_into(repo_root, tmp.path()).expect("generate");
    let src = std::fs::read_to_string(tmp.path().join("sdk/ts/src/factories.ts")).unwrap();
    assert!(src.contains("export const agent"));
    assert!(src.contains("export const models"));
}
