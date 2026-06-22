//! EPIC 2.2: every published sample IR validates against the published IR
//! JSON schema. This is the load-bearing proof that the generated schema
//! accepts real IR (i.e. schemars reproduces the serde encoding) — if it
//! ever rejects valid IR, this fails loudly.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn published_samples_validate_against_published_schema() {
    let root = repo_root();
    let schema_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("schema/ir/tau-ir.schema.json"))
            .expect("schema present"),
    )
    .expect("schema parses");
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7) // schemars 0.8 emits Draft 7
        .build(&schema_json)
        .expect("schema compiles as a validator");

    let dir = root.join("schema/ir/samples");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("samples dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let instance: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{} failed schema validation:\n{}",
            path.display(),
            errors.join("\n")
        );
        count += 1;
    }
    assert!(count >= 4, "expected >=4 samples, found {count}");
}
