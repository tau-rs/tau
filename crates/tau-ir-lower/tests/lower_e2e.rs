//! End-to-end lowering test against a minimal tau.toml.

use tau_ir::IrFormatVersion;
use tau_ir_lower::LowerError;
use tau_ir_lower::{lower_project, Caches};
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
            move |url: &str| -> Option<tau_ir_lower::ResolvedMcpContract> {
                mcp_known.iter().find(|u| u.as_str() == url).map(|u| {
                    tau_ir_lower::ResolvedMcpContract {
                        hash: hash_of(u),
                        expanded_tools: vec![],
                        requires_sampling: false,
                    }
                })
            },
        )),
        skill: Box::leak(Box::new(|_name: &str| -> Option<[u8; 32]> { None })),
        prompt_file: &|_| Ok(Vec::new()),
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
    // The assert `matches!(err, LowerError::CapabilityFitFailed { .. })` passes.
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
        packages = ["mock-llm"]

        [project]
        name = "temp-monitor"

        [models]
        default = { backend = "mock-llm", model = "mock-model" }

        [agents.monitor]
        display_name = "Monitor"
        package      = "monitor@^0.1"
        model        = "default"
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
    let module = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;
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
        packages = ["mock-llm"]

        [project]
        name = "net-workflow"

        [models]
        default = { backend = "mock-llm", model = "mock-model" }

        [agents.x]
        display_name = "X"
        package      = "x@^0.1"
        model        = "default"
        tool_refs    = ["weather"]

        [tools.weather]
        mcp = "https://example.com"
        capabilities = [{ kind = "net.http", hosts = "any" }]
    "#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    // Use a target triple that EXCLUDES NetworkHttp from required_shapes.
    // Currently no such Available entry exists in the registry; the test
    // is #[ignore] until one is added.
    let target = lookup_target_excluding_network();
    let caches = caches_with(vec![], vec!["https://example.com".into()]);
    let err = lower_project(&config, &target, &caches).unwrap_err();
    assert!(matches!(err, LowerError::CapabilityFitFailed { .. }));
}

#[test]
fn lowers_goals_and_deliverables_into_checks() {
    use tau_ir::pipeline::StepRun;
    use tau_ir::{AgentId, CheckId, OnFail, PipelineStepId};

    // Worked example: gather -> writer pipeline; writer produces the
    // report and holds a covering fs.write capability; one goal
    // (regex match) and one deliverable (path locus, retry from writer).
    let toml = r#"
        packages = ["mock-llm"]

        [project]
        name = "research"

        [models]
        default = { backend = "mock-llm", model = "mock-model" }

        [agents.gather]
        display_name = "Gather"
        package      = "research@^0.1"
        model        = "default"

        [agents.writer]
        display_name = "Writer"
        package      = "research@^0.1"
        model        = "default"
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
    let ir = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;

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
packages = ["mock-llm"]

[project]
name = "ctx-lower"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "demo@^0.1"
model        = "default"

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
    let module = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;
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

/// D7-B PR3: an explicit `llm_backed` / `stateful` determinism string lowers
/// to the matching `DeterminismClass`. Guards the explicit match arms against
/// regressing back to the silent `_ => Pure` default.
#[test]
fn known_determinism_strings_lower_to_their_class() {
    use tau_ir::context::DeterminismClass;
    let toml = r#"
packages = ["mock-llm"]

[project]
name = "det-lower"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "demo@^0.1"
model        = "default"

[[agents.a.context.pipeline]]
transformer = "trim_old"
determinism = "llm_backed"

[[agents.a.context.pipeline]]
transformer = "compact_tool_outputs"
determinism = "stateful"

[[agents.a.context.pipeline]]
transformer = "fit_budget"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let module = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;
    let agent = module
        .workflow
        .agents
        .get(&tau_ir::AgentId("a".into()))
        .unwrap();
    let ctx = agent.context.as_ref().expect("context present");
    assert_eq!(ctx.pipeline[0].determinism, DeterminismClass::LlmBacked);
    assert_eq!(ctx.pipeline[1].determinism, DeterminismClass::Stateful);
    assert_eq!(ctx.pipeline[2].determinism, DeterminismClass::Pure);
}

/// D7-B PR3: an unknown determinism string is a hard build error, not a
/// silent downgrade to `Pure` (the most permissive class). Per ADR-0065.
#[test]
fn unknown_determinism_string_is_rejected() {
    let toml = r#"
packages = ["mock-llm"]

[project]
name = "det-bad"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "demo@^0.1"
model        = "default"

[[agents.a.context.pipeline]]
transformer = "trim_old"
determinism = "sometimes"

[[agents.a.context.pipeline]]
transformer = "fit_budget"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let err = lower_project(&config, &target, &caches).expect_err("must reject");
    match err {
        LowerError::UnknownDeterminism {
            agent,
            transformer,
            determinism,
        } => {
            assert_eq!(agent, "a");
            assert_eq!(transformer, "trim_old");
            assert_eq!(determinism, "sometimes");
        }
        other => panic!("expected UnknownDeterminism, got {other:?}"),
    }
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
        packages = ["mock-llm"]

        [project]
        name = "explicit-check"

        [models]
        default = { backend = "mock-llm", model = "mock-model" }

        [agents.gather]
        display_name = "Gather"
        package      = "research@^0.1"
        model        = "default"

        [agents.writer]
        display_name = "Writer"
        package      = "research@^0.1"
        model        = "default"
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
    let ir = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;

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
        pipe.steps[3].run
    );

    // The PipelineStepId at position 2 is the one declared in TOML ("check-report").
    assert_eq!(
        pipe.steps[2].id,
        PipelineStepId("check-report".into()),
        "step id at position 2 must be 'check-report'"
    );
}

