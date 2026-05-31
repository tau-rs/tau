//! Third lowering stage: workflow-shape invariants.

use crate::error::IrError;
use crate::subflow::SubflowKind;
use crate::tool_impl::ToolImpl;

use super::parse::Parsed;

/// Run the typecheck stage on a `Parsed` value.
pub fn typecheck(parsed: &Parsed) -> Result<(), IrError> {
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
        if let ToolImpl::Native { fn_ref, content_hash } = &tool.impl_ {
            if content_hash == &[0u8; 32] {
                return Err(IrError::UnknownDeterministicFn {
                    step: crate::StepId(tool_id.0.clone()),
                    fn_name: fn_ref.name.clone(),
                });
            }
        }
    }

    Ok(())
}
