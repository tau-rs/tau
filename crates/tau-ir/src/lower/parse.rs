//! First lowering stage: pull typed declarations out of `ProjectConfig`.
//!
//! This stage does no I/O and no resolution. It walks the
//! `ProjectConfig`'s agent/tool/step tables and produces an
//! in-memory `Parsed` value: a partially-populated `Workflow` whose
//! `ToolImpl::Native::content_hash` and similar resolution slots are
//! filled with zero bytes (the `resolve` stage fills them).

use alloc::collections::BTreeMap;
use tau_pkg::project::ProjectConfig;
use tau_pkg::{PromptEntry, ToolBody};

use crate::capability::{CapabilityRequirements, CapabilityTable};
use crate::error::IrError;
use crate::ids::{AgentId, StepId, ToolId};
use crate::module::Workflow;
use crate::node::{Agent, Deterministic, Tool, ToolSpec};
use crate::subflow::{SubflowEdge, SubflowKind};
use crate::tool_impl::{Hash256, NativeFnRef, ToolImpl};
use crate::AgentBudget;

/// Output of the parse stage.
pub(super) struct Parsed {
    /// Partially-populated workflow (content hashes are zero pending `resolve`).
    pub(super) workflow: Workflow,
}

/// Run the parse stage on a `ProjectConfig`.
pub(super) fn parse(config: &ProjectConfig) -> Result<Parsed, IrError> {
    let mut agents: BTreeMap<AgentId, Agent> = BTreeMap::new();
    let mut tools: BTreeMap<ToolId, Tool> = BTreeMap::new();
    let mut steps: BTreeMap<StepId, Deterministic> = BTreeMap::new();
    let mut edges: alloc::vec::Vec<SubflowEdge> = alloc::vec::Vec::new();
    let mut capability_table: BTreeMap<ToolId, CapabilityRequirements> = BTreeMap::new();

    // --- Tools ---------------------------------------------------------
    //
    // `ProjectConfig::tools` is a `BTreeMap<String, ToolEntry>` produced by
    // tau-pkg::config. Each `ToolEntry` discriminates Native vs Mcp vs Subflow.
    for (name, entry) in config.tools.iter() {
        let tool_id = ToolId(name.clone());
        let caps = CapabilityRequirements {
            declared: entry.capabilities.clone(),
        };
        let impl_ = match &entry.body {
            ToolBody::Native(fn_name) => ToolImpl::Native {
                fn_ref: NativeFnRef {
                    name: fn_name.clone(),
                },
                // resolved by `resolve` stage; zero is a sentinel
                content_hash: Hash256::default(),
            },
            ToolBody::Mcp(url) => ToolImpl::Mcp {
                url: url.clone(),
                contract_hash: Hash256::default(),
                capability_subset: caps.clone(),
            },
            ToolBody::Subflow(target) => {
                // Subflow-as-tool is sugar for a SubflowEdge::Spawn; we
                // emit an edge and DO NOT register a Tool node for it.
                edges.push(SubflowEdge {
                    id: crate::SubflowId(name.clone()),
                    kind: SubflowKind::Spawn {
                        target_agent: AgentId(target.clone()),
                        cap_subset: caps,
                    },
                });
                continue;
            }
            // `ToolBody` is `#[non_exhaustive]`; future variants are
            // forwarded as a parse error so existing callers aren't
            // silently broken when a new variant lands.
            _ => {
                return Err(IrError::Parse("unsupported tool body variant".into()));
            }
        };
        let spec = ToolSpec {
            name: name.clone(),
            description: entry.description.clone(),
            input_schema: entry.input_schema.clone(),
        };
        capability_table.insert(tool_id.clone(), caps.clone());
        tools.insert(
            tool_id.clone(),
            Tool {
                id: tool_id,
                impl_,
                capabilities: caps,
                spec,
            },
        );
    }

    // --- Agents --------------------------------------------------------
    for (name, entry) in config.agents.iter() {
        let agent_id = AgentId(name.clone());
        let tool_refs = entry.tool_refs.iter().cloned().map(ToolId).collect();
        // Resolve the prompt string from the PromptEntry enum.
        // PromptEntry::File is normalized to the path string; the
        // interpreter (β.2.4) is responsible for loading the file.
        let prompt = match &entry.prompt {
            PromptEntry::Inline(s) => s.clone(),
            PromptEntry::File(p) => p.to_string_lossy().into_owned(),
            PromptEntry::None => alloc::string::String::new(),
            // Non_exhaustive — default to empty string for any future variant.
            _ => alloc::string::String::new(),
        };
        agents.insert(
            agent_id.clone(),
            Agent {
                id: agent_id,
                prompt,
                model: entry.model.clone(),
                tool_refs,
                context: None, // β.4 fills this in when its config table exists
                budget: AgentBudget {
                    max_turns: entry.max_turns,
                    max_tokens: entry.max_tokens,
                },
            },
        );
    }

    // --- Deterministic steps ------------------------------------------
    for (name, entry) in config.steps.iter() {
        let step_id = StepId(name.clone());
        steps.insert(
            step_id.clone(),
            Deterministic {
                id: step_id,
                fn_ref: NativeFnRef {
                    name: entry.fn_name.clone(),
                },
                input_schema: entry.input_schema.clone(),
                output_schema: entry.output_schema.clone(),
            },
        );
    }

    Ok(Parsed {
        workflow: Workflow {
            agents,
            tools,
            steps,
            edges,
            capability_table: CapabilityTable(capability_table),
        },
    })
}
