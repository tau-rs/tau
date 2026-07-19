//! Third lowering stage: workflow-shape invariants.

use crate::error::LowerError;
use tau_ir::subflow::SubflowKind;
use tau_ir::tool_impl::ToolImpl;

use super::parse::Parsed;

/// Run the typecheck stage on a `Parsed` value.
pub(super) fn typecheck(parsed: &Parsed) -> Result<(), LowerError> {
    // 1. Each Agent::tool_refs entry must exist in `tools`.
    for (agent_id, agent) in parsed.workflow.agents.iter() {
        for tool_ref in agent.tool_refs.iter() {
            if !parsed.workflow.tools.contains_key(tool_ref) {
                return Err(LowerError::UnknownToolRef {
                    agent: agent_id.clone(),
                    tool: tool_ref.clone(),
                });
            }
        }
    }

    // 2. Each Subflow::Spawn must reference an existing agent.
    for edge in parsed.workflow.edges.iter() {
        match &edge.kind {
            SubflowKind::Spawn {
                target_agent,
                cap_subset: _,
            } => {
                if !parsed.workflow.agents.contains_key(target_agent) {
                    return Err(LowerError::UnknownSubflowTarget {
                        subflow: edge.id.clone(),
                        agent: target_agent.clone(),
                    });
                }
                // cap_subset's subset-of-parent check is deferred: the
                // PARENT agent (the one that contains this edge) is the
                // one whose grant we'd narrow. v0's tau.toml does not yet
                // express the parent linkage; the lowering pass treats
                // every edge as adjacent-to-every-agent. β.2.4
                // (interpreter) will enforce cap_subset ⊆ caller's grant
                // dynamically.
            }
            SubflowKind::Compose { .. } => {
                return Err(LowerError::UnsupportedComposeSubflow {
                    subflow: edge.id.clone(),
                });
            }
        }
    }

    // 3. Sanity: every Native tool's content_hash must be non-zero.
    //    (If it's still the resolve-stage sentinel, the native tool
    //    cache didn't know about it — this is the place to refuse.)
    for (tool_id, tool) in parsed.workflow.tools.iter() {
        if let ToolImpl::Native {
            fn_ref,
            content_hash,
        } = &tool.impl_
        {
            if content_hash == &[0u8; 32] {
                return Err(LowerError::UnknownNativeTool {
                    tool: tool_id.clone(),
                    fn_name: fn_ref.name.clone(),
                });
            }
        }
    }

    // 4+5. Every ToolImpl::Subflow's target must exist in `agents`;
    //      every ToolImpl::Step's id must exist in `steps`.
    //      Both checks iterate the same map — fold them into one pass.
    for (tool_id, tool) in parsed.workflow.tools.iter() {
        if let ToolImpl::Subflow { target } = &tool.impl_ {
            if !parsed.workflow.agents.contains_key(target) {
                return Err(LowerError::UnknownSubflowToolTarget {
                    tool: tool_id.clone(),
                    agent: target.clone(),
                });
            }
        }
        if let ToolImpl::Step { id } = &tool.impl_ {
            if !parsed.workflow.steps.contains_key(id) {
                return Err(LowerError::UnknownStepToolTarget {
                    tool: tool_id.clone(),
                    step: id.clone(),
                });
            }
        }
    }

    // 6. Each trigger's entrypoint agent must exist.
    for trigger in parsed.triggers.iter() {
        if !parsed.workflow.agents.contains_key(&trigger.agent) {
            return Err(LowerError::UnknownTriggerAgent {
                trigger: trigger.name.clone(),
                agent: trigger.agent.clone(),
            });
        }
    }

    // 7. Pipeline checks: run targets exist, no dup ids, no forward refs.
    check_pipeline(&parsed.workflow)?;

    // 8. Context-pipeline structural checks.
    check_context(&parsed.workflow)?;

    Ok(())
}

