//! `sampling/createMessage` — server-initiated request asking host to
//! invoke an LLM.
//!
//! Per the β.3 design doc §8.3 and §9: v0 routes this through the
//! agent's `LlmBackend` filtered by the `sampling.models` allowlist.
//! `modelPreferences` is parsed but ignored in v0 (β.3.1 adds it).

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `sampling/createMessage` request — server asks host for an LLM
/// completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingCreateMessageRequest {
    /// Chat-style message history.
    pub messages: Vec<SamplingMessage>,
    /// Model hints (intelligence / speed / cost weighting). v0 ignores;
    /// β.3.1 honors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modelPreferences")]
    pub model_preferences: Option<ModelPreferences>,
    /// Optional system prompt the host should pass to the LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "systemPrompt")]
    pub system_prompt: Option<String>,
    /// Optional inclusion of host-context in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeContext")]
    pub include_context: Option<String>,
    /// Maximum tokens hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<u32>,
    /// Other parameters (temperature, stopSequences, etc.); preserved
    /// across (de)serialization. BTreeMap keeps key order stable for
    /// canonical hashing.
    #[serde(flatten, default, skip_serializing_if = "alloc::collections::BTreeMap::is_empty")]
    pub additional: alloc::collections::BTreeMap<String, Value>,
}

/// A message in a sampling request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingMessage {
    /// Role (`"user"` | `"assistant"` per spec; tau forwards through).
    pub role: String,
    /// Content block — v0 supports text only on inbound sampling.
    pub content: SamplingContent,
}

/// Content block of a sampling message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SamplingContent {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Image (base64 data + mime).
    Image {
        /// Base64-encoded image bytes.
        data: String,
        /// MIME type.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Model-preference hints (parsed in v0; not yet honored).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelPreferences {
    /// Server's hint for intelligence (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "intelligencePriority")]
    pub intelligence_priority: Option<f32>,
    /// Server's hint for speed (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "speedPriority")]
    pub speed_priority: Option<f32>,
    /// Server's hint for cost (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "costPriority")]
    pub cost_priority: Option<f32>,
    /// Server's hint for specific model names (free-form).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<ModelHint>,
}

/// One model-name hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelHint {
    /// Suggested model name.
    pub name: String,
}

/// `sampling/createMessage` response — the host's LLM completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingCreateMessageResponse {
    /// Role of the response (always `"assistant"` per spec).
    pub role: String,
    /// Completion content.
    pub content: SamplingContent,
    /// Model name actually used.
    pub model: String,
    /// Stop reason (`"endTurn"` | `"stopSequence"` | `"maxTokens"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stopReason")]
    pub stop_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn sampling_request_round_trips() {
        let req = SamplingCreateMessageRequest {
            messages: vec![SamplingMessage {
                role: "user".to_string(),
                content: SamplingContent::Text {
                    text: "summarize".to_string(),
                },
            }],
            model_preferences: Some(ModelPreferences {
                intelligence_priority: Some(0.9),
                speed_priority: Some(0.1),
                cost_priority: None,
                hints: vec![ModelHint {
                    name: "claude-haiku".to_string(),
                }],
            }),
            system_prompt: None,
            include_context: None,
            max_tokens: Some(512),
            additional: alloc::collections::BTreeMap::new(),
        };
        let bytes = serde_json::to_vec(&req).expect("serialize");
        let decoded: SamplingCreateMessageRequest =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(req, decoded);
        // Suppress unused-import warning for `json!` macro now that
        // `additional` no longer needs Value construction here.
        let _ = json!({});
    }

    #[test]
    fn sampling_response_round_trips() {
        let resp = SamplingCreateMessageResponse {
            role: "assistant".to_string(),
            content: SamplingContent::Text {
                text: "summary".to_string(),
            },
            model: "claude-haiku-4-5".to_string(),
            stop_reason: Some("endTurn".to_string()),
        };
        let bytes = serde_json::to_vec(&resp).expect("serialize");
        let decoded: SamplingCreateMessageResponse =
            serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(resp, decoded);
    }
}
