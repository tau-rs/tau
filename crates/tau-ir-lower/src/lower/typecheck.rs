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

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    for step in &pipeline.steps {
        let sid = step.id.0.as_str();
        if !seen_ids.insert(sid) {
            return Err(LowerError::DuplicatePipelineStepId { id: sid.into() });
        }

        let exists = match &step.run {
            StepRun::Agent(a) => wf.agents.contains_key(a),
            StepRun::Tool(t) => wf.tools.contains_key(t),
            StepRun::Deterministic(s) => wf.steps.contains_key(s),
            StepRun::Check(c) => wf.checks.contains_key(c),
        };
        if !exists {
            // For Check steps, emit the more precise UnknownCheckRef error.
            if let StepRun::Check(check_id) = &step.run {
                return Err(LowerError::UnknownCheckRef {
                    step: sid.into(),
                    check: check_id.0.clone(),
                });
            }
            let target = match &step.run {
                StepRun::Agent(a) => alloc::format!("agent:{}", a.0),
                StepRun::Tool(t) => alloc::format!("tool:{}", t.0),
                StepRun::Deterministic(s) => alloc::format!("deterministic:{}", s.0),
                StepRun::Check(c) => alloc::format!("check:{}", c.0),
            };
            return Err(LowerError::UnknownPipelineRun {
                step: sid.into(),
                target,
            });
        }

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
                let exists_anywhere = pipeline.steps.iter().any(|s| s.id.0 == ref_id);
                if !exists_anywhere {
                    return Err(LowerError::UnknownOutputRef {
                        step: sid.into(),
                        referenced: ref_id,
                    });
                }
                // seen_ids currently holds exactly the steps at-or-before this one
                // (the current step's id was just inserted above).
                // A reference is valid only to a STRICTLY earlier step.
                // Guard self-reference explicitly; !seen_ids.contains catches
                // forward references (later steps not yet inserted).
                if ref_id == sid || !seen_ids.contains(ref_id.as_str()) {
                    return Err(LowerError::ForwardOutputRef {
                        step: sid.into(),
                        referenced: ref_id,
                    });
                }
            }
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
