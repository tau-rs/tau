//! Property: cosmetic permutations of the source produce the same canonical bytes.
//!
//! Two source `tau.toml` files that differ only in whitespace, comments,
//! and key ordering inside the same table MUST produce byte-identical
//! `to_canonical_bytes`.

use tau_ir::canonical::to_canonical_bytes;
use tau_ir::lower::{lower_project, Caches};
use tau_pkg::ProjectConfig;
use tau_ports::target::registry;

fn lower(toml: &str, target: &tau_ports::target::TargetTriple) -> tau_ir::IrModule {
    let config = ProjectConfig::parse_str(toml).expect("parse");
    let caches = Caches {
        native_tool: &|_n: &str| Some([1u8; 32]),
        mcp_contract: &|_u: &str| Some(([2u8; 32], tau_ir::CapabilityRequirements::default())),
        skill: &|_n: &str| None,
    };
    lower_project(&config, target, &caches).expect("lower")
}

#[test]
fn cosmetic_permutations_produce_same_bytes() {
    let target = registry::list_available()
        .next()
        .expect("target")
        .triple
        .clone();

    let a = r#"
        [project]
        name = "demo"

        [agents.monitor]
        display_name = "monitor"
        package = "x@*"
        llm_backend = "anthropic"
        model = "M"
        tool_refs = ["t"]

        [tools.t]
        native = "T"
        capabilities = []
    "#;
    let b = r#"
        # leading comment
        [tools.t]
        capabilities = []                # tools first
        native = "T"                      # extra spaces

        [project]
        name = "demo"

        [agents.monitor]
        tool_refs = [ "t" ]              # whitespace
        model = "M"
        display_name = "monitor"
        package = "x@*"
        llm_backend = "anthropic"
    "#;

    let bytes_a = to_canonical_bytes(&lower(a, &target));
    let bytes_b = to_canonical_bytes(&lower(b, &target));
    assert_eq!(
        bytes_a, bytes_b,
        "cosmetic permutations must canonicalize to identical bytes"
    );
}
