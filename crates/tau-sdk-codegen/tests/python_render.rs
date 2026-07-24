mod common;

use std::path::Path;

/// The generated Python SDK, driving the basic_agent fixture, must render TOML
/// that lowers to the same canonical IR as the fixture's tau.toml.
#[test]
fn generated_python_sdk_lowers_equal_to_toml() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();

    // Regenerate the SDK into the repo tree so the test drives current output.
    tau_sdk_codegen::generate(repo_root).expect("generate SDK");

    let fixture = manifest.join("tests/fixtures/basic_agent");
    let toml = std::fs::read_to_string(fixture.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    let sdk_python = repo_root.join("sdk/python");
    match common::run_python_toml(&fixture.join("project.py"), Some(&sdk_python)) {
        None => eprintln!("SKIP: python3 not available"),
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(toml_bytes, py_bytes, "python SDK output must lower equal");
        }
    }
}
