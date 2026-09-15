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
//! | `thinking` block, `provider: anthropic` | `data` verbatim, in place (reply: `thinking` / `redacted_thinking` wrapped, in order) |
//! | `thinking` block, other provider | *dropped* (ADR-0007 §2) |
//! | `tools[]` | `tools[]` (`name`, `description`, `input_schema`) |
//! | `max_tokens` | `max_tokens`, clamped |
//! | `sampling.seed` | *unsupported* → `error.unsupported` |
//! | `sampling.temperature` | `temperature` if *config* `sampling: Accepted`; else *unsupported* → `error.unsupported` |
//! | `sampling.top_p` | `top_p` if *config* `sampling: Accepted`; else *unsupported* → `error.unsupported` |
//! | `sampling.stop_sequences` | `stop_sequences` |
//! | `stop` | `end_turn`, `tool_use`→`tool_call`, `max_tokens`, `stop_sequence`, `refusal` |
//! | `usage` | `usage.input_tokens`, `usage.output_tokens` |
//! | *config* `thinking: Disabled` | `thinking: {"type": "disabled"}` |
//!
//! `pause_turn` only arises with server-side tools, which the bridge does
//! not declare; one that arrives anyway is `error.provider`. Thinking blocks
//! are sealed: the reply wraps each one as the provider wrote it, and the
//! request unwraps it and sends it back unchanged, because the provider
//! rejects a turn whose thinking was edited or dropped (ADR-0007).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_kernel::bridge::{
    Content, ErrorKind, Message, ModelError, ModelReply, ModelRequest, Role, StopReason, Usage,
};

use super::{SamplingMode, ThinkingMode};
use crate::model::ceiling::clamp_max_tokens;

/// The `provider` tag this driver writes on, and replays, thinking blocks.
pub const PROVIDER: &str = "anthropic";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) thinking: Option<ThinkingParam>,
}

/// The `thinking` request parameter. Only the form the config can ask for:
/// omitted means the model's default, which is thinking on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ThinkingParam {
    Disabled,
}

/// The body of `POST /v1/messages/count_tokens`: the main body minus what
/// the endpoint does not take. It accepts `model`, `messages`, `system`
/// and `tools`; `max_tokens` and the sampling knobs are not in its schema,
/// so they are not sent. Borrowed from a [`Request`] so the two bodies
/// cannot drift apart.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct CountRequest<'a> {
    pub(super) model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) system: Option<&'a str>,
    pub(super) messages: &'a [RequestMessage],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub(super) tools: &'a [Tool],
    /// Thinking changes the prompt the provider builds, so the count
    /// carries the same setting as the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thinking: Option<&'a ThinkingParam>,
}

impl<'a> From<&'a Request> for CountRequest<'a> {
    fn from(request: &'a Request) -> Self {
        Self {
            model: &request.model,
            system: request.system.as_deref(),
            messages: &request.messages,
            tools: &request.tools,
            thinking: request.thinking.as_ref(),
        }
    }
}

/// What `count_tokens` answers: `{"input_tokens": N}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct CountResponse {
    pub(super) input_tokens: u64,
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
    /// A sealed block sent back as the provider wrote it: `thinking` or
    /// `redacted_thinking`, unwrapped from a bridge `thinking` block.
    #[serde(untagged)]
    Sealed(Value),
}

/// A tool definition, as the provider wants it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Tool {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input_schema: Value,
}

/// One Messages API response body (HTTP 200).
///
/// `content` is kept raw: a thinking block goes into the bridge exactly as
/// it arrived, so the value is what is wrapped, not a re-serialization of
/// a typed copy of it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct Response {
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) content: Vec<Value>,
    #[serde(default)]
    pub(super) stop_reason: Option<String>,
    pub(super) usage: ResponseUsage,
}