/// Structural validation of every agent's context pipeline.
///
/// Builtins: `trim_old`, `compact_tool_outputs`, `fit_budget`. Custom nodes
/// (`ContextNodeKind::Custom`) are accepted structurally here; their
/// capability grants are checked in tau-pkg. Rules:
/// - the last step must be the builtin `fit_budget` (guarantees a ceiling);
/// - no transformer name repeats;
/// - a `Builtin`-kind step must be a known builtin name.
fn check_context(wf: &tau_ir::module::Workflow) -> Result<(), LowerError> {
    use alloc::collections::BTreeSet;
    use tau_ir::context::ContextNodeKind;

    const BUILTINS: [&str; 3] = ["trim_old", "compact_tool_outputs", "fit_budget"];

    for (id, agent) in wf.agents.iter() {
        let Some(ctx) = &agent.context else { continue };
        if ctx.pipeline.is_empty() {
            continue;
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for step in &ctx.pipeline {
            if !seen.insert(step.transformer.as_str()) {
                return Err(crate::error::LowerError::DuplicateContextTransformer {
                    agent: id.0.clone(),
                    transformer: step.transformer.clone(),
                });
            }
            if matches!(step.kind, ContextNodeKind::Builtin)
                && !BUILTINS.contains(&step.transformer.as_str())
            {
                return Err(crate::error::LowerError::UnknownContextTransformer {
                    agent: id.0.clone(),
                    transformer: step.transformer.clone(),
                });
            }
        }
        let last = ctx.pipeline.last().expect("non-empty checked above");
        if last.transformer != "fit_budget" {
            return Err(crate::error::LowerError::ContextFitBudgetNotLast {
                agent: id.0.clone(),
                last: last.transformer.clone(),
            });
        }
    }
    Ok(())
}

fn check_pipeline(wf: &tau_ir::module::Workflow) -> Result<(), LowerError> {
    use alloc::collections::BTreeSet;
    use tau_ir::pipeline::StepRun;
    use tau_ir::template::{extract_refs, TemplateRef};

    let Some(pipeline) = &wf.pipeline else {
        return Ok(());
    };

    // Tree-wide id uniqueness: nested Branch/Parallel/Loop steps share the
    // same flat global namespace as top-level steps (EPIC 4.2 Decision 1),
    // so uniqueness must be checked across the whole tree up front.
    let mut all_ids_preorder = alloc::vec::Vec::new();
    collect_all_ids(&pipeline.steps, &mut all_ids_preorder);
    let mut all_ids: BTreeSet<&str> = BTreeSet::new();
    for id in &all_ids_preorder {
        if !all_ids.insert(*id) {
            return Err(LowerError::DuplicatePipelineStepId { id: (*id).into() });
        }
    }

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    // Nested descendants of PRIOR top-level steps are also visible to a
    // later top-level step's template refs (EPIC 4.2 Decision 1: the store
    // is flat and block-transparent, so a downstream step may read a
    // Branch-arm or Loop-body output by bare id). Populated at the END of
    // each iteration below, so at step `i` this holds exactly the subtree
    // ids of steps `[0..i)` — never the current step's or a later step's
    // nested descendants.
    let mut visible_from_prior: BTreeSet<&str> = BTreeSet::new();
    for step in &pipeline.steps {
        let sid = step.id.0.as_str();
        // Global uniqueness was already enforced above; this insert only
        // maintains the execution-order-visibility scope for output refs.
        seen_ids.insert(sid);

        // For Check steps, emit the more precise UnknownCheckRef error when
        // the check id is absent from workflow.checks.
        if let StepRun::Check(check_id) = &step.run {
            if !wf.checks.contains_key(check_id) {
                return Err(LowerError::UnknownCheckRef {
                    step: sid.into(),
                    check: check_id.0.clone(),
                });
            }
        }

        // Validate this step's run target and recurse into any nested steps.
        validate_step_run(&step.run, wf, sid, &all_ids, &seen_ids)?;

        // For Check steps, validate the check's locus integrity:
        // if the check has a Locus::Output referencing a step, that step must
        // appear strictly BEFORE the current check step in the pipeline.
        if let StepRun::Check(check_id) = &step.run {
            if let Some(check) = wf.checks.get(check_id) {
                use tau_ir::check::{CheckVerify, Locus};
                let locus = match &check.verify {
                    CheckVerify::Goal { evaluates, .. } => Some(evaluates),
                    CheckVerify::Deliverable { locus, .. } => Some(locus),
                };
                if let Some(Locus::Output(ref_step_id)) = locus {
                    // seen_ids holds exactly the steps strictly BEFORE current
                    // (current step's id was just inserted above, so we check
                    // using the pre-insertion state — but since we check
                    // whether the referenced id is in seen_ids BEFORE we
                    // process the current step's template refs, we need to
                    // check whether ref_step_id was inserted strictly before
                    // sid). seen_ids already has sid, so we check ≠ sid AND
                    // seen_ids contains the ref (modulo the current step).
                    let is_earlier =
                        ref_step_id.0 != sid && seen_ids.contains(ref_step_id.0.as_str());
                    if !is_earlier {
                        return Err(LowerError::UnknownCheckLocus {
                            check: check_id.0.clone(),
                            output: ref_step_id.0.clone(),
                        });
                    }
                }
            }
        }

        let refs = extract_refs(&step.input).map_err(|e| LowerError::BadPipelineTemplate {
            step: sid.into(),
            detail: alloc::format!("{e}"),
        })?;
        for r in refs {
            if let TemplateRef::StepOutput(ref_id) = r {
                // Existence is checked tree-wide (EPIC 4.2 Decision 1: nested
                // Branch/Loop/Parallel steps share the same flat global
                // namespace as top-level steps), not just among top-level ids.
                if !all_ids.contains(ref_id.as_str()) {
                    return Err(LowerError::UnknownOutputRef {
                        step: sid.into(),
                        referenced: ref_id,
                    });
                }
                // seen_ids currently holds exactly the top-level steps at-or-
                // before this one (the current step's id was just inserted
                // above). visible_from_prior holds the subtree ids of every
                // STRICTLY earlier top-level step (Branch arms, Loop bodies,
                // Parallel branches all included — a downstream step may
                // read a block's result by bare id per Decision 1). A
                // reference is valid iff it is a strictly earlier top-level
                // step OR a nested descendant of one. Guard self-reference
                // explicitly; the two `!contains` catches forward references
                // (later top-level steps, or nested descendants of the
                // current/later top-level steps).
                if ref_id == sid
                    || (!seen_ids.contains(ref_id.as_str())
                        && !visible_from_prior.contains(ref_id.as_str()))
                {
                    return Err(LowerError::ForwardOutputRef {
                        step: sid.into(),
                        referenced: ref_id,
                    });
                }
            }
        }

        // Make this step's whole subtree (including its own id, which
        // harmlessly overlaps seen_ids) visible to later top-level steps.
        let mut sub = alloc::vec::Vec::new();
        collect_all_ids(core::slice::from_ref(step), &mut sub);
        visible_from_prior.extend(sub);
    }
    Ok(())
}

/// Collect every `PipelineStepId` in the tree (top-level + nested inside
/// Branch/Parallel/Loop), in pre-order, so uniqueness spans the whole
/// pipeline (EPIC 4.2's flat global namespace, Decision 1).
fn collect_all_ids<'a>(
    steps: &'a [tau_ir::pipeline::PipelineStep],
    out: &mut alloc::vec::Vec<&'a str>,
) {
    use tau_ir::pipeline::StepRun;
    for s in steps {
        out.push(s.id.0.as_str());
        match &s.run {
            StepRun::Branch {
                then, otherwise, ..
            } => {
                collect_all_ids(then, out);
                collect_all_ids(otherwise, out);
            }
            StepRun::Parallel { branches } => {
                for b in branches {
                    collect_all_ids(b, out);
                }
            }
            StepRun::Loop { body, .. } => collect_all_ids(body, out),
            _ => {}
        }
    }
}

