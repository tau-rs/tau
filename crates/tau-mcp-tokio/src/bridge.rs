//! `McpBackedTool` — implements `DynTool` over an `Arc<McpClient>` so
//! MCP-expanded tools register in the standard
//! `BTreeMap<ToolId, Arc<dyn DynTool>>` that `ForwardingDispatcher`
//! already owns.
//!
//! One `McpBackedTool` per IR ToolId (`<entry>.<server-tool>`).
//! All entries for the same `[tools.<entry>]` share the same
//! `Arc<McpClient>`; only `server_tool_name` + `capability_subset`
//! differ.

use std::sync::Arc;

use tau_domain::{Capability, Value};
use tau_mcp::protocol::tools::{ContentBlock, ToolsCallResponse};
use tau_ports::error::ToolError;
use tau_ports::tool::{SessionContext, ToolContent, ToolResult};
use tau_ports::{Tool, ToolSpec};

use crate::host_lifecycle::McpClient;

/// One MCP-expanded server-tool exposed as a `DynTool`.
pub struct McpBackedTool {
    /// IR ToolId in the form `<entry>.<server-tool>` (e.g. `weather.get_forecast`).
    /// Held as the tool's `name()` per DynTool contract.
    ir_tool_id: String,
    /// Shared MCP client for this server entry.
    client: Arc<McpClient>,
    /// Server-side tool name sent on `tools/call`.
    server_tool_name: String,
    /// Capabilities the tool requires (per PR-4 expansion's intersection).
    capabilities: Vec<Capability>,
    /// JSON Schema for the tool's input (passed through from the contract).
    /// Stored as `tau_domain::Value` to match `ToolSpec.input_schema`.
    input_schema: Value,
    /// Human-readable description, propagated to the LLM tools/list.
    description: String,
}

impl McpBackedTool {
    /// Construct one MCP-backed tool.
    ///
    /// `input_schema_json` is the raw `serde_json::Value` from the MCP
    /// server's `inputSchema`; it is converted to `tau_domain::Value`
    /// here so `schema()` returns the right type.
    pub fn new(
        ir_tool_id: impl Into<String>,
        client: Arc<McpClient>,
        server_tool_name: impl Into<String>,
        capabilities: Vec<Capability>,
        input_schema_json: serde_json::Value,
        description: impl Into<String>,
    ) -> Arc<Self> {
        // serde_json::Value → tau_domain::Value via JSON round-trip.
        // Both enums share the same on-wire shape, so this is lossless.
        let input_schema: Value =
            serde_json::from_value(input_schema_json).unwrap_or(Value::Object(Default::default()));
        Arc::new(Self {
            ir_tool_id: ir_tool_id.into(),
            client,
            server_tool_name: server_tool_name.into(),
            capabilities,
            input_schema,
            description: description.into(),
        })
    }

    /// Server-side tool name (for diagnostics / tests).
    pub fn server_tool_name(&self) -> &str {
        &self.server_tool_name
    }
}

impl Tool for McpBackedTool {
    type Session = ();

    fn name(&self) -> &str {
        &self.ir_tool_id
    }

    fn schema(&self) -> ToolSpec {
        // ToolSpec is #[non_exhaustive] outside tau-ports — construct via
        // serde round-trip (same escape-hatch as ForwardingDispatcher's
        // test stubs in ir_dispatcher.rs).
        serde_json::from_value(serde_json::json!({
            "name": self.ir_tool_id,
            "description": self.description,
            "input_schema": self.input_schema,
        }))
        .expect("McpBackedTool ToolSpec must deserialize")
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
        // McpBackedTool is stateless; no per-session setup. The MCP
        // client itself was opened at runtime boot.
        Ok(())
    }

    async fn invoke(
        &self,
        _session: &mut Self::Session,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        // tau_domain::Value → serde_json::Value for the wire call.
        let json_args = serde_json::to_value(&args).map_err(|e| ToolError::Internal {
            message: format!("encode MCP args: {e}"),
        })?;
        let resp: ToolsCallResponse = self
            .client
            .call_tool(&self.server_tool_name, json_args)
            .await
            .map_err(|e| ToolError::Internal {
                message: format!("MCP call_tool {:?}: {e}", self.server_tool_name),
            })?;
        // Convert MCP Content[] to tau ToolResult.
        // Text blocks map to ToolContent::Text; non-text blocks are
        // joined as a text placeholder in v0 (β.3.1 may surface them
        // structurally).
        let content = mcp_content_to_tool_content(&resp.content);
        let is_error = resp.is_error.unwrap_or(false);
        Ok(ToolResult::new(content, is_error))
    }

    async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
        Ok(())
    }
}

/// Convert MCP `ContentBlock[]` to `tau_ports::ToolContent[]`.
///
/// Text → `ToolContent::Text`. Image + Resource → `ToolContent::Text`
/// placeholder in v0; β.3.1 may add `ToolContent::ImageRef` etc.
fn mcp_content_to_tool_content(blocks: &[ContentBlock]) -> Vec<ToolContent> {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => ToolContent::Text { text: text.clone() },
            ContentBlock::Image { mime_type, .. } => ToolContent::Text {
                text: format!("[image/{mime_type} — not rendered in v0]"),
            },
            ContentBlock::Resource { .. } => ToolContent::Text {
                text: "[resource — not rendered in v0]".to_string(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text {
            text: s.to_string(),
        }
    }

    #[test]
    fn mcp_content_to_tool_content_maps_text_blocks() {
        let blocks = vec![text_block("hello"), text_block("world")];
        let result = mcp_content_to_tool_content(&blocks);
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0], ToolContent::Text { text } if text == "hello"));
        assert!(matches!(&result[1], ToolContent::Text { text } if text == "world"));
    }

    #[test]
    fn mcp_content_to_tool_content_handles_single_block() {
        let blocks = vec![text_block("solo")];
        let result = mcp_content_to_tool_content(&blocks);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ToolContent::Text { text } if text == "solo"));
    }

    #[test]
    fn mcp_content_to_tool_content_handles_empty() {
        let blocks: Vec<ContentBlock> = vec![];
        let result = mcp_content_to_tool_content(&blocks);
        assert!(result.is_empty());
    }
}
