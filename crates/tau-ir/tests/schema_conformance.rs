//! Validates the conformance kit against the published schema.
#![cfg(feature = "schema")]

use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/ir")
}

fn load(p: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(p)).unwrap()).unwrap()
}

fn compiled() -> jsonschema::Validator {
    jsonschema::validator_for(&load("tau-ir.v2.2.0.schema.json")).expect("schema compiles")
}

#[test]
fn valid_samples_validate() {
    let v = compiled();
    for name in ["minimal", "agents-tools", "triggers", "durable"] {
        let inst = load(&format!("conformance/valid/{name}.json"));
        assert!(v.is_valid(&inst), "valid/{name}.json should validate");
    }
}

#[test]
fn invalid_samples_are_rejected() {
    let v = compiled();
    for name in ["missing-ir-format", "unknown-node-kind"] {
        let inst = load(&format!("conformance/invalid/{name}.json"));
        assert!(!v.is_valid(&inst), "invalid/{name}.json should be rejected");
    }
}