/// Ref-check one nested step's `input` template against the scope in force
/// at its execution site. Mirrors the top-level template loop in
/// `check_pipeline`: `StepOutput(ref_id)` must exist somewhere in the tree
/// (`all_ids`) — else `UnknownOutputRef` — and must be visible in the
/// caller-supplied `scope` (an earlier sibling or an outer step, never the
/// step itself) — else `ForwardOutputRef`.
fn check_nested_template_refs(
    sid: &str,
    input: &str,
    all_ids: &alloc::collections::BTreeSet<&str>,
    scope: &alloc::collections::BTreeSet<&str>,
) -> Result<(), LowerError> {
    use tau_ir::template::{extract_refs, TemplateRef};

    let refs = extract_refs(input).map_err(|e| LowerError::BadPipelineTemplate {
        step: sid.into(),
        detail: alloc::format!("{e}"),
    })?;
    for r in refs {
        if let TemplateRef::StepOutput(ref_id) = r {
            if !all_ids.contains(ref_id.as_str()) {
                return Err(LowerError::UnknownOutputRef {
                    step: sid.into(),
                    referenced: ref_id,
                });
            }
            if ref_id == sid || !scope.contains(ref_id.as_str()) {
                return Err(LowerError::ForwardOutputRef {
                    step: sid.into(),
                    referenced: ref_id,
                });
            }
        }
    }
    Ok(())
}

