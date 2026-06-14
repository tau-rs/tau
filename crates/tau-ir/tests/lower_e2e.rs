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

#[test]
fn lowers_goals_and_deliverables_into_checks() {
    use tau_ir::pipeline::StepRun;
    use tau_ir::{AgentId, CheckId, OnFail, PipelineStepId};

    // Worked example: gather -> writer pipeline; writer produces the
    // report and holds a covering fs.write capability; one goal
    // (regex match) and one deliverable (path locus, retry from writer).
    let toml = r#"
        [project]
        name = "research"

        [agents.gather]
        display_name = "Gather"
        package      = "research@^0.1"
        llm_backend  = "anthropic"
        model        = "claude-haiku-4-5"

        [agents.writer]
        display_name = "Writer"
        package      = "research@^0.1"
        llm_backend  = "anthropic"
        model        = "claude-haiku-4-5"
        produces     = ["/workspace/report.md"]
        tool_refs    = ["write_file"]

        [tools.write_file]
        native = "WriteFile"
        capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]

        [[pipeline.steps]]
        id    = "gather"
        run   = "agent:gather"
        input = "${input}"

        [[pipeline.steps]]
        id    = "writer"
        run   = "agent:writer"
        input = "${steps.gather.output}"

        [goals.has_sources]
        evaluates = "/workspace/report.md"
        check     = "matches"
        pattern   = "(?m)^## Sources"

        [deliverables.report]
        path         = "/workspace/report.md"
        must_satisfy = "A coherent summary."
        on_fail      = "retry"
        max_attempts = 3
        retry_from   = "writer"
    "#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec!["WriteFile".into()], vec![]);
    let ir = lower_project(&config, &target, &caches).expect("lower");

    // produces copied onto the IR Agent.
    assert_eq!(
        ir.workflow.agents[&AgentId("writer".into())].produces,
        vec!["/workspace/report.md".to_string()]
    );

    // two checks present.
    assert_eq!(ir.workflow.checks.len(), 2);

    // checks appended after writer, in order: goal(has_sources) then
    // deliverable(report).
    let pipe = ir.workflow.pipeline.as_ref().expect("pipeline present");
    let tail: Vec<_> = pipe.steps.iter().rev().take(2).map(|s| &s.run).collect();
    assert!(
        matches!(tail[1], StepRun::Check(CheckId(ref s)) if s == "has_sources"),
        "expected has_sources before report; got {:?}",
        pipe.steps.iter().map(|s| &s.run).collect::<Vec<_>>()
    );
    assert!(
        matches!(tail[0], StepRun::Check(CheckId(ref s)) if s == "report"),
        "expected report last; got {:?}",
        pipe.steps.iter().map(|s| &s.run).collect::<Vec<_>>()
    );

    // gate resolves to the producer step; on_fail is Retry.
    let report = &ir.workflow.checks[&CheckId("report".into())];
    assert_eq!(report.retry.gate, PipelineStepId("writer".into()));
    assert_eq!(report.retry.on_fail, OnFail::Retry);
}

#[test]
fn lowers_context_pipeline_onto_agent() {
    let toml = r#"
[project]
name = "ctx-lower"

[agents.a]
display_name = "A"
package      = "demo@^0.1"
llm_backend  = "mock-llm"
model        = "m"

[[agents.a.context.pipeline]]
transformer = "trim_old"
[agents.a.context.steps.trim_old]
keep_last_turns = 4

[[agents.a.context.pipeline]]
transformer = "fit_budget"
[agents.a.context.steps.fit_budget]
max_tokens = 4000
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let module = lower_project(&config, &target, &caches).expect("lower");
    let agent = module
        .workflow
        .agents
        .get(&tau_ir::AgentId("a".into()))
        .unwrap();
    let ctx = agent.context.as_ref().expect("context present");
    assert_eq!(ctx.pipeline.len(), 2);
    assert_eq!(ctx.pipeline[0].transformer, "trim_old");
    assert_eq!(ctx.pipeline[1].transformer, "fit_budget");
}

#[test]
fn explicit_check_placement_is_not_double_appended() {
    use tau_ir::pipeline::StepRun;
    use tau_ir::{AgentId, CheckId, PipelineStepId};

    // gather → writer pipeline that ALSO contains an explicit
    // `run = "check:report"` step BEFORE the tail.
    // The deliverable check must appear EXACTLY ONCE at the explicitly
    // declared position, not also auto-appended at the end.
    let toml = r#"
        [project]
        name = "explicit-check"

        [agents.gather]
        display_name = "Gather"
        package      = "research@^0.1"
        llm_backend  = "anthropic"
        model        = "claude-haiku-4-5"

        [agents.writer]
        display_name = "Writer"
        package      = "research@^0.1"
        llm_backend  = "anthropic"
        model        = "claude-haiku-4-5"
        produces     = ["/workspace/report.md"]
        tool_refs    = ["write_file"]

        [tools.write_file]
        native = "WriteFile"
        capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]

        [[pipeline.steps]]
        id    = "gather"
        run   = "agent:gather"
        input = "${input}"

        [[pipeline.steps]]
        id    = "writer"
        run   = "agent:writer"
        input = "${steps.gather.output}"

        [[pipeline.steps]]
        id    = "check-report"
        run   = "check:report"
        input = "${input}"

        [[pipeline.steps]]
        id    = "gather2"
        run   = "agent:gather"
        input = "${steps.check-report.output}"

        [deliverables.report]
        path         = "/workspace/report.md"
        must_satisfy = "A coherent summary."
        on_fail      = "abort"
    "#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec!["WriteFile".into()], vec![]);
    let ir = lower_project(&config, &target, &caches).expect("lower");

    let pipe = ir.workflow.pipeline.as_ref().expect("pipeline present");

    // Count occurrences of StepRun::Check(CheckId("report")) in the pipeline.
    let check_report_positions: Vec<usize> = pipe
        .steps
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if matches!(&s.run, StepRun::Check(CheckId(id)) if id == "report") {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        check_report_positions.len(),
        1,
        "check:report must appear exactly once; got positions {:?} in steps: {:?}",
        check_report_positions,
        pipe.steps.iter().map(|s| &s.run).collect::<Vec<_>>()
    );

    // Assert it is at the explicitly-declared position (index 2, after
    // gather and writer, before gather2).
    assert_eq!(
        check_report_positions[0],
        2,
        "check:report must be at the explicitly-declared position (index 2); \
         got index {}; steps: {:?}",
        check_report_positions[0],
        pipe.steps.iter().map(|s| &s.run).collect::<Vec<_>>()
    );

    // And confirm the step after is gather2 (not another check).
    assert!(
        matches!(&pipe.steps[3].run, StepRun::Agent(AgentId(id)) if id == "gather"),
        "step after the explicit check must be gather2 (agent:gather); got {:?}",
        &pipe.steps[3].run
    );

    // The PipelineStepId at position 2 is the one declared in TOML ("check-report").
    assert_eq!(
        pipe.steps[2].id,
        PipelineStepId("check-report".into()),
        "step id at position 2 must be 'check-report'"
    );
}
