//! `initialize` method — first request the host sends.
//!
//! The host advertises its protocol version + client info; the server
//! responds with its protocol version + server info + capabilities.

use alloc::collections::BTreeMap;
use alloc::string::String;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `initialize` request params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeRequest {
    /// MCP protocol version the host speaks (tau v0 sends
    /// `"2025-03-26"`).
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Client (host) info.
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
    /// Client capabilities (per MCP spec — free-form map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
}

/// `initialize` response result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResponse {
    /// MCP protocol version the server speaks.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Server info.
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    /// Server capabilities (per MCP spec — free-form map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
}

/// Host-side client info (tau ships `name="tau"`, version = crate version).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client name (`"tau"` for tau-mcp).
    pub name: String,
    /// Client version (tau crate version string).
    pub version: String,
    /// Additional fields the server may report; preserved across
    /// (de)serialization.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub additional: BTreeMap<String, Value>,
}

/// Server-side info reported by the MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server name (e.g. `"weather-mcp"`).
    pub name: String,
    /// Server version string.
    pub version: String,
    /// Additional fields the server may report; preserved across
    /// (de)serialization.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub additional: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::json;

    #[test]
    fn initialize_request_round_trips() {
        let req = InitializeRequest {
            protocol_version: "2025-03-26".to_string(),
            client_info: ClientInfo {
                name: "tau".to_string(),
                version: "0.0.0".to_string(),
                additional: BTreeMap::new(),
            },
            capabilities: Some(json!({"roots":{"listChanged":false}})),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: InitializeRequest = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
    }

    #[test]
    fn initialize_response_round_trips() {
        let resp = InitializeResponse {
            protocol_version: "2025-03-26".to_string(),
            server_info: ServerInfo {
                name: "weather".to_string(),
                version: "1.2.3".to_string(),
                additional: BTreeMap::new(),
            },
            capabilities: Some(json!({"tools":{"listChanged":false}})),
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: InitializeResponse = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn server_info_preserves_additional_fields() {
        // Real servers report extra fields tau doesn't know about; they
        // must round-trip without loss.
        let wire = json!({
            "name":"weather","version":"1.0","author":"NWS","website":"https://weather.gov"
        });
        let info: ServerInfo = serde_json::from_value(wire.clone()).expect("decode");
        assert_eq!(info.name, "weather");
        assert_eq!(info.additional.get("author").and_then(Value::as_str), Some("NWS"));
        let reencoded = serde_json::to_value(&info).expect("encode");
        assert_eq!(reencoded, wire);
    }
}