/// A content block on the way out, by its tag. Anything the bridge has no
/// slot for is `Other`.
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
    /// Thinking and its redacted form: sealed into a bridge `thinking`
    /// block, the raw value verbatim (ADR-0007). The fields are not read.
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
/// controlled. Refuses a present `sampling.temperature` or `sampling.top_p`
/// the same way unless `sampling` is [`SamplingMode::Accepted`]: the model
/// would answer 400, and a knob that was quietly dropped is the same quiet
/// loss of control. `max_tokens` is clamped to `max_max_tokens`. A `thinking`
/// block of this provider is unwrapped and sent verbatim; one of another
/// provider is dropped (ADR-0007 §2). The bridge version is the caller's
/// check; this function assumes a current request.
pub(super) fn to_provider(
    request: &ModelRequest,
    model: &str,
    max_max_tokens: u32,
    thinking: ThinkingMode,
    sampling_mode: SamplingMode,
) -> Result<Request, ModelError> {
    let sampling = request.sampling.clone().unwrap_or_default();
    if sampling.seed.is_some() {
        return Err(ModelError {
            kind: ErrorKind::Unsupported,
            message: "sampling.seed is not supported by the Anthropic Messages API".into(),
        });
    }
    if sampling_mode == SamplingMode::Refused {
        let present = [
            ("temperature", sampling.temperature.is_some()),
            ("top_p", sampling.top_p.is_some()),
        ];
        if let Some((field, _)) = present.iter().find(|(_, is_present)| *is_present) {
            return Err(ModelError {
                kind: ErrorKind::Unsupported,
                message: format!(
                    "sampling.{field} is not accepted by model `{model}`; \
                     set AnthropicConfig::sampling to Accepted for a model that takes it"
                ),
            });
        }
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
        thinking: match thinking {
            ThinkingMode::ProviderDefault => None,
            ThinkingMode::Disabled => Some(ThinkingParam::Disabled),
        },
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
        .filter_map(|block| match block {
            Content::Text { text } => Some(RequestBlock::Text { text: text.clone() }),
            Content::ToolCall { id, name, input } => Some(RequestBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            // `error_kind` has no provider slot and is dropped, as §4 says.
            Content::ToolResult {
                call_id,
                content,
                is_error,
                error_kind: _,
            } => Some(RequestBlock::ToolResult {
                tool_use_id: call_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            }),
            // Ours goes back exactly as it came; another provider's
            // reasoning is unreadable here and is dropped (ADR-0007 §2).
            Content::Thinking { provider, data } => {
                (provider == PROVIDER).then(|| RequestBlock::Sealed(data.clone()))
            }
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
    for raw in &response.content {
        let block: ResponseBlock = serde_json::from_value(raw.clone())
            .map_err(|e| provider(&format!("response content block could not be read: {e}")))?;
        match block {
            ResponseBlock::Text { text } => content.push(Content::Text { text }),
            ResponseBlock::ToolUse { id, name, input } => {
                content.push(Content::ToolCall { id, name, input });
            }
            ResponseBlock::Thinking | ResponseBlock::RedactedThinking => {
                content.push(Content::Thinking {
                    provider: PROVIDER.to_owned(),
                    data: raw.clone(),
                });
            }
            ResponseBlock::Other => {
                return Err(provider(
                    "response carries a content block the bridge has no slot for",
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
    const REQUEST_THINKING: &str =
        include_str!("../../../../kernel/tests/fixtures/bridge/request-thinking.json");
    const REPLY_THINKING: &str =
        include_str!("../../../../kernel/tests/fixtures/bridge/reply-thinking.json");
    const ANTHROPIC_REQUEST_THINKING: &str =
        include_str!("../../../tests/fixtures/anthropic/request-thinking.json");
    const ANTHROPIC_RESPONSE_THINKING: &str =
        include_str!("../../../tests/fixtures/anthropic/response-thinking.json");

    const DEFAULT: ThinkingMode = ThinkingMode::ProviderDefault;
    const REFUSED: SamplingMode = SamplingMode::Refused;
    const ACCEPTED: SamplingMode = SamplingMode::Accepted;

    fn bridge_request() -> ModelRequest {
        serde_json::from_str(REQUEST).unwrap()
    }

    #[test]
    fn the_adr_example_request_is_refused_for_its_seed() {
        let err =
            to_provider(&bridge_request(), "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("seed"), "{}", err.message);
    }

    #[test]
    fn the_adr_example_request_minus_seed_maps_to_the_recorded_provider_body() {
        let mut request = bridge_request();
        let sampling = request.sampling.as_mut().unwrap();
        sampling.seed = None;
        sampling.temperature = None;
        let body = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap();
        let expected: Value = serde_json::from_str(ANTHROPIC_REQUEST).unwrap();
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
        let back: Request = serde_json::from_value(expected).unwrap();
        assert_eq!(back, body, "the provider body round-trips");
    }

    #[test]
    fn temperature_and_top_p_are_refused_unless_the_config_says_the_model_takes_them() {
        // The ADR's example minus its seed carries `temperature: 0.0`, which
        // every current model rejects: refused by default, naming the field
        // and the model.
        let mut request = bridge_request();
        request.sampling.as_mut().unwrap().seed = None;
        let err = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("temperature"), "{}", err.message);
        assert!(err.message.contains("claude-opus-5"), "{}", err.message);

        let sampling = request.sampling.as_mut().unwrap();
        sampling.temperature = None;
        sampling.top_p = Some(0.5);
        let err = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("top_p"), "{}", err.message);
        assert!(err.message.contains("claude-opus-5"), "{}", err.message);

        // Both present: the first field is the one named; nothing is built.
        let sampling = request.sampling.as_mut().unwrap();
        sampling.temperature = Some(0.0);
        let err = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap_err();
        assert!(err.message.contains("temperature"), "{}", err.message);

        // Stop sequences alone are fine on a refusing model.
        let sampling = request.sampling.as_mut().unwrap();
        sampling.temperature = None;
        sampling.top_p = None;
        sampling.stop_sequences = vec!["END".into()];
        let body = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap();
        assert_eq!(body.temperature, None);
        assert_eq!(body.top_p, None);
        assert_eq!(body.stop_sequences, ["END"]);

        // An accepting model gets both, unchanged.
        let sampling = request.sampling.as_mut().unwrap();
        sampling.temperature = Some(0.0);
        sampling.top_p = Some(0.5);
        let body = to_provider(&request, "claude-opus-4-6", 1_024, DEFAULT, ACCEPTED).unwrap();
        assert_eq!(body.temperature, Some(0.0));
        assert_eq!(body.top_p, Some(0.5));
        assert_eq!(
            SamplingMode::default(),
            REFUSED,
            "refuse unless told otherwise"
        );
    }

    #[test]
    fn max_tokens_is_clamped_and_sampling_passes_through() {
        let mut request = bridge_request();
        let sampling = request.sampling.as_mut().unwrap();
        sampling.seed = None;
        sampling.top_p = Some(0.5);
        sampling.stop_sequences = vec!["END".into()];
        request.max_tokens = 4_096;
        let body = to_provider(&request, "m", 1_024, DEFAULT, ACCEPTED).unwrap();
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
        let body =
            serde_json::to_value(to_provider(&request, "m", 8, DEFAULT, REFUSED).unwrap()).unwrap();
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
    fn the_count_body_is_the_main_body_minus_what_count_tokens_rejects() {
        let mut request = bridge_request();
        let sampling = request.sampling.as_mut().unwrap();
        sampling.seed = None;
        sampling.top_p = Some(0.5);
        sampling.stop_sequences = vec!["END".into()];
        let body = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, ACCEPTED).unwrap();
        assert_eq!(
            body.temperature,
            Some(0.0),
            "the knobs are in the main body"
        );
        let count = serde_json::to_value(CountRequest::from(&body)).unwrap();

        let mut expected: Value = serde_json::from_str(ANTHROPIC_REQUEST).unwrap();
        let object = expected.as_object_mut().unwrap();
        object.remove("max_tokens");
        assert_eq!(count, expected);
        let keys: Vec<&str> = count
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["messages", "model", "system", "tools"]);
    }

    #[test]
    fn the_count_body_omits_what_the_request_did_not_carry() {
        let body = Request {
            model: "m".into(),
            max_tokens: 8,
            system: None,
            messages: vec![],
            tools: vec![],
            temperature: None,
            top_p: None,
            stop_sequences: vec![],
            thinking: None,
        };
        let count = serde_json::to_value(CountRequest::from(&body)).unwrap();
        assert_eq!(count, json!({"model": "m", "messages": []}));
    }

    #[test]
    fn the_count_response_parses_and_rejects_the_wrong_shape() {
        let count: CountResponse = serde_json::from_value(json!({"input_tokens": 2095})).unwrap();
        assert_eq!(count.input_tokens, 2_095);
        assert!(serde_json::from_value::<CountResponse>(json!({"tokens": 1})).is_err());
        assert!(serde_json::from_value::<CountResponse>(json!({"input_tokens": "1"})).is_err());
        assert!(serde_json::from_value::<CountResponse>(json!({"input_tokens": -1})).is_err());
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
    fn thinking_blocks_are_sealed_verbatim_and_unknown_blocks_refused() {
        // Whatever fields the provider puts in, including ones this driver
        // has never heard of, the sealed block is the raw value.
        let thinking =
            json!({"type": "thinking", "thinking": "", "signature": "abc", "new_field": 1});
        let redacted = json!({"type": "redacted_thinking", "data": "xyz"});
        let content = json!([thinking, redacted, {"type": "text", "text": "ok"}]);
        let reply = to_bridge(&response(Some("end_turn"), content), VERSION).unwrap();
        assert_eq!(
            reply.content,
            [
                Content::Thinking {
                    provider: "anthropic".into(),
                    data: thinking
                },
                Content::Thinking {
                    provider: "anthropic".into(),
                    data: redacted
                },
                Content::Text { text: "ok".into() },
            ]
        );

        let content = json!([{"type": "server_tool_use", "id": "x", "name": "web_search"}]);
        let err = to_bridge(&response(Some("end_turn"), content), VERSION).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);

        // A block with a known tag but the wrong shape is unreadable, too.
        let content = json!([{"type": "text", "no_text": true}]);
        let err = to_bridge(&response(Some("end_turn"), content), VERSION).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
    }

    #[test]
    fn the_recorded_thinking_response_maps_to_the_adr_0007_reply_fixture() {
        let response: Response = serde_json::from_str(ANTHROPIC_RESPONSE_THINKING).unwrap();
        let reply = to_bridge(&response, VERSION).unwrap();
        let expected: Value = serde_json::from_str(REPLY_THINKING).unwrap();
        assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    }

    #[test]
    fn a_request_replays_our_thinking_blocks_in_place_and_drops_foreign_ones() {
        let mut request: ModelRequest = serde_json::from_str(REQUEST_THINKING).unwrap();
        let body = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap();
        let expected: Value = serde_json::from_str(ANTHROPIC_REQUEST_THINKING).unwrap();
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
        let back: Request = serde_json::from_value(expected).unwrap();
        assert_eq!(
            back, body,
            "the provider body round-trips, sealed blocks included"
        );

        // The same two blocks, but written by some other provider's driver:
        // this driver cannot replay them, and the provider would reject
        // them, so they are dropped and the turn is otherwise unchanged.
        let assistant = request.messages.get_mut(1).unwrap();
        for block in &mut assistant.content {
            if let Content::Thinking { provider, .. } = block {
                *provider = "someone-else".into();
            }
        }
        let body = to_provider(&request, "claude-opus-5", 1_024, DEFAULT, REFUSED).unwrap();
        let turn = body.messages.get(1).unwrap();
        assert!(
            !turn
                .content
                .iter()
                .any(|b| matches!(b, RequestBlock::Sealed(_))),
            "{turn:?}"
        );
        assert_eq!(turn.content.len(), 2, "text and tool_use remain");
    }

    #[test]
    fn thinking_off_is_one_parameter_and_the_default_is_none() {
        let mut request = bridge_request();
        request.sampling = None;
        let on = to_provider(&request, "m", 8, ThinkingMode::ProviderDefault, REFUSED).unwrap();
        assert_eq!(on.thinking, None);
        assert!(
            serde_json::to_value(&on).unwrap().get("thinking").is_none(),
            "omitted, not null"
        );
        let off = to_provider(&request, "m", 8, ThinkingMode::Disabled, REFUSED).unwrap();
        assert_eq!(
            serde_json::to_value(&off).unwrap().get("thinking"),
            Some(&json!({"type": "disabled"}))
        );
        // The count is sized with the same setting the call is made with.
        assert_eq!(
            serde_json::to_value(CountRequest::from(&off))
                .unwrap()
                .get("thinking"),
            Some(&json!({"type": "disabled"}))
        );
        assert!(serde_json::to_value(CountRequest::from(&on))
            .unwrap()
            .get("thinking")
            .is_none());
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
