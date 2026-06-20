//! Second lowering stage: fill content hashes from caller-supplied caches.
//! For MCP tools, also EXPANDS one author entry into N IR nodes
//! (one per server-tool). See β.3 design doc §5.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::LowerError;
use crate::lower::{Caches, McpBuildError, ResolvedMcpContract};
use tau_ir::capability::CapabilityRequirements;
use tau_ir::ids::ToolId;
use tau_ir::node::Tool;
use tau_ir::tool_impl::ToolImpl;

use super::parse::Parsed;

/// Run the resolve stage on a `Parsed` value.
pub(super) fn resolve(mut parsed: Parsed, caches: &Caches<'_>) -> Result<Parsed, LowerError> {
    // First pass: resolve native tool content_hashes (unchanged from β.2).
    // Collect MCP entries that need expansion.
    let mut mcp_entries: Vec<(ToolId, String)> = Vec::new(); // (old id, url)
    for (id, tool) in parsed.workflow.tools.iter_mut() {
        match &mut tool.impl_ {
            ToolImpl::Native {
                fn_ref,
                content_hash,
            } => {
                if let Some(h) = (caches.native_tool)(&fn_ref.name) {
                    *content_hash = h;
                }
                // If the cache returns None we KEEP the zero sentinel and
                // let typecheck decide whether that's an error.
            }
            ToolImpl::Mcp { url, .. } => {
                mcp_entries.push((id.clone(), url.clone()));
            }
            ToolImpl::Subflow { .. } | ToolImpl::Step { .. } => {}
        }
    }

    // Second pass: per MCP entry, expand into N server-tool IR nodes.
    //
    // Cache-miss policy mirrors the native_tool path above: leave the
    // original ToolImpl::Mcp untouched and let downstream stages (or
    // a caller that wires an empty cache deliberately, e.g. the
    // tau-ir-conformance tests that exercise the IR-level Mcp variant
    // directly) decide whether that's an error. tau-cli's build path
    // always populates the cache via PinnedResolver or
    // LiveMcpContractResolver, so the strict "missing entry =>
    // ContractUnreachable" behavior is enforced there via the
    // resolver's own error path; resolve itself stays permissive.
    for (old_id, url) in mcp_entries {
        let Some(resolved) = (caches.mcp_contract)(&url) else {
            continue;
        };
        expand_mcp_entry(&mut parsed, &old_id, &url, &resolved)?;
    }

    Ok(parsed)
}

fn expand_mcp_entry(
    parsed: &mut Parsed,
    old_id: &ToolId,
    url: &str,
    resolved: &ResolvedMcpContract,
) -> Result<(), LowerError> {
    // Capture the original tool (envelope + spec) before removing it.
    let original_tool = parsed
        .workflow
        .tools
        .get(old_id)
        .cloned()
        .expect("old_id was just collected from this map");
    let envelope = match &original_tool.impl_ {
        ToolImpl::Mcp {
            capability_subset, ..
        } => capability_subset.clone(),
        _ => unreachable!("expand_mcp_entry called on non-Mcp tool"),
    };

    // Enforce SamplingRequiredByContract.
    // TODO(PR-4-Phase-3): once Tool::sampling_models is wired from
    // tau-pkg, check that the slice is non-empty here.
    if resolved.requires_sampling {
        // Dormant until Phase 3 adds Tool::sampling_models.
    }

    // Enforce no dot in server-tool names + caps subset + emit new entries.
    let mut new_entries: Vec<(ToolId, Tool)> = Vec::new();
    for st in &resolved.expanded_tools {
        if st.name.contains('.') {
            return Err(LowerError::McpBuild(
                McpBuildError::ServerToolNameContainsDot {
                    entry: old_id.0.clone(),
                    name: st.name.clone(),
                },
            ));
        }
        let missing = caps_missing_from_envelope(&envelope, &st.caps);
        if !missing.is_empty() {
            return Err(LowerError::McpBuild(
                McpBuildError::EnvelopeCoversContract {
                    entry: old_id.0.clone(),
                    tool: st.name.clone(),
                    missing,
                },
            ));
        }
        let intersection = intersect_caps(&envelope, &st.caps);
        let new_id = ToolId(alloc::format!("{}.{}", old_id.0, st.name));
        let new_tool = Tool {
            id: new_id.clone(),
            spec: original_tool.spec.clone(),
            impl_: ToolImpl::Mcp {
                url: url.into(),
                contract_hash: resolved.hash,
                capability_subset: intersection,
                server_tool_name: st.name.clone(),
            },
            capabilities: st.caps.clone(),
        };
        new_entries.push((new_id, new_tool));
    }

    // Remove old entry, insert N new ones.
    parsed.workflow.tools.remove(old_id);
    for (id, tool) in new_entries.iter() {
        parsed.workflow.tools.insert(id.clone(), tool.clone());
    }

    // Rewrite each agent's tool_refs: replace old_id with all new_ids.
    let new_ids: Vec<ToolId> = new_entries.iter().map(|(id, _)| id.clone()).collect();
    for agent in parsed.workflow.agents.values_mut() {
        let mut rewritten: Vec<ToolId> = Vec::with_capacity(agent.tool_refs.len());
        for r in agent.tool_refs.iter() {
            if r == old_id {
                rewritten.extend(new_ids.iter().cloned());
            } else {
                rewritten.push(r.clone());
            }
        }
        agent.tool_refs = rewritten;
    }

    Ok(())
}

