//! `tools/list` and `tools/call` payloads.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `tools/list` request — empty params.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolsListRequest {
    /// Optional cursor for paginated tool lists (rarely used in 2026
    /// servers; we accept but don't paginate in v0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `tools/list` response — a vector of advertised tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsListResponse {
    /// One entry per tool the server exposes.
    pub tools: Vec<McpTool>,
    /// Cursor for the next page (we accept but don't follow in v0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// A tool advertised by an MCP server.
///
/// PR-4 expands one `ToolImpl::Mcp` per `[tools.<entry>]` in tau.toml
/// into one `Tool` per `McpTool` in this list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name as the server expects it on the wire (e.g.
    /// `"get_forecast"`). PR-4 forbids `.` in this name to avoid
    /// IR-ToolId namespace collision.
    pub name: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for the tool's input.
    #[serde(rename = "inputSchema")]
    pub input_schema: McpToolInputSchema,
}

/// JSON schema for a tool's input — wrapped to preserve serde shape.
///
/// MCP servers ship JSON Schema 2020-12 (or similar). tau passes the
/// schema through opaquely to the LLM; no validation in v0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpToolInputSchema(pub Value);

/// `tools/call` request — invoke a server tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsCallRequest {
    /// Server-side tool name (NOT the IR ToolId).
    pub name: String,
    /// Tool arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// `tools/call` response — the tool's output content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsCallResponse {
    /// One or more content blocks (text / image / resource).
    pub content: Vec<ContentBlock>,
    /// Set to `true` if the server reports the tool errored.
    #[serde(default, rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// One block of tool result content.
///
/// MCP servers commonly return `Text`. `Image` + `Resource` are spec
/// but rare in 2026 ecosystem; tau accepts but doesn't render them in
/// v0 (passed through as opaque JSON to the agent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },
    /// Image (base64 data + mime).
    Image {
        /// Base64-encoded image bytes.
        data: String,
        /// MIME type (e.g. `"image/png"`).
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Embedded resource reference.
    Resource {
        /// Resource payload (free-form).
        resource: Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn tools_list_response_round_trips() {
        let resp = ToolsListResponse {
            tools: vec![McpTool {
                name: "get_forecast".to_string(),
                description: Some("Get a weather forecast".to_string()),
                input_schema: McpToolInputSchema(json!({
                    "type":"object",
                    "properties":{"lat":{"type":"number"},"lon":{"type":"number"}},
                    "required":["lat","lon"]
                })),
            }],
            next_cursor: None,
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: ToolsListResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn tools_call_request_round_trips() {
        let req = ToolsCallRequest {
            name: "get_forecast".to_string(),
            arguments: Some(json!({"lat":40.7,"lon":-74.0})),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: ToolsCallRequest = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
    }

    #[test]
    fn tools_call_response_text_round_trips() {
        let resp = ToolsCallResponse {
            content: vec![ContentBlock::Text {
                text: "Sunny, 72°F".to_string(),
            }],
            is_error: None,
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: ToolsCallResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn tools_call_response_error_flag_preserved() {
        let resp = ToolsCallResponse {
            content: vec![ContentBlock::Text {
                text: "rate limited".to_string(),
            }],
            is_error: Some(true),
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: ToolsCallResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp.is_error, decoded.is_error);
    }

    #[test]
    fn content_block_image_round_trips() {
        let block = ContentBlock::Image {
            data: "iVBORw0KG…".to_string(),
            mime_type: "image/png".to_string(),
        };
        let bytes = serde_json::to_vec(&block).expect("serialize");
        let decoded: ContentBlock = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(block, decoded);
    }
}
