use std::path::Path;

/// The checked-in `sdk/embed-js/` scaffold must equal a fresh render. If
/// this fails, run `cargo run -p tau-cli -- embed --host js -o .` and
/// commit the result.
#[test]
fn committed_embed_js_matches_fresh_render() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let rendered = tau_sdk_codegen::embed_js::render_embed_js();
    let mut drifted = Vec::new();
    for (rel, expected) in &rendered {
        let actual = std::fs::read_to_string(repo_root.join(rel)).unwrap_or_default();
        if &actual != expected {
            drifted.push(rel.display().to_string());
        }
    }
    assert!(
        drifted.is_empty(),
        "committed sdk/embed-js drifted; regenerate:\n{}",
        drifted.join("\n")
    );
}
