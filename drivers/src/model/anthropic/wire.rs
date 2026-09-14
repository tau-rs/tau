//! The Anthropic column of the provider mapping (ADR-0006 §6): bridge types
//! to Messages API JSON and back. Pure — no I/O, no clock — so every branch
//! is a unit test.
//!
//! | Bridge | Anthropic Messages |
//! |---|---|
//! | `system` | top-level `system` |
//! | `text` block | `text` block |
//! | `tool_call` block | `tool_use` (`id`, `name`, `input`) |
//! | `tool_result` block | `tool_result` (`tool_use_id`, `content`, `is_error`) |
//! | `tools[]` | `tools[]` (`name`, `description`, `input_schema`) |
//! | `max_tokens` | `max_tokens`, clamped |
//! | `sampling.seed` | *unsupported* → `error.unsupported` |
//! | `sampling.stop_sequences` | `stop_sequences` |
//! | `stop` | `end_turn`, `tool_use`→`tool_call`, `max_tokens`, `stop_sequence`, `refusal` |
//! | `usage` | `usage.input_tokens`, `usage.output_tokens` |
//!
//! `pause_turn` only arises with server-side tools, which v1 does not
//! declare; one that arrives anyway is `error.provider`. `thinking` blocks in
//! a reply have no slot in bridge v1 (thinking is driver configuration, not
//! in v1) and are dropped; #42 tracks the amendment.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_kernel::bridge::{
    Content, ErrorKind, Message, ModelError, ModelReply, ModelRequest, Role, StopReason, Usage,
};

use super::estimate::clamp_max_tokens;

/// One Messages API request body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Request {
    pub(super) model: String,
    pub(super) max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) system: Option<String>,
    pub(super) messages: Vec<RequestMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) stop_sequences: Vec<String>,
}

/// One turn, as the provider frames it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RequestMessage {
    pub(super) role: String,
    pub(super) content: Vec<RequestBlock>,
}

/// A content block on the way in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum RequestBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

/// A tool definition, as the provider wants it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Tool {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input_schema: Value,
}

/// One Messages API response body (HTTP 200).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct Response {
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) content: Vec<ResponseBlock>,
    #[serde(default)]
    pub(super) stop_reason: Option<String>,
    pub(super) usage: ResponseUsage,
}

/// A content block on the way out. Anything v1 has no slot for is `Other`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ResponseBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Thinking and its redacted form: driver configuration in v1, no bridge
    /// slot, dropped on the way out.
    Thinking,
    RedactedThinking,
    #[serde(other)]
    Other,
}

/// What the provider counted. Cache fields are ignored: the driver never sets
/// `cache_control`, and the ceiling prices at the uncached rate anyway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub(super) struct ResponseUsage {
    #[serde(default)]
    pub(super) input_tokens: u64,
    #[serde(default)]
    pub(super) output_tokens: u64,
}

impl From<ResponseUsage> for Usage {
    fn from(usage: ResponseUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        }
    }
}

/// The body of a non-2xx answer: `{"type":"error","error":{"type","message"}}`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct ErrorBody {
    pub(super) error: ErrorDetail,
}

/// The provider's own description of what went wrong.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct ErrorDetail {
    #[serde(rename = "type", default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) message: String,
}

