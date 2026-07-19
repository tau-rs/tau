//! Property: cosmetic permutations of the source produce the same canonical bytes.
//!
//! Two source `tau.toml` files that differ only in whitespace, comments,
//! and key ordering inside the same table MUST produce byte-identical
//! `to_canonical_bytes`.

use tau_ir::canonical::to_canonical_bytes;
use tau_ir_lower::{lower_project, Caches};
use tau_pkg::ProjectConfig;
use tau_ports::target::registry;

fn lower(toml: &str, target: &tau_ports::target::TargetTriple) -> tau_ir::IrModule {
    let config = ProjectConfig::parse_str(toml).expect("parse");
    let caches = Caches {
        native_tool: &|_n: &str| Some([1u8; 32]),
        mcp_contract: &|_u: &str| {
            Some(tau_ir_lower::ResolvedMcpContract {
                hash: [2u8; 32],
                expanded_tools: vec![],
                requires_sampling: false,
            })
        },
        skill: &|_n: &str| None,
        prompt_file: &|_| Ok(Vec::new()),
    };
    lower_project(&config, target, &caches)
        .expect("lower")
        .module
}

#[test]
fn cosmetic_permutations_produce_same_bytes() {
    let target = registry::list_available().next().expect("target").triple;

    let a = r#"
        packages = ["mock-llm"]

        [project]
        name = "demo"

        [models]
        default = { backend = "mock-llm", model = "mock-model" }

        [agents.monitor]
        display_name = "monitor"
        package = "x@*"
        model = "default"
        tool_refs = ["t"]

        [tools.t]
        native = "T"
        capabilities = []
    "#;
    let b = r#"
        # leading comment
        packages = ["mock-llm"]

        [tools.t]
        capabilities = []                # tools first
        native = "T"                      # extra spaces

        [project]
        name = "demo"

        [models]
        default = { backend = "mock-llm", model = "mock-model" }

        [agents.monitor]
        tool_refs = [ "t" ]              # whitespace
        model = "default"
        display_name = "monitor"
        package = "x@*"
    "#;

    let bytes_a = to_canonical_bytes(&lower(a, &target));
    let bytes_b = to_canonical_bytes(&lower(b, &target));
    assert_eq!(
        bytes_a, bytes_b,
        "cosmetic permutations must canonicalize to identical bytes"
    );
}
