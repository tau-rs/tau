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

    // 6. Each trigger's entrypoint agent must exist.
    for trigger in parsed.triggers.iter() {
        if !parsed.workflow.agents.contains_key(&trigger.agent) {
            return Err(IrError::UnknownTriggerAgent {
                trigger: trigger.name.clone(),
                agent: trigger.agent.clone(),
            });
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
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
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
        let parsed = Parsed {
            workflow: Workflow {
                agents,
                tools,
                steps: BTreeMap::new(), // empty → "missing-step" not present
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
            },
            triggers: alloc::vec::Vec::new(),
        };
        let err = typecheck(&parsed).expect_err("typecheck should reject");
        assert!(
            matches!(err, IrError::UnknownStepToolTarget { ref tool, ref step }
                if tool.0 == "normalize" && step.0 == "missing-step"),
            "expected UnknownStepToolTarget; got {err:?}"
        );
    }
}
