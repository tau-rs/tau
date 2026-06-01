//! `ServerContract` — what tau pins for an MCP server.
//!
//! Captures the server's `initialize` response + `tools/list` snapshot
//! at build time. PR-4's lowering pass canonical-hashes a `ServerContract`
//! and stores `(url, contract_hash, expanded_tools)` in the lockfile.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use tau_domain::Capability;

use crate::protocol::initialize::ServerInfo;
use crate::protocol::tools::{McpTool, McpToolInputSchema};

/// Frozen server contract.
///
/// One contract per MCP server URL. `tau build` constructs this from
/// the live (or pinned) handshake; `canonical_hash` produces the
/// `Hash256` that participates in the IR module hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerContract {
    /// MCP protocol version the server advertised at `initialize`.
    pub protocol_version: String,
    /// Server's reported info.
    pub server_info: ServerInfo,
    /// The full `tools/list` snapshot, in server order.
    pub tools: Vec<ContractTool>,
}

/// One tool from the server's `tools/list` plus its declared caps.
///
/// `caps` is the **server-declared** capability set for this tool. PR-4
/// intersects it with the author's per-server envelope before storing
/// in the IR's `ToolImpl::Mcp::capability_subset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractTool {
    /// Server-side tool name.
    pub name: String,
    /// Server-supplied description (passed through to the LLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Server-supplied input JSON schema.
    pub input_schema: McpToolInputSchema,
    /// Server-declared capabilities (per-tool).
    ///
    /// Note: MCP spec does NOT currently standardize a "capability
    /// declaration" field on tools/list entries. tau extracts caps from
    /// a tau-specific extension field; if the server doesn't ship it,
    /// caps default to the empty vector and the author's envelope is
    /// the upper bound (per the spec's "envelope ∩ contract" rule).
    ///
    /// β.3.1 may evolve this once the MCP spec lands per-tool caps.
    #[serde(default)]
    pub caps: Vec<Capability>,
}

impl ServerContract {
    /// Build a `ServerContract` from a handshake-completed pair of
    /// (`InitializeResponse`, `ToolsListResponse`). PR-2 + PR-3 wire
    /// this from the live transport.
    ///
    /// `caps_extractor` lets the caller pull caps from a tau-specific
    /// extension field on each `McpTool`; if the extension is absent
    /// (most off-the-shelf servers), the closure returns `Vec::new()`
    /// and the author's envelope is the upper bound.
    pub fn from_handshake<F>(
        init: crate::protocol::initialize::InitializeResponse,
        tools_list: crate::protocol::tools::ToolsListResponse,
        mut caps_extractor: F,
    ) -> Self
    where
        F: FnMut(&McpTool) -> Vec<Capability>,
    {
        let tools = tools_list
            .tools
            .into_iter()
            .map(|t| ContractTool {
                caps: caps_extractor(&t),
                name: t.name,
                description: t.description,
                input_schema: t.input_schema,
            })
            .collect();
        ServerContract {
            protocol_version: init.protocol_version,
            server_info: init.server_info,
            tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn weather_contract() -> ServerContract {
        ServerContract {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            tools: vec![ContractTool {
                name: "get_forecast".to_string(),
                description: Some("Get weather forecast".to_string()),
                input_schema: McpToolInputSchema(json!({
                    "type":"object",
                    "properties":{"lat":{"type":"number"},"lon":{"type":"number"}}
                })),
                caps: vec![],
            }],
        }
    }

    #[test]
    fn server_contract_round_trips() {
        let c = weather_contract();
        let bytes = serde_json::to_vec(&c).expect("serialize");
        let decoded: ServerContract = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(c, decoded);
    }

    #[test]
    fn from_handshake_constructs_contract() {
        use crate::protocol::initialize::InitializeResponse;
        use crate::protocol::tools::ToolsListResponse;

        let init = InitializeResponse {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.0".to_string(),
                additional: BTreeMap::new(),
            },
            capabilities: None,
        };
        let tools = ToolsListResponse {
            tools: vec![McpTool {
                name: "get_forecast".to_string(),
                description: None,
                input_schema: McpToolInputSchema(json!({"type":"object"})),
            }],
            next_cursor: None,
        };
        let c = ServerContract::from_handshake(init, tools, |_| vec![]);
        assert_eq!(c.tools.len(), 1);
        assert_eq!(c.tools[0].name, "get_forecast");
        assert!(c.tools[0].caps.is_empty());
    }
}