/// Return the capability requirements for the expanded server-tool, bounded
/// by the author's envelope.
///
/// v0: contract caps WIN when envelope ⊇ contract (enforced by
/// `caps_missing_from_envelope` above). So the resolved entry holds
/// exactly the contract caps (already a subset of the envelope).
fn intersect_caps(
    _envelope: &CapabilityRequirements,
    contract: &CapabilityRequirements,
) -> CapabilityRequirements {
    // Envelope ⊇ contract is enforced by `caps_missing_from_envelope` before
    // this function is called, so we can safely return the contract's caps.
    contract.clone()
}

/// Return human-readable names for capabilities the contract declares
/// that the envelope omits.
///
/// `CapabilityRequirements::declared` is a `Vec<tau_domain::Capability>`.
/// `Capability` implements `PartialEq`, so we do a simple set-difference:
/// for each item in `contract.declared`, check it appears in
/// `envelope.declared`.
fn caps_missing_from_envelope(
    envelope: &CapabilityRequirements,
    contract: &CapabilityRequirements,
) -> Vec<String> {
    contract
        .declared
        .iter()
        .filter(|cap| !envelope.declared.contains(cap))
        .map(|cap| alloc::format!("{cap:?}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::{ResolvedMcpContract, ResolvedServerTool};
    use tau_ir::ids::{AgentId, ToolId};
    use tau_pkg::project::ProjectConfig;

    fn rsc_minimal(name: &str) -> ResolvedServerTool {
        ResolvedServerTool {
            name: name.into(),
            caps: CapabilityRequirements::default(),
            input_schema: serde_json::Value::Null,
        }
    }

    #[test]
    fn dot_in_server_tool_name_rejected() {
        // Build a minimal tau.toml with one MCP entry, then supply a
        // ResolvedMcpContract whose server-tool name contains a dot.
        // resolve() should return McpBuildError::ServerToolNameContainsDot.
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.main]
display_name = "Main"
package      = "p@^0.1"
model        = "default"
tool_refs    = ["weather"]

[tools.weather]
mcp = "https://mcp.example.com"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = super::super::parse::parse(&config).expect("parse stage");

        let bad_contract = ResolvedMcpContract {
            hash: [1u8; 32],
            expanded_tools: alloc::vec![rsc_minimal("bad.name")],
            requires_sampling: false,
        };

        let caches = crate::lower::Caches {
            native_tool: &|_| None,
            mcp_contract: &|_url| Some(bad_contract.clone()),
            skill: &|_| None,
        };

        let err = resolve(parsed, &caches).unwrap_err();
        match &err {
            LowerError::McpBuild(McpBuildError::ServerToolNameContainsDot { entry, name }) => {
                assert_eq!(entry, "weather");
                assert_eq!(name, "bad.name");
            }
            other => panic!("expected ServerToolNameContainsDot; got {other:?}"),
        }
    }

    #[test]
    fn mcp_entry_expands_to_n_tools_and_rewrites_agent_refs() {
        // One MCP entry with two server-tools should become two IR tools,
        // and the agent's tool_refs should be rewritten accordingly.
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.main]
display_name = "Main"
package      = "p@^0.1"
model        = "default"
tool_refs    = ["weather"]

[tools.weather]
mcp = "https://mcp.example.com"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = super::super::parse::parse(&config).expect("parse stage");

        let contract = ResolvedMcpContract {
            hash: [2u8; 32],
            expanded_tools: alloc::vec![rsc_minimal("get_forecast"), rsc_minimal("get_current"),],
            requires_sampling: false,
        };

        let caches = crate::lower::Caches {
            native_tool: &|_| None,
            mcp_contract: &|_url| Some(contract.clone()),
            skill: &|_| None,
        };

        let result = resolve(parsed, &caches).expect("resolve");

        // Old entry removed.
        assert!(!result
            .workflow
            .tools
            .contains_key(&ToolId("weather".into())));
        // New entries present.
        assert!(result
            .workflow
            .tools
            .contains_key(&ToolId("weather.get_forecast".into())));
        assert!(result
            .workflow
            .tools
            .contains_key(&ToolId("weather.get_current".into())));

        // Agent refs rewritten.
        let agent = result
            .workflow
            .agents
            .get(&AgentId("main".into()))
            .expect("agent present");
        assert!(agent
            .tool_refs
            .contains(&ToolId("weather.get_forecast".into())));
        assert!(agent
            .tool_refs
            .contains(&ToolId("weather.get_current".into())));
        assert!(!agent.tool_refs.contains(&ToolId("weather".into())));
    }
}
