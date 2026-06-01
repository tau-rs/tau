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
use crate::subflow::SubflowEdge;
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
    let edges: alloc::vec::Vec<SubflowEdge> = alloc::vec::Vec::new();
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
            ToolBody::Subflow(target) => ToolImpl::Subflow {
                target: AgentId(target.clone()),
            },
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

    // --- Deterministic steps as tools ----------------------------------
    //
    // Each [steps.<name>] block registers BOTH a `Deterministic` node
    // (above) and a `Tool` with `ToolImpl::Step { id }`. The Tool
    // registration is what lets an agent reference the step in its
    // `tool_refs`; the Deterministic node is what the runtime registry
    // dispatches against at invoke time.
    for (name, entry) in config.steps.iter() {
        let step_id = StepId(name.clone());
        let tool_id = ToolId(name.clone());
        let caps = CapabilityRequirements {
            declared: alloc::vec::Vec::new(),
        };
        let spec = ToolSpec {
            name: name.clone(),
            description: alloc::string::String::new(),
            input_schema: entry.input_schema.clone(),
        };
        capability_table.insert(tool_id.clone(), caps.clone());
        tools.insert(
            tool_id.clone(),
            Tool {
                id: tool_id,
                impl_: ToolImpl::Step { id: step_id },
                capabilities: caps,
                spec,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_impl::ToolImpl;
    use tau_pkg::project::ProjectConfig;

    #[test]
    fn parse_registers_tool_node_for_subflow_body() {
        let toml = r#"
[project]
name = "p"

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["notify"]

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = []

[tools.notify]
subflow     = "worker"
description = "Hand off to worker"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config).expect("parse stage");

        let tool = parsed
            .workflow
            .tools
            .get(&ToolId("notify".into()))
            .expect("notify tool registered");
        assert!(
            matches!(&tool.impl_, ToolImpl::Subflow { target } if target.0 == "worker"),
            "expected ToolImpl::Subflow targeting worker; got {:?}",
            tool.impl_
        );
    }

    #[test]
    fn parse_registers_tool_node_for_each_step() {
        let toml = r#"
[project]
name = "p"

[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["normalize"]

[steps.normalize]
deterministic = "parse_celsius"
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config).expect("parse stage");

        // Step registered in workflow.steps:
        assert!(parsed
            .workflow
            .steps
            .contains_key(&StepId("normalize".into())));

        // AND registered as a Tool with ToolImpl::Step:
        let tool = parsed
            .workflow
            .tools
            .get(&ToolId("normalize".into()))
            .expect("normalize tool registered");
        assert!(
            matches!(&tool.impl_, ToolImpl::Step { id } if id.0 == "normalize"),
            "expected ToolImpl::Step{{normalize}}; got {:?}",
            tool.impl_
        );
    }

    #[test]
    fn parse_emits_no_subflow_edge_for_subflow_body() {
        // v0 routes subflow dispatch through ToolImpl::Subflow exclusively;
        // SubflowEdge is reserved for SubflowKind::Compose (future). This
        // test pins the new shape so a regression that re-introduces the
        // edge gets caught.
        let toml = r#"
[project]
name = "p"

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["notify"]

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = []

[tools.notify]
subflow     = "worker"
description = "Hand off to worker"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config).expect("parse stage");
        assert!(
            parsed.workflow.edges.is_empty(),
            "expected no SubflowEdge entries; got {:?}",
            parsed.workflow.edges
        );
    }
}
