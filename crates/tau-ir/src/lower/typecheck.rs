//! Third lowering stage: workflow-shape invariants.

use crate::error::IrError;
use crate::subflow::SubflowKind;
use crate::tool_impl::ToolImpl;

use super::parse::Parsed;

/// Run the typecheck stage on a `Parsed` value.
pub(super) fn typecheck(parsed: &Parsed) -> Result<(), IrError> {
    // 1. Each Agent::tool_refs entry must exist in `tools`.
    for (agent_id, agent) in parsed.workflow.agents.iter() {
        for tool_ref in agent.tool_refs.iter() {
            if !parsed.workflow.tools.contains_key(tool_ref) {
                return Err(IrError::UnknownToolRef {
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
                    return Err(IrError::UnknownSubflowTarget {
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
                return Err(IrError::UnsupportedComposeSubflow {
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
                return Err(IrError::UnknownNativeTool {
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
                return Err(IrError::UnknownSubflowToolTarget {
                    tool: tool_id.clone(),
                    agent: target.clone(),
                });
            }
        }
        if let ToolImpl::Step { id } = &tool.impl_ {
            if !parsed.workflow.steps.contains_key(id) {
                return Err(IrError::UnknownStepToolTarget {
                    tool: tool_id.clone(),
                    step: id.clone(),
                });
            }
        }
    }

    // 6. Pipeline checks: run targets exist, no dup ids, no forward refs.
    check_pipeline(&parsed.workflow)?;

    Ok(())
}

fn check_pipeline(wf: &crate::module::Workflow) -> Result<(), IrError> {
    use crate::pipeline::StepRun;
    use crate::template::{extract_refs, TemplateRef};
    use alloc::collections::BTreeSet;

    let Some(pipeline) = &wf.pipeline else {
        return Ok(());
    };

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    for step in &pipeline.steps {
        let sid = step.id.0.as_str();
        if !seen_ids.insert(sid) {
            return Err(IrError::DuplicatePipelineStepId { id: sid.into() });
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
                return Err(IrError::UnknownCheckRef {
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
            return Err(IrError::UnknownPipelineRun {
                step: sid.into(),
                target,
            });
        }

        // For Check steps, validate the check's locus integrity:
        // if the check has a Locus::Output referencing a step, that step must
        // appear strictly BEFORE the current check step in the pipeline.
        if let StepRun::Check(check_id) = &step.run {
            if let Some(check) = wf.checks.get(check_id) {
                use crate::check::{CheckVerify, Locus};
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
                        return Err(IrError::UnknownCheckLocus {
                            check: check_id.0.clone(),
                            output: ref_step_id.0.clone(),
                        });
                    }
                }
            }
        }

        let refs = extract_refs(&step.input).map_err(|e| IrError::BadPipelineTemplate {
            step: sid.into(),
            detail: alloc::format!("{e}"),
        })?;
        for r in refs {
            if let TemplateRef::StepOutput(ref_id) = r {
                let exists_anywhere = pipeline.steps.iter().any(|s| s.id.0 == ref_id);
                if !exists_anywhere {
                    return Err(IrError::UnknownOutputRef {
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
                    return Err(IrError::ForwardOutputRef {
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
    use crate::capability::{CapabilityRequirements, CapabilityTable};
    use crate::ids::{AgentId, StepId, ToolId};
    use crate::lower::parse::Parsed;
    use crate::module::Workflow;
    use crate::node::{Agent, Tool, ToolSpec};
    use crate::tool_impl::ToolImpl;
    use crate::AgentBudget;
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::vec;

    fn empty_caps() -> CapabilityRequirements {
        CapabilityRequirements { declared: vec![] }
    }

    fn agent_with_tool_refs(id: &str, refs: &[&str]) -> Agent {
        Agent {
            id: AgentId(id.to_string()),
            prompt: String::new(),
            model: String::new(),
            tool_refs: refs.iter().map(|s| ToolId(s.to_string())).collect(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
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
            matches!(err, IrError::UnknownPipelineRun { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_forward_output_reference() {
        let toml = r#"
            [project]
            name = "demo"

            [agents.a]
            display_name = "A"
            package      = "demo@^0.1"
            llm_backend  = "mock-llm"
            tool_refs    = []

            [agents.b]
            display_name = "B"
            package      = "demo@^0.1"
            llm_backend  = "mock-llm"
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
            matches!(err, IrError::ForwardOutputRef { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn accepts_valid_backward_reference() {
        let toml = r#"
            [project]
            name = "demo"

            [agents.a]
            display_name = "A"
            package      = "demo@^0.1"
            llm_backend  = "mock-llm"
            tool_refs    = []

            [agents.b]
            display_name = "B"
            package      = "demo@^0.1"
            llm_backend  = "mock-llm"
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
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownSubflowToolTarget { ref tool, ref agent }
                if tool.0 == "call_ghost" && agent.0 == "ghost"),
            "expected UnknownSubflowToolTarget; got {err:?}"
        );
    }

    #[test]
    fn check_step_with_unknown_check_id_is_rejected() {
        // A pipeline step runs StepRun::Check("ghost") but workflow.checks
        // has no entry for "ghost" → should return UnknownCheckRef.
        use crate::capability::{CapabilityRequirements, CapabilityTable};
        use crate::check::{Check, CheckVerify, GoalPredicate, Locus, OnFail, RetryPolicy};
        use crate::ids::{CheckId, PipelineStepId};
        use crate::pipeline::{Pipeline, PipelineStep, StepRun};

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
        };
        let err = typecheck(&parsed).expect_err("should reject unknown check ref");
        assert!(
            matches!(err, IrError::UnknownCheckRef { ref step, ref check }
                if step == "step-a" && check == "ghost"),
            "expected UnknownCheckRef; got {err:?}"
        );
    }

    #[test]
    fn check_locus_output_referencing_later_step_is_rejected() {
        // A check whose Locus::Output names a step that comes AFTER the check
        // step in the pipeline → should return UnknownCheckLocus.
        use crate::capability::{CapabilityRequirements, CapabilityTable};
        use crate::check::{Check, CheckVerify, GoalPredicate, Locus, OnFail, RetryPolicy};
        use crate::ids::{CheckId, PipelineStepId};
        use crate::pipeline::{Pipeline, PipelineStep, StepRun};

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
                            run: StepRun::Tool(crate::ids::ToolId("some-tool".to_string())),
                            input: "${input}".to_string(),
                        },
                    ],
                }),
                checks,
            },
        };
        let err = typecheck(&parsed).expect_err("should reject forward-referencing check locus");
        assert!(
            matches!(err, IrError::UnknownCheckLocus { ref check, ref output }
                if check == "my-check" && output == "later"),
            "expected UnknownCheckLocus; got {err:?}"
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
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownStepToolTarget { ref tool, ref step }
                if tool.0 == "normalize" && step.0 == "missing-step"),
            "expected UnknownStepToolTarget; got {err:?}"
        );
    }
}
