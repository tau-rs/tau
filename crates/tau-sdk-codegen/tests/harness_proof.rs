mod common;

use std::path::Path;

/// Proves the python3 → TOML → ProjectConfig → IR path yields the same canonical
/// bytes as the hand-written tau.toml. This is the mechanical spine of the 5.3
/// acceptance test, isolated so it fails loudly if the toolchain wiring breaks.
#[test]
fn python_emitted_toml_lowers_equal_to_native_toml() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harness");

    let toml = std::fs::read_to_string(dir.join("tau.toml")).unwrap();
    let toml_bytes = common::lower_toml_bytes(&toml);

    match common::run_python_toml(&dir.join("emit_toml.py"), None) {
        None => {
            eprintln!("SKIP: python3 not available; skipping python-path assertion");
        }
        Some(py_toml) => {
            let py_bytes = common::lower_toml_bytes(&py_toml);
            assert_eq!(toml_bytes, py_bytes, "python-emitted TOML must lower equal");
        }
    }
}