/// Maps a bridge request to a provider request, or refuses it.
///
/// Refuses (`unsupported`) a present `sampling.seed`: Anthropic has no seed,
/// and a seed that was quietly ignored is a branch that was quietly not
/// controlled. `max_tokens` is clamped to `max_max_tokens`. The bridge
/// version is the caller's check; this function assumes a v1 request.
pub(super) fn to_provider(
    request: &ModelRequest,
    model: &str,
    max_max_tokens: u32,
) -> Result<Request, ModelError> {
    let sampling = request.sampling.clone().unwrap_or_default();
    if sampling.seed.is_some() {
        return Err(ModelError {
            kind: ErrorKind::Unsupported,
            message: "sampling.seed is not supported by the Anthropic Messages API".into(),
        });
    }
    let messages = request.messages.iter().map(message_to_provider).collect();
    let tools = request
        .tools
        .iter()
        .map(|t| Tool {
            name: t.name.as_str().to_owned(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect();
    Ok(Request {
        model: model.to_owned(),
        max_tokens: clamp_max_tokens(request.max_tokens, max_max_tokens),
        system: request.system.clone(),
        messages,
        tools,
        temperature: sampling.temperature,
        top_p: sampling.top_p,
        stop_sequences: sampling.stop_sequences,
    })
}

fn message_to_provider(message: &Message) -> RequestMessage {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content = message
        .content
        .iter()
        .map(|block| match block {
            Content::Text { text } => RequestBlock::Text { text: text.clone() },
            Content::ToolCall { id, name, input } => RequestBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
            // `error_kind` has no provider slot and is dropped, as §4 says.
            Content::ToolResult {
                call_id,
                content,
                is_error,
                error_kind: _,
            } => RequestBlock::ToolResult {
                tool_use_id: call_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            },
        })
        .collect();
    RequestMessage {
        role: role.to_owned(),
        content,
    }
}

/// Maps a provider response to the bridge, or says why it could not.
///
/// A 200 the driver cannot map — an unknown block type, an unknown or missing
/// stop reason, or `pause_turn` — is `error.provider`. The usage is real
/// either way; the caller bills it regardless of which arm this returns.
pub(super) fn to_bridge(response: &Response, v: u16) -> Result<ModelReply, ModelError> {
    let mut content = Vec::with_capacity(response.content.len());
    for block in &response.content {
        match block {
            ResponseBlock::Text { text } => content.push(Content::Text { text: text.clone() }),
            ResponseBlock::ToolUse { id, name, input } => content.push(Content::ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            ResponseBlock::Thinking | ResponseBlock::RedactedThinking => {}
            ResponseBlock::Other => {
                return Err(provider(
                    "response carries a content block bridge v1 has no slot for",
                ));
            }
        }
    }
    let stop = match response.stop_reason.as_deref() {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolCall,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("stop_sequence") => StopReason::StopSequence,
        Some("refusal") => StopReason::Refusal,
        Some("pause_turn") => {
            return Err(provider(
                "stop_reason pause_turn: server-side tools are not part of bridge v1",
            ))
        }
        Some(other) => return Err(provider(&format!("unknown stop_reason `{other}`"))),
        None => return Err(provider("response has no stop_reason")),
    };
    Ok(ModelReply {
        v,
        model: response.model.clone(),
        content,
        stop,
        usage: response.usage.into(),
    })
}

fn provider(message: &str) -> ModelError {
    ModelError {
        kind: ErrorKind::Provider,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;
    use tau_kernel::bridge::VERSION;

    use super::*;

    const REQUEST: &str = include_str!("../../../../kernel/tests/fixtures/bridge/request.json");
    const REPLY_TOOL_CALL: &str =
        include_str!("../../../../kernel/tests/fixtures/bridge/reply-tool-call.json");
    const ANTHROPIC_REQUEST: &str = include_str!("../../../tests/fixtures/anthropic/request.json");
    const ANTHROPIC_RESPONSE: &str =
        include_str!("../../../tests/fixtures/anthropic/response-tool-use.json");

    fn bridge_request() -> ModelRequest {
        serde_json::from_str(REQUEST).unwrap()
    }

    #[test]
    fn the_adr_example_request_is_refused_for_its_seed() {
        let err = to_provider(&bridge_request(), "claude-opus-5", 1_024).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("seed"), "{}", err.message);
    }

    #[test]
    fn the_adr_example_request_minus_seed_maps_to_the_recorded_provider_body() {
        let mut request = bridge_request();
        request.sampling.as_mut().unwrap().seed = None;
        let body = to_provider(&request, "claude-opus-5", 1_024).unwrap();
        let expected: Value = serde_json::from_str(ANTHROPIC_REQUEST).unwrap();
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
        let back: Request = serde_json::from_value(expected).unwrap();
        assert_eq!(back, body, "the provider body round-trips");
    }

    #[test]
    fn max_tokens_is_clamped_and_sampling_passes_through() {
        let mut request = bridge_request();
        let sampling = request.sampling.as_mut().unwrap();
        sampling.seed = None;
        sampling.top_p = Some(0.5);
        sampling.stop_sequences = vec!["END".into()];
        request.max_tokens = 4_096;
        let body = to_provider(&request, "m", 1_024).unwrap();
        assert_eq!(body.max_tokens, 1_024);
        assert_eq!(body.temperature, Some(0.0));
        assert_eq!(body.top_p, Some(0.5));
        assert_eq!(body.stop_sequences, ["END"]);
    }

    #[test]
    fn a_request_without_sampling_or_tools_serializes_minimally() {
        let request = ModelRequest {
            v: VERSION,
            system: None,
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text { text: "hi".into() }],
            }],
            tools: vec![],
            max_tokens: 8,
            sampling: None,
        };
        let body = serde_json::to_value(to_provider(&request, "m", 8).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "m",
                "max_tokens": 8,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
            })
        );
    }

    #[test]
    fn the_recorded_response_maps_to_the_adr_reply_fixture() {
        let response: Response = serde_json::from_str(ANTHROPIC_RESPONSE).unwrap();
        let reply = to_bridge(&response, VERSION).unwrap();
        let expected: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
        assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    }

    fn response(stop: Option<&str>, content: Value) -> Response {
        serde_json::from_value(json!({
            "model": "m",
            "content": content,
            "stop_reason": stop,
            "usage": {"input_tokens": 1, "output_tokens": 2, "cache_read_input_tokens": 0}
        }))
        .unwrap()
    }

    #[test]
    fn every_stop_reason_maps_and_the_rest_are_provider_errors() {
        let cases = [
            ("end_turn", StopReason::EndTurn),
            ("tool_use", StopReason::ToolCall),
            ("max_tokens", StopReason::MaxTokens),
            ("stop_sequence", StopReason::StopSequence),
            ("refusal", StopReason::Refusal),
        ];
        for (raw, want) in cases {
            let reply = to_bridge(&response(Some(raw), json!([])), VERSION).unwrap();
            assert_eq!(reply.stop, want, "{raw}");
            assert_eq!(
                reply.usage,
                Usage {
                    input_tokens: 1,
                    output_tokens: 2
                }
            );
        }
        for raw in [Some("pause_turn"), Some("something_new"), None] {
            let err = to_bridge(&response(raw, json!([])), VERSION).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Provider, "{raw:?}");
        }
    }

    #[test]
    fn thinking_blocks_are_dropped_and_unknown_blocks_refused() {
        let content = json!([
            {"type": "thinking", "thinking": "", "signature": "abc"},
            {"type": "redacted_thinking", "data": "xyz"},
            {"type": "text", "text": "ok"}
        ]);
        let reply = to_bridge(&response(Some("end_turn"), content), VERSION).unwrap();
        assert_eq!(reply.content, [Content::Text { text: "ok".into() }]);

        let content = json!([{"type": "server_tool_use", "id": "x", "name": "web_search"}]);
        let err = to_bridge(&response(Some("end_turn"), content), VERSION).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
    }

    #[test]
    fn the_provider_error_body_parses() {
        let body: ErrorBody = serde_json::from_value(json!({
            "type": "error",
            "error": {"type": "rate_limit_error", "message": "slow down"},
            "request_id": "req_1"
        }))
        .unwrap();
        assert_eq!(body.error.kind, "rate_limit_error");
        assert_eq!(body.error.message, "slow down");
    }
}
