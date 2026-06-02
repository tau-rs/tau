//! End-to-end lowering test against a minimal tau.toml.

use tau_ir::lower::{lower_project, Caches};
use tau_ir::{IrError, IrFormatVersion};
use tau_pkg::project::ProjectConfig;
use tau_ports::target::TargetTriple;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hash_of(s: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().into()
}

/// Build a `Caches` whose closures own their data (so they are `'static`).
///
/// `native_known` and `mcp_known` are cloned into boxed closures so there
/// are no non-`'static` borrows in the `Fn` impls.
fn caches_with(native_known: Vec<String>, mcp_known: Vec<String>) -> Caches<'static> {
    Caches {
        native_tool: Box::leak(Box::new(move |name: &str| -> Option<[u8; 32]> {
            native_known
                .iter()
                .find(|n| n.as_str() == name)
                .map(|n| hash_of(n))
        })),
        mcp_contract: Box::leak(Box::new(
            move |url: &str| -> Option<tau_ir::lower::ResolvedMcpContract> {
                mcp_known.iter().find(|u| u.as_str() == url).map(|u| {
                    tau_ir::lower::ResolvedMcpContract {
                        hash: hash_of(u),
                        expanded_tools: vec![],
                        requires_sampling: false,
                    }
                })
            },
        )),
        skill: Box::leak(Box::new(|_name: &str| -> Option<[u8; 32]> { None })),
    }
}

/// Return the first Available target from the registry.
fn lookup_first_available() -> TargetTriple {
    tau_ports::target::registry::list_available()
        .next()
        .expect("at least one available target")
        .triple
}

/// Return a target triple that does NOT include `NetworkHttp` in its
/// `required_shapes`. As of β.2 start, every Available entry in the
/// registry includes NetworkHttp (all use `fs_rw_exec_net` or
/// `all_shapes`). We therefore use a synthetic triple not in the
/// registry — `registry::lookup` returns `None`, and
/// `capability_fit::check` returns `CapabilityFitFailed` with an empty
/// `missing` list. The test is marked `#[ignore]` because there is no
/// real registry entry that exercises the shape-miss path; a future PR
/// adding a `no-network` target tier should un-ignore this test.
///
/// IGNORE-REASON: no Available registry entry exists without NetworkHttp;
/// the second test is a design placeholder for when such an entry lands.
#[allow(dead_code)]
fn lookup_target_excluding_network() -> TargetTriple {
    // Synthetic triple not in the registry → registry::lookup returns None
    // → capability_fit::check returns CapabilityFitFailed { missing: [], tools: [] }.
    // The assert `matches!(err, IrError::CapabilityFitFailed { .. })` passes.
    "darwin-container-strict".parse().unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn lowering_passes_minimal_workflow() {
    // Uses `[agents.<id>]` (plural) per the existing tau.toml convention;
    // the spec example's `[agent.X]` (singular) is non-normative.
    let toml = r#"
        [project]
        name = "temp-monitor"

        [agents.monitor]
        display_name = "Monitor"
        package      = "monitor@^0.1"
        llm_backend  = "anthropic"
        model        = "claude-haiku-4-5"
        tool_refs    = ["read_temp", "set_fan"]

        [tools.read_temp]
        native = "ReadTemp"
        capabilities = []

        [tools.set_fan]
        native = "SetFan"
        capabilities = []
    "#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec!["ReadTemp".into(), "SetFan".into()], vec![]);
    let module = lower_project(&config, &target, &caches).expect("lower");
    assert_eq!(module.ir_format.0, IrFormatVersion::CURRENT);
    assert!(
        module
            .workflow
            .agents
            .contains_key(&tau_ir::AgentId("monitor".into())),
        "expected agent 'monitor' in workflow"
    );
    assert!(
        module
            .workflow
            .tools
            .contains_key(&tau_ir::ToolId("read_temp".into())),
        "expected tool 'read_temp' in workflow"
    );
    assert!(
        module
            .workflow
            .tools
            .contains_key(&tau_ir::ToolId("set_fan".into())),
        "expected tool 'set_fan' in workflow"
    );
}

#[test]
// IGNORE-REASON: no Available registry entry exists without NetworkHttp;
// this test is a design placeholder for when a `no-network` target tier
// lands in the registry. The capability_fit logic is already exercised
// via the synthetic-triple path, but the semantic "shape miss" path
// requires a real entry with a constrained shape set.
#[ignore]
fn lowering_refuses_on_capability_fit_mismatch() {
    // Workflow declares network; build for a target without NetworkHttp shape.
    let toml = r#"
        [project]
        name = "net-workflow"

        [agents.x]
        display_name = "X"
        package      = "x@^0.1"
        llm_backend  = "anthropic"
        model        = "x"
        tool_refs    = ["weather"]

        [tools.weather]
        mcp = "https://example.com"
        capabilities = [{ kind = "net.http" }]
    "#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    // Use a target triple that EXCLUDES NetworkHttp from required_shapes.
    // Currently no such Available entry exists in the registry; the test
    // is #[ignore] until one is added.
    let target = lookup_target_excluding_network();
    let caches = caches_with(vec![], vec!["https://example.com".into()]);
    let err = lower_project(&config, &target, &caches).unwrap_err();
    assert!(matches!(err, IrError::CapabilityFitFailed { .. }));
}