#[test]
fn agent_output_schema_survives_lowering() {
    let toml = r#"
packages = ["mock"]

[project]
name = "p"

[models.mock-1]
backend = "mock"
model = "mock-1"

[agents.judge]
display_name = "Judge"
package = "p@^0.1"
model = "mock-1"
output_schema = { type = "object" }
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let module = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;
    let agent = module
        .workflow
        .agents
        .get(&tau_ir::AgentId("judge".into()))
        .expect("agent");
    assert_eq!(
        agent.output_schema,
        Some(serde_json::json!({"type": "object"}))
    );
}

#[test]
fn lowering_resolves_model_from_allow_models() {
    // A governed project (ADR-0057 / EPIC 1.2) moves its model alias map under
    // [allow.models] when an [allow] ceiling is declared. Lowering must resolve
    // the alias there, not only from a top-level [models] table.
    let toml = r#"
        packages = ["mock-llm"]

        [project]
        name = "governed-monitor"

        [allow]
        "fs.read" = { paths = ["/proj/**"] }

        [allow.models.default]
        backend = "mock-llm"
        model   = "mock-model"

        [agents.monitor]
        display_name = "Monitor"
        package      = "monitor@^0.1"
        model        = "default"
    "#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let module = lower_project(&config, &target, &caches)
        .expect("governed project with [allow.models] must lower")
        .module;
    assert!(
        module
            .workflow
            .agents
            .contains_key(&tau_ir::AgentId("monitor".into())),
        "expected agent 'monitor' in workflow"
    );
}

// ---------------------------------------------------------------------------
// D6-B: `system_file` prompts lower to content-addressed assets.
// ---------------------------------------------------------------------------

/// Two agents referencing the SAME `system_file` lower to one deduped asset,
/// each agent's prompt becomes a `PromptSource::Asset` with the content hash,
/// and the blob carries the file bytes. Proves the non-hermetic path bug is
/// fixed: the IR carries a content hash, never the path string.
#[test]
fn system_file_prompts_lower_to_deduped_content_addressed_assets() {
    let toml = r#"
packages = ["mock-llm"]

[project]
name = "asset-demo"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "demo@^0.1"
model        = "default"
[agents.a.prompt]
system_file = "shared.md"

[agents.b]
display_name = "B"
package      = "demo@^0.1"
model        = "default"
[agents.b.prompt]
system_file = "shared.md"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    const CONTENT: &[u8] = b"You are a careful assistant.";

    let caches = Caches {
        native_tool: &|_| None,
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|p: &std::path::Path| {
            assert_eq!(p, std::path::Path::new("shared.md"));
            Ok(CONTENT.to_vec())
        },
    };
    let out = lower_project(&config, &target, &caches).expect("lower");

    let expected_hash = tau_ir::asset::asset_hash(CONTENT);
    for id in ["a", "b"] {
        let agent = out
            .module
            .workflow
            .agents
            .get(&tau_ir::AgentId(id.into()))
            .unwrap_or_else(|| panic!("agent {id} present"));
        assert_eq!(
            agent.prompt.asset_hash(),
            Some(expected_hash.as_str()),
            "agent {id} prompt must be a content-addressed asset ref (never the path)"
        );
    }

    // Dedup by construction: identical content across agents => one blob.
    assert_eq!(
        out.assets.len(),
        1,
        "identical prompts must dedup to one blob"
    );
    let blob = out.assets.get(&expected_hash).expect("asset blob present");
    assert_eq!(blob.bytes, CONTENT, "blob carries the file content");
    assert_eq!(blob.kind, tau_ir::asset::AssetKind::Prompt);
}

