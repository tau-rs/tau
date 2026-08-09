use std::path::Path;

/// The checked-in sdk/ packages must equal a fresh render. If this fails, run
/// `cargo run -p tau-sdk-codegen --bin gen` and commit the result.
#[test]
fn committed_sdk_matches_fresh_render() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let rendered = tau_sdk_codegen::emit::render_all(repo_root).expect("render");

    let mut drifted = Vec::new();
    for (rel, expected) in &rendered {
        let path = repo_root.join(rel);
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        if &actual != expected {
            drifted.push(rel.display().to_string());
        }
    }
    assert!(
        drifted.is_empty(),
        "committed SDK drifted from generator; run `cargo run -p tau-sdk-codegen --bin gen` and commit:\n{}",
        drifted.join("\n")
    );
}