/// Validate a [`StepRun`]`s referenced nodes and recurse into nested steps.
///
/// `outer_step_id` is the id of the top-level pipeline step that owns this
/// run (used in error messages). `all_ids` is the flat global namespace of
/// every `PipelineStepId` in the tree (top-level + nested), used to
/// distinguish "unknown anywhere" from "known but out of scope" when
/// ref-checking nested `input` templates. `seen_ids` is the set of step ids
/// that are in scope at the point this run executes (used for
/// `Locus::Output` checks inside conditions, and as the starting scope for
/// nested control-flow blocks).
///
/// Scope rules (EPIC 4.2 Decision 1 — flat global namespace + execution-order
/// visibility):
/// - `Branch.on` sees only the outer scope (it runs *before* either arm).
/// - Each `Branch` arm (`then`/`otherwise`) sees the outer scope plus its own
///   prior siblings — never the other arm's ids (only one arm executes).
/// - `Loop.until` sees the outer scope *plus every body id* (it runs *after*
///   each body pass).
/// - `Loop` body steps see the outer scope plus their own prior siblings.
/// - Each `Parallel` branch sees the outer scope plus its own prior steps
///   only — never a sibling branch's ids (no ordering between branches).
fn validate_step_run(
    run: &tau_ir::pipeline::StepRun,
    wf: &tau_ir::module::Workflow,
    outer_step_id: &str,
    all_ids: &alloc::collections::BTreeSet<&str>,
    seen_ids: &alloc::collections::BTreeSet<&str>,
) -> Result<(), LowerError> {
    use tau_ir::check::Locus;
    use tau_ir::pipeline::StepRun;

    match run {
        StepRun::Agent(a) => {
            if !wf.agents.contains_key(a) {
                return Err(LowerError::UnknownPipelineRun {
                    step: outer_step_id.into(),
                    target: alloc::format!("agent:{}", a.0),
                });
            }
        }
        StepRun::Tool(t) => {
            if !wf.tools.contains_key(t) {
                return Err(LowerError::UnknownPipelineRun {
                    step: outer_step_id.into(),
                    target: alloc::format!("tool:{}", t.0),
                });
            }
        }
        StepRun::Deterministic(s) => {
            if !wf.steps.contains_key(s) {
                return Err(LowerError::UnknownPipelineRun {
                    step: outer_step_id.into(),
                    target: alloc::format!("deterministic:{}", s.0),
                });
            }
        }
        StepRun::Check(check_id) => {
            // For nested Check refs (inside Branch/Parallel/Loop bodies) the
            // outer check_pipeline guard never fires — validate here so that
            // a ghost check ref inside a control-flow block is caught.
            if !wf.checks.contains_key(check_id) {
                return Err(LowerError::UnknownCheckRef {
                    step: outer_step_id.into(),
                    check: check_id.0.clone(),
                });
            }
        }
        StepRun::Branch {
            on,
            then,
            otherwise,
        } => {
            // Validate the condition's locus if it references a step output.
            if let Locus::Output(ref_step_id) = &on.evaluates {
                let is_in_scope =
                    ref_step_id.0 != outer_step_id && seen_ids.contains(ref_step_id.0.as_str());
                if !is_in_scope {
                    return Err(LowerError::ConditionUnknownOutput {
                        step: outer_step_id.into(),
                        output: ref_step_id.0.clone(),
                    });
                }
            }
            // Each arm sees the outer scope plus its own prior siblings.
            // Arms are mutually exclusive (only one executes), so `then`
            // and `otherwise` never see each other's ids.
            for arm in [then, otherwise] {
                let mut arm_scope = seen_ids.clone();
                for nested in arm {
                    validate_step_run(&nested.run, wf, outer_step_id, all_ids, &arm_scope)?;
                    check_nested_template_refs(
                        nested.id.0.as_str(),
                        &nested.input,
                        all_ids,
                        &arm_scope,
                    )?;
                    arm_scope.insert(nested.id.0.as_str());
                }
            }
        }
        StepRun::Parallel { branches } => {
            // Each branch sees the outer scope plus its own prior steps —
            // never a sibling branch's ids (no ordering between branches).
            for branch in branches {
                let mut branch_scope = seen_ids.clone();
                for nested in branch {
                    validate_step_run(&nested.run, wf, outer_step_id, all_ids, &branch_scope)?;
                    check_nested_template_refs(
                        nested.id.0.as_str(),
                        &nested.input,
                        all_ids,
                        &branch_scope,
                    )?;
                    branch_scope.insert(nested.id.0.as_str());
                }
            }
        }
        StepRun::Loop {
            body,
            until,
            max_iters,
        } => {
            // Enforce the mandatory iteration bound.
            if *max_iters == 0 {
                return Err(LowerError::LoopMaxItersZero {
                    step: outer_step_id.into(),
                });
            }
            // Loop.until runs AFTER each body pass, so it may read body
            // outputs (the EPIC 4.1 deferred hole, closed here).
            let mut loop_scope: alloc::collections::BTreeSet<&str> = seen_ids.clone();
            for nested in body {
                loop_scope.insert(nested.id.0.as_str());
            }
            if let Locus::Output(ref_step_id) = &until.evaluates {
                let is_in_scope =
                    ref_step_id.0 != outer_step_id && loop_scope.contains(ref_step_id.0.as_str());
                if !is_in_scope {
                    return Err(LowerError::ConditionUnknownOutput {
                        step: outer_step_id.into(),
                        output: ref_step_id.0.clone(),
                    });
                }
            }
            // Body steps see the outer scope plus their own prior siblings.
            let mut body_scope = seen_ids.clone();
            for nested in body {
                validate_step_run(&nested.run, wf, outer_step_id, all_ids, &body_scope)?;
                check_nested_template_refs(
                    nested.id.0.as_str(),
                    &nested.input,
                    all_ids,
                    &body_scope,
                )?;
                body_scope.insert(nested.id.0.as_str());
            }
        }
        StepRun::Suspend { .. } => {
            // No refs to validate.
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::parse::Parsed;
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::vec;
    use tau_ir::capability::{CapabilityRequirements, CapabilityTable};
    use tau_ir::ids::{AgentId, StepId, ToolId};
    use tau_ir::module::Workflow;
    use tau_ir::node::{Agent, Tool, ToolSpec};
    use tau_ir::tool_impl::ToolImpl;
    use tau_ir::AgentBudget;

    fn empty_caps() -> CapabilityRequirements {
        CapabilityRequirements { declared: vec![] }
    }

    fn agent_with_tool_refs(id: &str, refs: &[&str]) -> Agent {
        Agent {
            id: AgentId(id.to_string()),
            prompt: String::new(),
            model_ref: tau_ir::model_ref::ModelRef {
                backend: String::new(),
                model_id: String::new(),
            },
            tool_refs: refs.iter().map(|s| ToolId(s.to_string())).collect(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        }
    }

    fn tool_with_impl(name: &str, impl_: ToolImpl) -> Tool {
        Tool {
            id: ToolId(name.to_string()),
            impl_,
            capabilities: empty_caps(),
            spec: ToolSpec {
                name: name.to_string(),
                description: String::new(),
                input_schema: serde_json::Value::Null,
            },
        }
    }

    #[test]
    fn rejects_unknown_run_target() {
        let toml = r#"
            [project]
            name = "demo"
            [[pipeline.steps]]
            id = "a"
            run = "agent:ghost"
        "#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&parsed).unwrap_err();
        assert!(
            matches!(err, LowerError::UnknownPipelineRun { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_forward_output_reference() {
        let toml = r#"
            packages = ["mock-llm"]

            [project]
            name = "demo"

            [models]
            default = { backend = "mock-llm", model = "mock-model" }

            [agents.a]
            display_name = "A"
            package      = "demo@^0.1"
            model        = "default"
            tool_refs    = []

            [agents.b]
            display_name = "B"
            package      = "demo@^0.1"
            model        = "default"
            tool_refs    = []

            [[pipeline.steps]]
            id = "a"
            run = "agent:a"
            input = "${steps.b.output}"

            [[pipeline.steps]]
            id = "b"
            run = "agent:b"
        "#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&parsed).unwrap_err();
        assert!(
            matches!(err, LowerError::ForwardOutputRef { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn accepts_valid_backward_reference() {
        let toml = r#"
            packages = ["mock-llm"]

            [project]
            name = "demo"

            [models]
            default = { backend = "mock-llm", model = "mock-model" }

            [agents.a]
            display_name = "A"
            package      = "demo@^0.1"
            model        = "default"
            tool_refs    = []

            [agents.b]
            display_name = "B"
            package      = "demo@^0.1"
            model        = "default"
            tool_refs    = []

            [[pipeline.steps]]
            id = "a"
            run = "agent:a"

            [[pipeline.steps]]
            id = "b"
            run = "agent:b"
            input = "${steps.a.output}"
        "#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let parsed = crate::lower::parse::parse(&cfg).unwrap();
        assert!(
            typecheck(&parsed).is_ok(),
            "valid backward reference should be accepted"
        );
    }

    #[test]
    fn typecheck_rejects_subflow_tool_pointing_at_missing_agent() {
        let mut agents = BTreeMap::new();
        // Only `parent` exists — subflow points at `ghost`.
        agents.insert(
            AgentId("parent".to_string()),
            agent_with_tool_refs("parent", &["call_ghost"]),
        );
        let mut tools = BTreeMap::new();
        tools.insert(
            ToolId("call_ghost".to_string()),
            tool_with_impl(
                "call_ghost",
                ToolImpl::Subflow {
                    target: AgentId("ghost".to_string()),
                },
            ),
        );
        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, LowerError::UnknownSubflowToolTarget { ref tool, ref agent }
                if tool.0 == "call_ghost" && agent.0 == "ghost"),
            "expected UnknownSubflowToolTarget; got {err:?}"
        );
    }

    #[test]
    fn check_step_with_unknown_check_id_is_rejected() {
        // A pipeline step runs StepRun::Check("ghost") but workflow.checks
        // has no entry for "ghost" → should return UnknownCheckRef.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::ids::{CheckId, PipelineStepId};
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let check_id = CheckId("ghost".to_string());
        // No entry in workflow.checks for "ghost".
        let parsed = Parsed {
            workflow: Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("step-a".to_string()),
                        run: StepRun::Check(check_id),
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(), // empty — "ghost" is not here
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("should reject unknown check ref");
        assert!(
            matches!(err, LowerError::UnknownCheckRef { ref step, ref check }
                if step == "step-a" && check == "ghost"),
            "expected UnknownCheckRef; got {err:?}"
        );
    }

    #[test]
    fn check_locus_output_referencing_later_step_is_rejected() {
        // A check whose Locus::Output names a step that comes AFTER the check
        // step in the pipeline → should return UnknownCheckLocus.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Check, CheckVerify, GoalPredicate, Locus, OnFail, RetryPolicy};
        use tau_ir::ids::{CheckId, PipelineStepId};
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let check_id = CheckId("my-check".to_string());
        // The check evaluates Locus::Output("later") but "later" runs after
        // the check step in the pipeline.
        let check = Check {
            id: check_id.clone(),
            verify: CheckVerify::Goal {
                evaluates: Locus::Output(PipelineStepId("later".to_string())),
                predicate: GoalPredicate::Exists,
            },
            retry: RetryPolicy {
                on_fail: OnFail::Abort,
                max_attempts: 1,
                gate: PipelineStepId("check-step".to_string()),
            },
        };

        let mut checks = BTreeMap::new();
        checks.insert(check_id.clone(), check);

        let parsed = Parsed {
            workflow: Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![
                        // check-step runs first, but it references "later" which is after
                        PipelineStep {
                            id: PipelineStepId("check-step".to_string()),
                            run: StepRun::Check(check_id),
                            input: "${input}".to_string(),
                        },
                        // "later" comes after the check step — invalid forward reference
                        PipelineStep {
                            id: PipelineStepId("later".to_string()),
                            run: StepRun::Tool(tau_ir::ids::ToolId("some-tool".to_string())),
                            input: "${input}".to_string(),
                        },
                    ],
                }),
                checks,
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("should reject forward-referencing check locus");
        assert!(
            matches!(err, LowerError::UnknownCheckLocus { ref check, ref output }
                if check == "my-check" && output == "later"),
            "expected UnknownCheckLocus; got {err:?}"
        );
    }

    // ── check_context tests ──────────────────────────────────────────────────

    use tau_ir::context::{ContextConfig, ContextNodeKind, ContextStep, DeterminismClass};

    fn agent_with_context(steps: alloc::vec::Vec<&str>) -> tau_ir::module::Workflow {
        let pipeline = steps
            .into_iter()
            .map(|t| ContextStep {
                transformer: t.into(),
                determinism: DeterminismClass::Pure,
                kind: ContextNodeKind::Builtin,
                config: Default::default(),
            })
            .collect();
        let mut wf = tau_ir::module::Workflow::default();
        // `ContextConfig` is `#[non_exhaustive]`; build via `Default` + the
        // public `pipeline` field rather than a struct literal.
        let mut ctx = ContextConfig::default();
        ctx.pipeline = pipeline;
        wf.agents.insert(
            AgentId("a".into()),
            Agent {
                id: AgentId("a".into()),
                prompt: "p".into(),
                model_ref: tau_ir::model_ref::ModelRef {
                    backend: "b".into(),
                    model_id: "m".into(),
                },
                tool_refs: alloc::vec![],
                context: Some(ctx),
                budget: tau_ir::AgentBudget {
                    max_turns: None,
                    max_tokens: None,
                },
                produces: alloc::vec![],
                output_schema: None,
                durable: None,
            },
        );
        wf
    }

    #[test]
    fn context_ok_when_fit_budget_last() {
        let wf = agent_with_context(alloc::vec!["trim_old", "fit_budget"]);
        assert!(check_context(&wf).is_ok());
    }

    #[test]
    fn context_rejects_unknown_transformer() {
        let wf = agent_with_context(alloc::vec!["bogus", "fit_budget"]);
        assert!(matches!(
            check_context(&wf),
            Err(crate::error::LowerError::UnknownContextTransformer { .. })
        ));
    }

    #[test]
    fn context_rejects_fit_budget_not_last() {
        let wf = agent_with_context(alloc::vec!["fit_budget", "trim_old"]);
        assert!(matches!(
            check_context(&wf),
            Err(crate::error::LowerError::ContextFitBudgetNotLast { .. })
        ));
    }

    #[test]
    fn context_rejects_duplicate() {
        let wf = agent_with_context(alloc::vec!["trim_old", "trim_old", "fit_budget"]);
        assert!(matches!(
            check_context(&wf),
            Err(crate::error::LowerError::DuplicateContextTransformer { .. })
        ));
    }

    // ── EPIC 4.1 control-flow typecheck tests ────────────────────────────────

    #[test]
    fn branch_nested_step_with_unknown_agent_fails_typecheck() {
        // A pipeline step is a Branch whose `then` contains StepRun::Agent("ghost")
        // not in workflow.agents → typecheck must reject (nested refs validated).
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let parsed = Parsed {
            workflow: Workflow {
                agents: BTreeMap::new(), // no agents at all
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("cf-step".to_string()),
                        run: StepRun::Branch {
                            on: Condition {
                                evaluates: Locus::Path("/some/path".to_string()),
                                predicate: GoalPredicate::Exists,
                            },
                            then: alloc::vec![PipelineStep {
                                id: PipelineStepId("nested-then".to_string()),
                                run: StepRun::Agent(tau_ir::ids::AgentId("ghost".to_string())),
                                input: "${input}".to_string(),
                            }],
                            otherwise: alloc::vec![],
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("should reject unknown nested agent");
        assert!(
            matches!(err, LowerError::UnknownPipelineRun { ref target, .. }
                if target.contains("ghost")),
            "expected UnknownPipelineRun mentioning ghost; got {err:?}"
        );
    }

    #[test]
    fn branch_nested_unknown_check_fails_typecheck() {
        // A Branch's `then` contains StepRun::Check("ghost-check") but
        // "ghost-check" is NOT in workflow.checks → typecheck must reject
        // with UnknownCheckRef.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::{CheckId, PipelineStepId};
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let parsed = Parsed {
            workflow: Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("cf-step".to_string()),
                        run: StepRun::Branch {
                            on: Condition {
                                evaluates: Locus::Path("/some/path".to_string()),
                                predicate: GoalPredicate::Exists,
                            },
                            then: alloc::vec![PipelineStep {
                                id: PipelineStepId("nested-check".to_string()),
                                run: StepRun::Check(CheckId("ghost-check".to_string())),
                                input: "${input}".to_string(),
                            }],
                            otherwise: alloc::vec![],
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(), // "ghost-check" absent
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("should reject nested unknown check ref");
        assert!(
            matches!(err, LowerError::UnknownCheckRef { ref check, .. }
                if check == "ghost-check"),
            "expected UnknownCheckRef mentioning ghost-check; got {err:?}"
        );
    }

    #[test]
    fn loop_max_iters_zero_fails_typecheck() {
        // StepRun::Loop { max_iters: 0, .. } → typecheck must reject.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let parsed = Parsed {
            workflow: Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("loop-step".to_string()),
                        run: StepRun::Loop {
                            body: alloc::vec![],
                            until: Condition {
                                evaluates: Locus::Path("/done".to_string()),
                                predicate: GoalPredicate::Exists,
                            },
                            max_iters: 0, // invalid
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("should reject max_iters == 0");
        assert!(
            matches!(err, LowerError::LoopMaxItersZero { ref step }
                if step == "loop-step"),
            "expected LoopMaxItersZero; got {err:?}"
        );
    }

    #[test]
    fn branch_with_valid_nested_steps_typechecks() {
        // Branch whose then/otherwise reference existing agents → OK.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("writer".to_string()),
            agent_with_tool_refs("writer", &[]),
        );
        agents.insert(
            AgentId("reviewer".to_string()),
            agent_with_tool_refs("reviewer", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("branch-step".to_string()),
                        run: StepRun::Branch {
                            on: Condition {
                                evaluates: Locus::Path("/flag".to_string()),
                                predicate: GoalPredicate::Exists,
                            },
                            then: alloc::vec![PipelineStep {
                                id: PipelineStepId("then-step".to_string()),
                                run: StepRun::Agent(AgentId("writer".to_string())),
                                input: "${input}".to_string(),
                            }],
                            otherwise: alloc::vec![PipelineStep {
                                id: PipelineStepId("else-step".to_string()),
                                run: StepRun::Agent(AgentId("reviewer".to_string())),
                                input: "${input}".to_string(),
                            }],
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        assert!(
            typecheck(&parsed).is_ok(),
            "valid branch with existing agents should typecheck"
        );
    }

    // ── EPIC 4.2 flat-global nested-scope tests ──────────────────────────────

    #[test]
    fn loop_until_reading_body_output_typechecks() {
        // Loop { body: [ improve = Agent(editor) ], until: Output(improve)
        // Matches "APPROVED", max_iters: 3 }
        // Was rejected pre-4.2 (nested id not in scope); now must ACCEPT.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("editor".to_string()),
            agent_with_tool_refs("editor", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("loop-step".to_string()),
                        run: StepRun::Loop {
                            body: alloc::vec![PipelineStep {
                                id: PipelineStepId("improve".to_string()),
                                run: StepRun::Agent(AgentId("editor".to_string())),
                                input: "${input}".to_string(),
                            }],
                            until: Condition {
                                evaluates: Locus::Output(PipelineStepId("improve".to_string())),
                                predicate: GoalPredicate::Matches("APPROVED".to_string()),
                            },
                            max_iters: 3,
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        typecheck(&parsed).expect("Loop.until may read its own body output");
    }

    #[test]
    fn branch_on_reading_own_then_body_output_rejected() {
        // Branch { on: Output(inner) NonEmpty, then: [ inner = Agent(x) ],
        // otherwise: [] }
        // Branch.on runs BEFORE then, so this must REJECT (ConditionUnknownOutput).
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(AgentId("x".to_string()), agent_with_tool_refs("x", &[]));

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("branch-step".to_string()),
                        run: StepRun::Branch {
                            on: Condition {
                                evaluates: Locus::Output(PipelineStepId("inner".to_string())),
                                predicate: GoalPredicate::NonEmpty,
                            },
                            then: alloc::vec![PipelineStep {
                                id: PipelineStepId("inner".to_string()),
                                run: StepRun::Agent(AgentId("x".to_string())),
                                input: "${input}".to_string(),
                            }],
                            otherwise: alloc::vec![],
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("Branch.on cannot read its own then-body");
        assert!(
            matches!(err, LowerError::ConditionUnknownOutput { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parallel_cross_branch_reference_rejected() {
        // Parallel { branches: [ [a = Agent], [b = Agent input
        // "${steps.a.output}"] ] }
        // branch 1 reads branch 0's output — no ordering between them → REJECT.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("agent-a".to_string()),
            agent_with_tool_refs("agent-a", &[]),
        );
        agents.insert(
            AgentId("agent-b".to_string()),
            agent_with_tool_refs("agent-b", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("parallel-step".to_string()),
                        run: StepRun::Parallel {
                            branches: alloc::vec![
                                alloc::vec![PipelineStep {
                                    id: PipelineStepId("a".to_string()),
                                    run: StepRun::Agent(AgentId("agent-a".to_string())),
                                    input: "${input}".to_string(),
                                }],
                                alloc::vec![PipelineStep {
                                    id: PipelineStepId("b".to_string()),
                                    run: StepRun::Agent(AgentId("agent-b".to_string())),
                                    input: "${steps.a.output}".to_string(),
                                }],
                            ],
                        },
                        input: "${input}".to_string(),
                    }],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("no cross-branch refs");
        assert!(
            matches!(
                err,
                LowerError::UnknownOutputRef { .. } | LowerError::ForwardOutputRef { .. }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn top_level_step_reads_loop_body_output_typechecks() {
        // seed = Agent, refine = Loop { body: [improve = Agent], until:
        // Output(improve) NonEmpty, max_iters: 2 }, tail = Agent input
        // "${steps.improve.output}".
        // A TOP-LEVEL step reading a Loop-body output by bare id must
        // ACCEPT (ADR-0059 Decision 1 — loop bodies always run ≥1x, so this
        // is always safe once the loop step itself resolves).
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("seed".to_string()),
            agent_with_tool_refs("seed", &[]),
        );
        agents.insert(
            AgentId("improve".to_string()),
            agent_with_tool_refs("improve", &[]),
        );
        agents.insert(
            AgentId("tail".to_string()),
            agent_with_tool_refs("tail", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![
                        PipelineStep {
                            id: PipelineStepId("seed".to_string()),
                            run: StepRun::Agent(AgentId("seed".to_string())),
                            input: "${input}".to_string(),
                        },
                        PipelineStep {
                            id: PipelineStepId("refine".to_string()),
                            run: StepRun::Loop {
                                body: alloc::vec![PipelineStep {
                                    id: PipelineStepId("improve".to_string()),
                                    run: StepRun::Agent(AgentId("improve".to_string())),
                                    input: "${steps.seed.output}".to_string(),
                                }],
                                until: Condition {
                                    evaluates: Locus::Output(
                                        PipelineStepId("improve".to_string(),)
                                    ),
                                    predicate: GoalPredicate::NonEmpty,
                                },
                                max_iters: 2,
                            },
                            input: "${input}".to_string(),
                        },
                        PipelineStep {
                            id: PipelineStepId("tail".to_string()),
                            run: StepRun::Agent(AgentId("tail".to_string())),
                            input: "${steps.improve.output}".to_string(),
                        },
                    ],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        typecheck(&parsed).expect("top-level may read a loop-body output");
    }

    #[test]
    fn top_level_step_reads_branch_arm_output_typechecks() {
        // seed = Agent, gate = Branch { on: valid, then: [inner = Agent],
        // otherwise: [] }, tail = Agent input "${steps.inner.output}".
        // A TOP-LEVEL step reading a Branch-arm output by bare id must
        // ACCEPT at typecheck (ADR-0059 Decision 1 + Known limitation: this
        // is admitted here and only hard-errors at RUNTIME if the arm that
        // defines `inner` did not execute — mirrors the interpreter e2e
        // test `downstream_reads_block_output_by_bare_id`).
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("seed".to_string()),
            agent_with_tool_refs("seed", &[]),
        );
        agents.insert(
            AgentId("inner".to_string()),
            agent_with_tool_refs("inner", &[]),
        );
        agents.insert(
            AgentId("tail".to_string()),
            agent_with_tool_refs("tail", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![
                        PipelineStep {
                            id: PipelineStepId("seed".to_string()),
                            run: StepRun::Agent(AgentId("seed".to_string())),
                            input: "${input}".to_string(),
                        },
                        PipelineStep {
                            id: PipelineStepId("gate".to_string()),
                            run: StepRun::Branch {
                                on: Condition {
                                    evaluates: Locus::Path("/flag".to_string()),
                                    predicate: GoalPredicate::Exists,
                                },
                                then: alloc::vec![PipelineStep {
                                    id: PipelineStepId("inner".to_string()),
                                    run: StepRun::Agent(AgentId("inner".to_string())),
                                    input: "${steps.seed.output}".to_string(),
                                }],
                                otherwise: alloc::vec![],
                            },
                            input: "${input}".to_string(),
                        },
                        PipelineStep {
                            id: PipelineStepId("tail".to_string()),
                            run: StepRun::Agent(AgentId("tail".to_string())),
                            input: "${steps.inner.output}".to_string(),
                        },
                    ],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        typecheck(&parsed).expect("top-level may read a branch-arm output");
    }

    #[test]
    fn top_level_forward_ref_to_later_nested_rejected() {
        // tail = Agent input "${steps.inner.output}" comes BEFORE gate =
        // Branch { then: [inner = Agent], otherwise: [] } — "inner" lives in
        // a LATER top-level block, so this must still REJECT. Proves the
        // Decision-1 fix (admit PRIOR-block nested descendants) did not
        // loosen forward-reference ordering.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("tail".to_string()),
            agent_with_tool_refs("tail", &[]),
        );
        agents.insert(
            AgentId("inner".to_string()),
            agent_with_tool_refs("inner", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![
                        PipelineStep {
                            id: PipelineStepId("tail".to_string()),
                            run: StepRun::Agent(AgentId("tail".to_string())),
                            input: "${steps.inner.output}".to_string(),
                        },
                        PipelineStep {
                            id: PipelineStepId("gate".to_string()),
                            run: StepRun::Branch {
                                on: Condition {
                                    evaluates: Locus::Path("/flag".to_string()),
                                    predicate: GoalPredicate::Exists,
                                },
                                then: alloc::vec![PipelineStep {
                                    id: PipelineStepId("inner".to_string()),
                                    run: StepRun::Agent(AgentId("inner".to_string())),
                                    input: "${input}".to_string(),
                                }],
                                otherwise: alloc::vec![],
                            },
                            input: "${input}".to_string(),
                        },
                    ],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed)
            .expect_err("top-level ref to a LATER block's nested id must still be rejected");
        assert!(
            matches!(
                err,
                LowerError::ForwardOutputRef { .. } | LowerError::UnknownOutputRef { .. }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn duplicate_nested_step_id_rejected() {
        // top-level id "improve" AND a loop body id "improve" →
        // DuplicatePipelineStepId.
        use tau_ir::capability::CapabilityTable;
        use tau_ir::check::{Condition, GoalPredicate, Locus};
        use tau_ir::ids::PipelineStepId;
        use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};

        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("editor".to_string()),
            agent_with_tool_refs("editor", &[]),
        );

        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: Some(Pipeline {
                    steps: alloc::vec![
                        PipelineStep {
                            id: PipelineStepId("improve".to_string()),
                            run: StepRun::Agent(AgentId("editor".to_string())),
                            input: "${input}".to_string(),
                        },
                        PipelineStep {
                            id: PipelineStepId("loop-step".to_string()),
                            run: StepRun::Loop {
                                body: alloc::vec![PipelineStep {
                                    id: PipelineStepId("improve".to_string()),
                                    run: StepRun::Agent(AgentId("editor".to_string())),
                                    input: "${input}".to_string(),
                                }],
                                until: Condition {
                                    evaluates: Locus::Path("/done".to_string()),
                                    predicate: GoalPredicate::Exists,
                                },
                                max_iters: 3,
                            },
                            input: "${input}".to_string(),
                        },
                    ],
                }),
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("nested ids share the global namespace");
        assert!(
            matches!(err, LowerError::DuplicatePipelineStepId { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn typecheck_rejects_step_tool_pointing_at_missing_step() {
        let mut agents = BTreeMap::new();
        agents.insert(
            AgentId("solo".to_string()),
            agent_with_tool_refs("solo", &["normalize"]),
        );
        let mut tools = BTreeMap::new();
        tools.insert(
            ToolId("normalize".to_string()),
            tool_with_impl(
                "normalize",
                ToolImpl::Step {
                    id: StepId("missing-step".to_string()),
                },
            ),
        );
        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(), // empty → "missing-step" not present
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, LowerError::UnknownStepToolTarget { ref tool, ref step }
                if tool.0 == "normalize" && step.0 == "missing-step"),
            "expected UnknownStepToolTarget; got {err:?}"
        );
    }
}
