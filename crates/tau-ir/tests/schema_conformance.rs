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
    jsonschema::validator_for(&load("tau-ir.v2.4.0.schema.json")).expect("schema compiles")
}

#[test]
fn valid_samples_validate() {
    let v = compiled();
    for name in [
        "minimal",
        "agents-tools",
        "triggers",
        "durable",
        "control_flow_branch",
    ] {
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

#[test]
fn decoder_rejects_forward_incompatible_and_unknown_fields() {
    for name in [
        "unknown-top-level-field",
        "unknown-nested-field",
        "ir-format-minor-plus-1",
        "ir-format-major-plus-1",
    ] {
        let bytes = std::fs::read(
            dir()
                .join("conformance/invalid")
                .join(format!("{name}.json")),
        )
        .unwrap_or_else(|_| panic!("read fixture {name}"));
        assert!(
            tau_ir::from_canonical_bytes(&bytes).is_err(),
            "fixture {name} must be rejected by the decoder",
        );
    }
}

#[test]
fn valid_samples_deserialize_through_tau_ir() {
    for name in [
        "minimal",
        "agents-tools",
        "triggers",
        "durable",
        "control_flow_branch",
    ] {
        let raw =
            std::fs::read_to_string(dir().join(format!("conformance/valid/{name}.json"))).unwrap();
        let parsed: Result<tau_ir::module::IrModule, _> = serde_json::from_str(&raw);
        assert!(
            parsed.is_ok(),
            "valid/{name}.json must deserialize into IrModule: {:?}",
            parsed.err()
        );
    }
}
