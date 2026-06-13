//! Third lowering stage: workflow-shape invariants.

use crate::check::{CheckVerify, JudgeRef, Locus, OnFail, Retry};
use crate::error::IrError;
use crate::ids::{CheckId, PipelineStepId};
use crate::pipeline::StepRun;
use crate::subflow::SubflowKind;
use crate::tool_impl::ToolImpl;

use super::parse::Parsed;

/// Run the typecheck stage on a `Parsed` value.
///
/// Mutates `parsed` to write resolved `Retry` bindings into
/// `workflow.checks` after producer / gate analysis.
pub(super) fn typecheck(parsed: &mut Parsed) -> Result<(), IrError> {
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

    // 6. Pipeline checks: run targets exist, no dup ids, no forward refs,
    //    and Check loci reference only earlier pipeline steps.
    check_pipeline(&parsed.workflow)?;

    // 7. Resolve retry intents, enforce guarantees G1+G2, D7 overlap,
    //    and verify custom judge agents exist.
    resolve_retries_and_guarantees(parsed)?;

    Ok(())
}

fn check_pipeline(wf: &crate::module::Workflow) -> Result<(), IrError> {
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

        // For a Check step: if its locus is Output(ref_step), verify that
        // ref_step is a strictly-earlier pipeline step (seen_ids holds all
        // steps visited so far, NOT including the current one yet — but we
        // inserted sid above before this block, so we need to check strictly
        // before the current step, i.e., seen_ids contains the step AND it
        // is not the current sid).
        if let StepRun::Check(c) = &step.run {
            // exists is already verified above; retrieve the check.
            if let Some(check) = wf.checks.get(c) {
                let locus_step_id: Option<&str> = match &check.verify {
                    crate::check::CheckVerify::Goal { evaluates, .. } => {
                        if let Locus::Output(ref s) = evaluates {
                            Some(s.0.as_str())
                        } else {
                            None
                        }
                    }
                    crate::check::CheckVerify::Deliverable { locus, .. } => {
                        if let Locus::Output(ref s) = locus {
                            Some(s.0.as_str())
                        } else {
                            None
                        }
                    }
                };
                if let Some(ref_step) = locus_step_id {
                    // Must be a strictly-earlier pipeline step (already in seen_ids
                    // before the current sid was inserted, i.e., seen_ids contains
                    // it but it is not sid itself — self-reference also fails).
                    // seen_ids now contains sid (inserted above), so exclude it.
                    let is_earlier = ref_step != sid && seen_ids.contains(ref_step);
                    // Also confirm it's actually a pipeline step at all.
                    let is_pipeline_step =
                        pipeline.steps.iter().any(|s| s.id.0.as_str() == ref_step);
                    if !is_pipeline_step || !is_earlier {
                        return Err(IrError::UnknownCheckLocus {
                            check: c.0.clone(),
                            output: alloc::string::String::from(ref_step),
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

/// Resolve retry intents into `Check.retry`, enforce Guarantees G1+G2, detect
/// D7 span overlaps, and verify custom judge agents exist.
fn resolve_retries_and_guarantees(parsed: &mut Parsed) -> Result<(), IrError> {
    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    // D7 custom judge existence: independent of retry; iterate all checks.
    for (cid, check) in parsed.workflow.checks.iter() {
        if let CheckVerify::Deliverable {
            judge: JudgeRef::Agent(ref agent_id),
            ..
        } = &check.verify
        {
            if !parsed.workflow.agents.contains_key(agent_id) {
                return Err(IrError::UnknownJudgeAgent {
                    check: cid.0.clone(),
                    judge: agent_id.0.clone(),
                });
            }
        }
    }

    // Build pipeline index map: step_id -> (index, &PipelineStep).
    // If no pipeline, any retry-enabled check will fail with DeliverableNoProducer.
    let pipeline_index: BTreeMap<&str, (usize, &crate::pipeline::PipelineStep)> =
        match parsed.workflow.pipeline.as_ref() {
            Some(p) => p
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| (s.id.0.as_str(), (i, s)))
                .collect(),
            None => BTreeMap::new(),
        };

    let pipeline_steps: &[crate::pipeline::PipelineStep] = match parsed.workflow.pipeline.as_ref()
    {
        Some(p) => &p.steps,
        None => &[],
    };

    // For each retry-enabled check, resolve producer + gate, verify G1+G2.
    // Collect resolved retries into a Vec first to satisfy the borrow checker
    // (we cannot simultaneously hold an immutable reference to `checks` and
    // a mutable one for get_mut).
    let mut resolved_retries: Vec<(CheckId, Retry)> = Vec::new();

    // Also collect span intervals for D7 overlap check: (check_id, gi, check_pos_in_pipeline).
    // check_pos is the index of the StepRun::Check step in the pipeline (if positioned).
    let mut span_intervals: Vec<(alloc::string::String, usize, usize)> = Vec::new();

    for (cid, raw) in parsed.retry_intent.iter() {
        // Only resolve for on_fail == Retry.
        if raw.on_fail != OnFail::Retry {
            continue;
        }

        // Retrieve the check's locus (cloned to avoid borrow conflict).
        let locus: Locus = match parsed.workflow.checks.get(cid) {
            Some(check) => match &check.verify {
                CheckVerify::Goal { evaluates, .. } => evaluates.clone(),
                CheckVerify::Deliverable { locus, .. } => locus.clone(),
            },
            None => {
                // Should never happen (parse stage creates all checks before retry_intent).
                continue;
            }
        };

        // --- Decision 2: find the producer step ---
        let producer_id: PipelineStepId = match &locus {
            Locus::Output(ref step_id) => {
                // The locus references a pipeline step output directly.
                if pipeline_index.contains_key(step_id.0.as_str()) {
                    step_id.clone()
                } else {
                    return Err(IrError::DeliverableNoProducer {
                        check: cid.0.clone(),
                        locus: alloc::format!("steps.{}.output", step_id.0),
                    });
                }
            }
            Locus::Path(ref path) => {
                // Find the FIRST pipeline step that is Agent(a) where a declares `path`.
                let found = pipeline_steps.iter().find(|s| {
                    if let StepRun::Agent(ref a) = s.run {
                        if let Some(paths) = parsed.produces.get(a) {
                            return paths.iter().any(|p| p == path);
                        }
                    }
                    false
                });
                match found {
                    Some(s) => s.id.clone(),
                    None => {
                        return Err(IrError::DeliverableNoProducer {
                            check: cid.0.clone(),
                            locus: path.clone(),
                        });
                    }
                }
            }
        };

        // --- Decision 3: find the gate step ---
        let gate_id: PipelineStepId = match &raw.retry_from {
            Some(ref from_str) => {
                // Named step must exist in the pipeline.
                if pipeline_index.contains_key(from_str.as_str()) {
                    PipelineStepId(from_str.clone())
                } else {
                    return Err(IrError::UnknownRetryFrom {
                        check: cid.0.clone(),
                        retry_from: from_str.clone(),
                    });
                }
            }
            None => producer_id.clone(),
        };

        // --- Decision 4: Guarantee 1 (gate <= producer) ---
        let gi = pipeline_index[gate_id.0.as_str()].0;
        let pi = pipeline_index[producer_id.0.as_str()].0;
        if gi > pi {
            return Err(IrError::GateAfterProducer {
                check: cid.0.clone(),
                gate: gate_id.0.clone(),
                producer: producer_id.0.clone(),
            });
        }

        // --- Decision 5: Guarantee 2 (span has at least one agent step) ---
        let span_steps = &pipeline_steps[gi..=pi];
        let has_agent = span_steps
            .iter()
            .any(|s| matches!(s.run, StepRun::Agent(_)));
        if !has_agent {
            // Build human-readable span string.
            let span = if gi == pi {
                pipeline_steps[gi].id.0.clone()
            } else {
                let first = pipeline_steps[gi].id.0.as_str();
                let last = pipeline_steps[pi].id.0.as_str();
                alloc::format!("{first} -> {last}")
            };
            return Err(IrError::RetrySpanDeterministic {
                check: cid.0.clone(),
                span,
            });
        }

        // Find the check's own position in the pipeline (the StepRun::Check step for this cid).
        // This is used for D7 overlap calculation.
        let check_pos: Option<usize> = pipeline_steps.iter().position(|s| {
            matches!(&s.run, StepRun::Check(ref c) if c == cid)
        });

        // Queue the resolved retry for writing.
        resolved_retries.push((
            cid.clone(),
            Retry {
                on_fail: OnFail::Retry,
                max_attempts: raw.max_attempts,
                gate: gate_id.clone(),
                producer: producer_id.clone(),
            },
        ));

        // Register span interval for D7 if the check is positioned in the pipeline.
        if let Some(ci) = check_pos {
            span_intervals.push((cid.0.clone(), gi, ci));
        }
    }

    // Write resolved retries into workflow.checks.
    for (cid, retry) in resolved_retries {
        if let Some(check) = parsed.workflow.checks.get_mut(&cid) {
            check.retry = Some(retry);
        }
    }

    // --- Decision 6: D7 overlap detection ---
    // For each pair of positioned retry spans, check if they overlap.
    for i in 0..span_intervals.len() {
        for j in (i + 1)..span_intervals.len() {
            let (ref a_id, a_start, a_end) = span_intervals[i];
            let (ref b_id, b_start, b_end) = span_intervals[j];
            // Overlap: a.start <= b.end && b.start <= a.end
            if a_start <= b_end && b_start <= a_end {
                return Err(IrError::OverlappingRetrySpans {
                    a: a_id.clone(),
                    b: b_id.clone(),
                });
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
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&mut parsed).unwrap_err();
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
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&mut parsed).unwrap_err();
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
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        assert!(
            typecheck(&mut parsed).is_ok(),
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
        let mut parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
            produces: BTreeMap::new(),
            retry_intent: BTreeMap::new(),
        };
        let err = typecheck(&mut parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownSubflowToolTarget { ref tool, ref agent }
                if tool.0 == "call_ghost" && agent.0 == "ghost"),
            "expected UnknownSubflowToolTarget; got {err:?}"
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
        let mut parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(), // empty → "missing-step" not present
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
            produces: BTreeMap::new(),
            retry_intent: BTreeMap::new(),
        };
        let err = typecheck(&mut parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownStepToolTarget { ref tool, ref step }
                if tool.0 == "normalize" && step.0 == "missing-step"),
            "expected UnknownStepToolTarget; got {err:?}"
        );
    }

    // ---- New A6 tests ----

    #[test]
    fn rejects_deliverable_with_no_producing_step() {
        // A deliverable with `on_fail=retry` but no agent declares the path.
        let toml = r#"
[project]
name = "demo"

[agents.gather]
display_name = "G"
package = "demo@^0.1"
llm_backend = "mock-llm"

[deliverables.report]
path = "/workspace/report.md"
must_satisfy = "good"
on_fail = "retry"
max_attempts = 3

[[pipeline.steps]]
id = "gather"
run = "agent:gather"
"#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&mut parsed).unwrap_err();
        assert!(
            matches!(err, IrError::DeliverableNoProducer { .. }),
            "expected DeliverableNoProducer; got {err:?}"
        );
    }

    #[test]
    fn rejects_gate_after_producer() {
        // retry_from names a step AFTER the producer.
        let toml = r#"
[project]
name = "demo"

[agents.gather]
display_name = "G"
package = "demo@^0.1"
llm_backend = "mock-llm"
produces = ["/workspace/report.md"]

[agents.writer]
display_name = "W"
package = "demo@^0.1"
llm_backend = "mock-llm"

[deliverables.report]
path = "/workspace/report.md"
must_satisfy = "good"
on_fail = "retry"
max_attempts = 3
retry_from = "writer"

[[pipeline.steps]]
id = "gather"
run = "agent:gather"

[[pipeline.steps]]
id = "writer"
run = "agent:writer"
input = "${steps.gather.output}"

[[pipeline.steps]]
id = "verify"
run = "check:report"
input = "${steps.writer.output}"
"#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&mut parsed).unwrap_err();
        assert!(
            matches!(err, IrError::GateAfterProducer { .. }),
            "expected GateAfterProducer; got {err:?}"
        );
    }

    #[test]
    fn rejects_retry_span_fully_deterministic() {
        // G2: a retry span containing no agent step must be rejected.
        // A pipeline of [deterministic-step, check-step] with an Output
        // locus on the deterministic step and retry_from = that step yields
        // a span [normalize..=normalize] with no agent → RetrySpanDeterministic.
        let toml2 = r#"
[project]
name = "demo"

[steps.normalize]
deterministic = "parse_celsius"

[goals.has_output]
evaluates = "steps.normalize.output"
check = "exists"
on_fail = "retry"
max_attempts = 3

[[pipeline.steps]]
id = "normalize"
run = "deterministic:normalize"

[[pipeline.steps]]
id = "verify"
run = "check:has_output"
input = "${steps.normalize.output}"
"#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml2).unwrap();
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        // typecheck would first hit UnknownDeterministicFn (check #3 equivalent
        // for steps, but that's UnknownNativeTool)... actually UnknownDeterministicFn
        // is set at resolve stage, not typecheck. Typecheck only checks that the
        // native content_hash is non-zero. Deterministic steps don't have a
        // content_hash, so this passes check #3.
        // But wait — ToolImpl::Step doesn't have a content_hash. Check #3 only
        // rejects ToolImpl::Native with zero hash. So this should reach G2.
        let err = typecheck(&mut parsed).unwrap_err();
        assert!(
            matches!(err, IrError::RetrySpanDeterministic { .. }),
            "expected RetrySpanDeterministic; got {err:?}"
        );
    }

    #[test]
    fn rejects_overlapping_retry_spans() {
        // Two positioned retry checks with overlapping spans.
        // check1: span [gather..=verify1] (gi=0, ci=2)
        // check2: span [gather..=verify2] (gi=0, ci=3) — overlaps with check1
        let toml = r#"
[project]
name = "demo"

[agents.gather]
display_name = "G"
package = "demo@^0.1"
llm_backend = "mock-llm"
produces = ["/workspace/out1.md", "/workspace/out2.md"]

[agents.writer]
display_name = "W"
package = "demo@^0.1"
llm_backend = "mock-llm"

[deliverables.check1]
path = "/workspace/out1.md"
must_satisfy = "good"
on_fail = "retry"
max_attempts = 2

[deliverables.check2]
path = "/workspace/out2.md"
must_satisfy = "good"
on_fail = "retry"
max_attempts = 2

[[pipeline.steps]]
id = "gather"
run = "agent:gather"

[[pipeline.steps]]
id = "writer"
run = "agent:writer"
input = "${steps.gather.output}"

[[pipeline.steps]]
id = "verify1"
run = "check:check1"
input = "${steps.writer.output}"

[[pipeline.steps]]
id = "verify2"
run = "check:check2"
input = "${steps.writer.output}"
"#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&mut parsed).unwrap_err();
        assert!(
            matches!(err, IrError::OverlappingRetrySpans { .. }),
            "expected OverlappingRetrySpans; got {err:?}"
        );
    }

    #[test]
    fn rejects_unknown_judge_agent() {
        // Deliverable with judge = "ghost" but no [agents.ghost].
        let toml = r#"
[project]
name = "demo"

[agents.gather]
display_name = "G"
package = "demo@^0.1"
llm_backend = "mock-llm"

[deliverables.report]
path = "/workspace/report.md"
must_satisfy = "good"
judge = "ghost"
"#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        let err = typecheck(&mut parsed).unwrap_err();
        assert!(
            matches!(err, IrError::UnknownJudgeAgent { ref check, ref judge }
                if check == "report" && judge == "ghost"),
            "expected UnknownJudgeAgent{{report, ghost}}; got {err:?}"
        );
    }

    #[test]
    fn valid_retry_resolves_producer_and_gate() {
        // Full valid case: gather (no produces) -> writer (produces path) ->
        // verify (check:report). on_fail=retry, retry_from=writer.
        let toml = r#"
[project]
name = "demo"

[agents.gather]
display_name = "G"
package = "demo@^0.1"
llm_backend = "mock-llm"

[agents.writer]
display_name = "W"
package = "demo@^0.1"
llm_backend = "mock-llm"
produces = ["/workspace/report.md"]

[deliverables.report]
path = "/workspace/report.md"
must_satisfy = "good"
on_fail = "retry"
max_attempts = 3
retry_from = "writer"

[[pipeline.steps]]
id = "gather"
run = "agent:gather"

[[pipeline.steps]]
id = "writer"
run = "agent:writer"
input = "${steps.gather.output}"

[[pipeline.steps]]
id = "verify"
run = "check:report"
input = "${steps.writer.output}"
"#;
        let cfg = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let mut parsed = crate::lower::parse::parse(&cfg).unwrap();
        typecheck(&mut parsed).expect("should be valid");
        let check = parsed
            .workflow
            .checks
            .get(&crate::ids::CheckId("report".to_string()))
            .expect("check present");
        let retry = check.retry.as_ref().expect("retry should be resolved");
        assert_eq!(retry.gate.0, "writer", "gate should be writer");
        assert_eq!(retry.producer.0, "writer", "producer should be writer");
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.on_fail, crate::check::OnFail::Retry);
    }
}