/// A missing/unreadable `system_file` prompt is a hard build error (D6-B moves
/// prompt-file existence from run time to build time), naming the agent + path.
#[test]
fn missing_system_file_prompt_is_a_build_error() {
    let toml = r#"
packages = ["mock-llm"]

[project]
name = "asset-missing"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "demo@^0.1"
model        = "default"
[agents.a.prompt]
system_file = "gone.md"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();

    let caches = Caches {
        native_tool: &|_| None,
        mcp_contract: &|_| None,
        skill: &|_| None,
        prompt_file: &|p: &std::path::Path| {
            Err(tau_ir_lower::PromptFileError(format!(
                "no such file: {}",
                p.display()
            )))
        },
    };
    match lower_project(&config, &target, &caches) {
        Err(LowerError::PromptFileUnreadable { agent, path, .. }) => {
            assert_eq!(agent.0, "a");
            assert_eq!(path, "gone.md");
        }
        other => panic!("expected PromptFileUnreadable build error, got {other:?}"),
    }
}

#[test]
fn lowers_authored_branch_into_steprun_branch() {
    use tau_ir::check::{GoalPredicate, Locus};
    use tau_ir::pipeline::StepRun;

    let toml = r#"
packages = ["mock-llm"]
[project]
name = "branch-demo"
[models.m]
backend = "mock-llm"
model = "m"
[agents.triage]
display_name = "Triage"
package = "branch-demo@^0.1"
model = "m"
max_turns = 1
[agents.oncall]
display_name = "Oncall"
package = "branch-demo@^0.1"
model = "m"
max_turns = 1
[agents.writer]
display_name = "Writer"
package = "branch-demo@^0.1"
model = "m"
max_turns = 1
[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "matches", pattern = "(?i)urgent" }
[[pipeline.steps.then]]
id = "escalate"
run = "agent:oncall"
input = "${steps.triage.output}"
[[pipeline.steps.otherwise]]
id = "ack"
run = "agent:writer"
input = "${steps.triage.output}"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let module = lower_project(&config, &target, &caches)
        .expect("lower")
        .module;

    let pipe = module.workflow.pipeline.as_ref().expect("pipeline present");
    assert_eq!(pipe.steps.len(), 2);
    match &pipe.steps[1].run {
        StepRun::Branch {
            on,
            then,
            otherwise,
        } => {
            assert!(matches!(&on.evaluates, Locus::Output(s) if s.0 == "triage"));
            assert!(matches!(&on.predicate, GoalPredicate::Matches(p) if p == "(?i)urgent"));
            assert_eq!(then.len(), 1);
            assert!(matches!(&then[0].run, StepRun::Agent(a) if a.0 == "oncall"));
            assert_eq!(otherwise.len(), 1);
            assert!(matches!(&otherwise[0].run, StepRun::Agent(a) if a.0 == "writer"));
        }
        other => panic!("expected Branch, got {other:?}"),
    }
}

#[test]
fn branch_arm_referencing_ghost_agent_is_rejected() {
    let toml = r#"
packages = ["mock-llm"]
[project]
name = "branch-ghost"
[models.m]
backend = "mock-llm"
model = "m"
[agents.triage]
display_name = "Triage"
package = "branch-ghost@^0.1"
model = "m"
max_turns = 1
[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "non_empty" }
[[pipeline.steps.then]]
id = "boom"
run = "agent:ghost"
input = "${input}"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let err = lower_project(&config, &target, &caches).unwrap_err();
    assert!(
        matches!(err, LowerError::UnknownPipelineRun { .. }),
        "expected UnknownPipelineRun for ghost arm agent, got {err:?}"
    );
}

#[test]
fn branch_condition_reading_out_of_scope_output_is_rejected() {
    // `route` reads `steps.later.output`, but `later` runs AFTER `route`.
    let toml = r#"
packages = ["mock-llm"]
[project]
name = "branch-scope"
[models.m]
backend = "mock-llm"
model = "m"
[agents.triage]
display_name = "Triage"
package = "branch-scope@^0.1"
model = "m"
max_turns = 1
[agents.later]
display_name = "Later"
package = "branch-scope@^0.1"
model = "m"
max_turns = 1
[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.later.output", check = "non_empty" }
[[pipeline.steps.then]]
id = "arm"
run = "agent:triage"
input = "${input}"
[[pipeline.steps]]
id = "later"
run = "agent:later"
input = "${input}"
"#;
    let config = ProjectConfig::parse_str(toml).expect("parse config");
    let target = lookup_first_available();
    let caches = caches_with(vec![], vec![]);
    let err = lower_project(&config, &target, &caches).unwrap_err();
    assert!(
        matches!(err, LowerError::ConditionUnknownOutput { .. }),
        "expected ConditionUnknownOutput, got {err:?}"
    );
}
