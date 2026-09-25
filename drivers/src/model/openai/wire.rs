//! The OpenAI-compatible column of the provider mapping (ADR-0006 §6):
//! bridge types to chat-completions JSON and back. Pure — no I/O, no clock —
//! so every branch is a unit test.
//!
//! | Bridge | OpenAI-compatible chat |
//! |---|---|
//! | `system` | first message, role `system` |
//! | `text` block | `content` string; several blocks join with a newline |
//! | `tool_call` block | `tool_calls[]` (`id`, `function.name`, `function.arguments` as a JSON *string*) |
//! | `tool_result` block | one message per result, role `tool`, `tool_call_id`; `is_error` folded into the text |
//! | `tools[]` | `tools[]` (`type: function`, `function.{name,description,parameters}`) |
//! | `max_tokens` | `max_tokens` or `max_completion_tokens`, clamped (see [`OutputCap`]) |
//! | `sampling.seed` | `seed` |
//! | `sampling.stop_sequences` | `stop` |
//! | `stop` | `finish_reason`: `stop`→`end_turn`, `tool_calls`→`tool_call`, `length`→`max_tokens`, `content_filter`→`refusal` |
//! | `usage` | `usage.prompt_tokens`, `usage.completion_tokens` |
//! | `thinking` block, `provider: openai-compatible` | request ← reply: the message's `reasoning` (Ollama) or `reasoning_content` (vLLM), verbatim, first in `content`. Reply → request: **dropped** (read-only, ADR-0007 §2) |
//! | `thinking` block, other provider | dropped (ADR-0007 §2) |
//!
//! A `user` turn is split: every `tool_result` becomes a `tool` message
//! first, in order, so they sit right after the assistant's `tool_calls`
//! as the endpoint requires; any text left becomes one `user` message after
//! them. The server's in-band reasoning is read, never replayed: no
//! chat-completions server asks for it back, so a `thinking` block in a
//! request is dropped whatever its `provider` — another provider's is
//! unreadable here by definition, and this driver's own is display-only
//! (ADR-0007 §2, amended by #123).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_kernel::bridge::{
    Content, ErrorKind, Message, ModelError, ModelReply, ModelRequest, Role, StopReason, Usage,
};

use super::OutputCap;
use crate::model::ceiling::clamp_max_tokens;

/// The `provider` tag this driver writes on the `thinking` block it seals
/// from a reply's in-band reasoning. Never replayed: see the module docs.
pub const PROVIDER: &str = "openai-compatible";

/// One chat-completions request body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Request {
    pub(super) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) max_completion_tokens: Option<u32>,
    pub(super) messages: Vec<RequestMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) stop: Vec<String>,
}

/// One message, as the provider frames it: the role picks the shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub(super) enum RequestMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        /// `null` when the turn is tool calls only.
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// One tool call, in a request or a response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ToolCall {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) function: FunctionCall,
}

/// The function half of a tool call. `arguments` is a JSON string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FunctionCall {
    pub(super) name: String,
    pub(super) arguments: String,
}

/// A tool definition, as the provider wants it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Tool {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) function: FunctionDef,
}

/// The function half of a tool definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FunctionDef {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: Value,
}

/// One chat-completions response body (HTTP 200).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Response {
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) choices: Vec<Choice>,
    #[serde(default)]
    pub(super) usage: ResponseUsage,
}

/// One choice. The driver never asks for more than one.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Choice {
    pub(super) message: ResponseMessage,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
}

/// The assistant's message. The server's in-band reasoning arrives under
/// one of two keys — `reasoning` on Ollama, `reasoning_content` on vLLM —
/// and either becomes the reply's `thinking` block; both are optional.
/// Fields the bridge has no slot for (`refusal`, `logprobs`) are ignored.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct ResponseMessage {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) reasoning: Option<Value>,
    #[serde(default)]
    pub(super) reasoning_content: Option<Value>,
    #[serde(default)]
    pub(super) tool_calls: Vec<ToolCall>,
}

impl ResponseMessage {
    /// The reasoning the server sent, verbatim, under whichever key it
    /// used: `reasoning_content` first, then `reasoning`. `None` when both
    /// are absent, `null`, or the empty string — a server that reasons but
    /// answered a trivial prompt sends `""`, which is no thinking at all.
    fn reasoning(&self) -> Option<&Value> {
        [&self.reasoning_content, &self.reasoning]
            .into_iter()
            .flatten()
            .find(|v| !v.is_null() && v.as_str() != Some(""))
    }
}

/// What the provider counted. Cached-token details are ignored: the ceiling
/// prices at the uncached rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub(super) struct ResponseUsage {
    #[serde(default)]
    pub(super) prompt_tokens: u64,
    #[serde(default)]
    pub(super) completion_tokens: u64,
}

impl From<ResponseUsage> for Usage {
    fn from(usage: ResponseUsage) -> Self {
        Self {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
        }
    }
}

/// The provider's own description of what went wrong. OpenAI wraps it as
/// `{"error":{"type","message"}}`; vLLM sends it flat as
/// `{"object":"error","message","type","code"}`. Both are read.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct ErrorDetail {
    #[serde(rename = "type", default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) message: String,
}

#[derive(Deserialize)]
struct WrappedError {
    error: ErrorDetail,
}

/// Reads an error body in either shape; `None` if it is neither, or says
/// nothing.
pub(super) fn parse_error(body: &[u8]) -> Option<ErrorDetail> {
    if let Ok(wrapped) = serde_json::from_slice::<WrappedError>(body) {
        return Some(wrapped.error);
    }
    serde_json::from_slice::<ErrorDetail>(body)
        .ok()
        .filter(|flat| !flat.message.is_empty())
}

/// Maps a bridge request to a provider request, or refuses it.
///
/// Every sampling field passes through: this column has a `seed`.
/// `max_tokens` is clamped to `max_max_tokens` and carried in the field
/// `output_cap` names. Refuses (`unsupported`) a block in a turn that has no
/// chat-completions shape: a `tool_call` in a `user` turn, a `tool_result`
/// in an `assistant` turn. A `thinking` block is dropped whatever its
/// `provider`: another provider's is unreadable here, and this driver's own
/// is read-only (ADR-0007 §2). The bridge version is the caller's check;
/// this function assumes a request it can read.
pub(super) fn to_provider(
    request: &ModelRequest,
    model: &str,
    max_max_tokens: u32,
    output_cap: OutputCap,
) -> Result<Request, ModelError> {
    let sampling = request.sampling.clone().unwrap_or_default();
    let mut messages = Vec::with_capacity(request.messages.len() + 1);
    if let Some(system) = &request.system {
        messages.push(RequestMessage::System {
            content: system.clone(),
        });
    }
    for message in &request.messages {
        message_to_provider(message, &mut messages)?;
    }
    let tools = request
        .tools
        .iter()
        .map(|t| Tool {
            kind: "function".to_owned(),
            function: FunctionDef {
                name: t.name.as_str().to_owned(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect();
    let cap = clamp_max_tokens(request.max_tokens, max_max_tokens);
    let (max_tokens, max_completion_tokens) = match output_cap {
        OutputCap::MaxTokens => (Some(cap), None),
        OutputCap::MaxCompletionTokens => (None, Some(cap)),
    };
    Ok(Request {
        model: model.to_owned(),
        max_tokens,
        max_completion_tokens,
        messages,
        tools,
        temperature: sampling.temperature,
        top_p: sampling.top_p,
        seed: sampling.seed,
        stop: sampling.stop_sequences,
    })
}

fn message_to_provider(message: &Message, out: &mut Vec<RequestMessage>) -> Result<(), ModelError> {
    let mut texts: Vec<&str> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut results = Vec::new();
    for block in &message.content {
        match (message.role, block) {
            (_, Content::Text { text }) => texts.push(text),
            // Dropped whatever its `provider` (ADR-0007 §2): another
            // provider's reasoning is unreadable here by definition, and
            // this driver's own is read-only — no chat-completions server
            // takes `reasoning` / `reasoning_content` back on the next turn.
            (_, Content::Thinking { .. }) => {}
            (Role::Assistant, Content::ToolCall { id, name, input }) => tool_calls.push(ToolCall {
                id: id.clone(),
                kind: "function".to_owned(),
                function: FunctionCall {
                    name: name.clone(),
                    arguments: input.to_string(),
                },
            }),
            // `error_kind` has no provider slot and is dropped, as §4 says;
            // `is_error` becomes a prefix the model can read.
            (
                Role::User,
                Content::ToolResult {
                    call_id,
                    content,
                    is_error,
                    error_kind: _,
                },
            ) => results.push(RequestMessage::Tool {
                tool_call_id: call_id.clone(),
                content: if *is_error {
                    format!("error: {content}")
                } else {
                    content.clone()
                },
            }),
            (Role::User, Content::ToolCall { .. }) => {
                return Err(unsupported(
                    "a tool_call block in a user turn has no chat-completions shape",
                ))
            }
            (Role::Assistant, Content::ToolResult { .. }) => {
                return Err(unsupported(
                    "a tool_result block in an assistant turn has no chat-completions shape",
                ))
            }
        }
    }
    let text = (!texts.is_empty()).then(|| texts.join("\n"));
    out.extend(results);
    match message.role {
        Role::User => {
            if let Some(content) = text {
                out.push(RequestMessage::User { content });
            }
        }
        Role::Assistant => {
            if text.is_some() || !tool_calls.is_empty() {
                out.push(RequestMessage::Assistant {
                    content: text,
                    tool_calls,
                });
            }
        }
    }
    Ok(())
}

/// Maps a provider response to the bridge, or says why it could not.
///
/// A 200 the driver cannot map — no choices, a tool call whose `arguments`
/// is not JSON or whose `type` is not `function`, an unknown or missing
/// `finish_reason` — is `error.provider`. The usage is real either way; the
/// caller bills it regardless of which arm this returns. Only the first
/// choice is read: the driver never asks for another.
///
/// The server's in-band reasoning, when the message carries any, is sealed
/// as one `thinking` block ahead of the text and the tool calls — the
/// order the model produced them in — with `provider` [`PROVIDER`] and
/// `data` the field's value verbatim (a JSON string on every server seen
/// so far). The program can read it; the driver never sends it back.
pub(super) fn to_bridge(response: &Response, v: u16) -> Result<ModelReply, ModelError> {
    let choice = response
        .choices
        .first()
        .ok_or_else(|| provider("response has no choices"))?;
    let mut content = Vec::with_capacity(choice.message.tool_calls.len() + 2);
    if let Some(data) = choice.message.reasoning() {
        content.push(Content::Thinking {
            provider: PROVIDER.to_owned(),
            data: data.clone(),
        });
    }
    if let Some(text) = choice.message.content.as_deref().filter(|t| !t.is_empty()) {
        content.push(Content::Text {
            text: text.to_owned(),
        });
    }
    for call in &choice.message.tool_calls {
        if call.kind != "function" {
            return Err(provider(&format!(
                "tool call `{}` has type `{}`, which the bridge has no slot for",
                call.id, call.kind
            )));
        }
        // Some servers send an empty string for a call with no arguments.
        let raw = if call.function.arguments.trim().is_empty() {
            "{}"
        } else {
            call.function.arguments.as_str()
        };
        let input: Value = serde_json::from_str(raw).map_err(|e| {
            provider(&format!(
                "tool call `{}` arguments are not JSON: {e}",
                call.id
            ))
        })?;
        content.push(Content::ToolCall {
            id: call.id.clone(),
            name: call.function.name.clone(),
            input,
        });
    }
    let stop = match choice.finish_reason.as_deref() {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolCall,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Refusal,
        Some(other) => return Err(provider(&format!("unknown finish_reason `{other}`"))),
        None => return Err(provider("response has no finish_reason")),
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

fn unsupported(message: &str) -> ModelError {
    ModelError {
        kind: ErrorKind::Unsupported,
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
    const TOOL_RESULTS: &str =
        include_str!("../../../../kernel/tests/fixtures/bridge/tool-results.json");
    const REQUEST_THINKING: &str =
        include_str!("../../../../kernel/tests/fixtures/bridge/request-thinking.json");
    const OPENAI_REQUEST: &str = include_str!("../../../tests/fixtures/openai/request.json");
    const OPENAI_RESPONSE: &str =
        include_str!("../../../tests/fixtures/openai/response-tool-call.json");

    const MODEL: &str = "Qwen/Qwen3-8B";

    fn bridge_request() -> ModelRequest {
        serde_json::from_str(REQUEST).unwrap()
    }

    #[test]
    fn the_adr_example_request_maps_whole_to_the_recorded_provider_body() {
        // Seed included: this column has one.
        let body = to_provider(&bridge_request(), MODEL, 1_024, OutputCap::MaxTokens).unwrap();
        let expected: Value = serde_json::from_str(OPENAI_REQUEST).unwrap();
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
        let back: Request = serde_json::from_value(expected).unwrap();
        assert_eq!(back, body, "the provider body round-trips");
    }

    #[test]
    fn the_adr_0007_thinking_request_maps_with_its_foreign_blocks_dropped() {
        let request: ModelRequest = serde_json::from_str(REQUEST_THINKING).unwrap();
        let body = to_provider(&request, MODEL, 1_024, OutputCap::MaxTokens).unwrap();
        // The same body as the plain example, which this fixture is plus
        // two `anthropic` thinking blocks and minus the sampling.
        let mut expected: Value = serde_json::from_str(OPENAI_REQUEST).unwrap();
        let fields = expected.as_object_mut().unwrap();
        fields.remove("temperature");
        fields.remove("seed");
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
    }

    #[test]
    fn max_tokens_is_clamped_into_the_configured_field_and_sampling_passes_through() {
        let mut request = bridge_request();
        let sampling = request.sampling.as_mut().unwrap();
        sampling.top_p = Some(0.5);
        sampling.stop_sequences = vec!["END".into()];
        request.max_tokens = 4_096;

        let body = to_provider(&request, "m", 1_024, OutputCap::MaxTokens).unwrap();
        assert_eq!(body.max_tokens, Some(1_024));
        assert_eq!(body.max_completion_tokens, None);
        assert_eq!(body.temperature, Some(0.0));
        assert_eq!(body.top_p, Some(0.5));
        assert_eq!(body.seed, Some(42));
        assert_eq!(body.stop, ["END"]);

        let body = to_provider(&request, "m", 1_024, OutputCap::MaxCompletionTokens).unwrap();
        assert_eq!(body.max_tokens, None);
        assert_eq!(body.max_completion_tokens, Some(1_024));
        let value = serde_json::to_value(&body).unwrap();
        assert!(value.get("max_tokens").is_none());
        assert_eq!(value.get("max_completion_tokens"), Some(&json!(1_024)));
    }

    #[test]
    fn a_request_without_system_sampling_or_tools_serializes_minimally() {
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
            serde_json::to_value(to_provider(&request, "m", 8, OutputCap::MaxTokens).unwrap())
                .unwrap();
        assert_eq!(
            body,
            json!({
                "model": "m",
                "max_tokens": 8,
                "messages": [{"role": "user", "content": "hi"}]
            })
        );
    }

    #[test]
    fn the_adr_tool_results_unfold_into_consecutive_tool_messages_with_errors_folded() {
        let results: Vec<Content> = serde_json::from_str(TOOL_RESULTS).unwrap();
        let request = ModelRequest {
            v: VERSION,
            system: None,
            messages: vec![Message {
                role: Role::User,
                content: results,
            }],
            tools: vec![],
            max_tokens: 8,
            sampling: None,
        };
        let body = to_provider(&request, "m", 8, OutputCap::MaxTokens).unwrap();
        let expected = [
            (
                "call_2",
                "error: unknown tool `serch`; available: search, store",
            ),
            (
                "call_3",
                "error: bad args for `search`: missing field `query`",
            ),
            ("call_4", "error: denied: this agent does not hold `search`"),
            ("call_5", "error: search: upstream timeout"),
        ];
        assert_eq!(body.messages.len(), expected.len());
        for (got, (id, text)) in body.messages.iter().zip(expected) {
            assert_eq!(
                got,
                &RequestMessage::Tool {
                    tool_call_id: id.into(),
                    content: text.into(),
                }
            );
        }
    }

    #[test]
    fn tool_results_go_before_the_turns_text_and_texts_join_with_a_newline() {
        let request = ModelRequest {
            v: VERSION,
            system: None,
            messages: vec![
                Message {
                    role: Role::Assistant,
                    content: vec![
                        Content::ToolCall {
                            id: "c1".into(),
                            name: "a".into(),
                            input: json!({}),
                        },
                        Content::ToolCall {
                            id: "c2".into(),
                            name: "b".into(),
                            input: json!({"x": 1}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![
                        Content::Text {
                            text: "first".into(),
                        },
                        Content::ToolResult {
                            call_id: "c1".into(),
                            content: "one".into(),
                            is_error: false,
                            error_kind: None,
                        },
                        Content::ToolResult {
                            call_id: "c2".into(),
                            content: "two".into(),
                            is_error: false,
                            error_kind: None,
                        },
                        Content::Text {
                            text: "second".into(),
                        },
                    ],
                },
            ],
            tools: vec![],
            max_tokens: 8,
            sampling: None,
        };
        let body = to_provider(&request, "m", 8, OutputCap::MaxTokens).unwrap();
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(
            value.get("messages").unwrap(),
            &json!([
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "a", "arguments": "{}"}},
                    {"id": "c2", "type": "function", "function": {"name": "b", "arguments": "{\"x\":1}"}}
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": "one"},
                {"role": "tool", "tool_call_id": "c2", "content": "two"},
                {"role": "user", "content": "first\nsecond"}
            ])
        );
    }

    #[test]
    fn blocks_in_the_wrong_turn_are_unsupported() {
        let turn = |role, block| ModelRequest {
            v: VERSION,
            system: None,
            messages: vec![Message {
                role,
                content: vec![block],
            }],
            tools: vec![],
            max_tokens: 8,
            sampling: None,
        };
        let call = Content::ToolCall {
            id: "c".into(),
            name: "n".into(),
            input: json!({}),
        };
        let err = to_provider(&turn(Role::User, call), "m", 8, OutputCap::MaxTokens).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("tool_call"), "{}", err.message);

        let result = Content::ToolResult {
            call_id: "c".into(),
            content: "x".into(),
            is_error: false,
            error_kind: None,
        };
        let err =
            to_provider(&turn(Role::Assistant, result), "m", 8, OutputCap::MaxTokens).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("tool_result"), "{}", err.message);
    }

    #[test]
    fn the_recorded_response_maps_to_the_adr_reply_fixture() {
        let response: Response = serde_json::from_str(OPENAI_RESPONSE).unwrap();
        let reply = to_bridge(&response, VERSION).unwrap();
        // The ADR's reply, answered by this column's model.
        let mut expected: Value = serde_json::from_str(REPLY_TOOL_CALL).unwrap();
        expected
            .as_object_mut()
            .unwrap()
            .insert("model".into(), json!(MODEL));
        assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    }

    fn response(finish: Option<&str>, message: Value) -> Response {
        serde_json::from_value(json!({
            "model": "m",
            "choices": [{"index": 0, "message": message, "finish_reason": finish}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
        }))
        .unwrap()
    }

    #[test]
    fn every_finish_reason_maps_and_the_rest_are_provider_errors() {
        let cases = [
            ("stop", StopReason::EndTurn),
            ("tool_calls", StopReason::ToolCall),
            ("length", StopReason::MaxTokens),
            ("content_filter", StopReason::Refusal),
        ];
        let message = json!({"role": "assistant", "content": "ok"});
        for (raw, want) in cases {
            let reply = to_bridge(&response(Some(raw), message.clone()), VERSION).unwrap();
            assert_eq!(reply.stop, want, "{raw}");
            assert_eq!(reply.content, [Content::Text { text: "ok".into() }]);
            assert_eq!(
                reply.usage,
                Usage {
                    input_tokens: 1,
                    output_tokens: 2
                }
            );
        }
        for raw in [Some("function_call"), Some("something_new"), None] {
            let err = to_bridge(&response(raw, message.clone()), VERSION).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Provider, "{raw:?}");
        }
    }

    #[test]
    fn several_tool_calls_become_several_blocks_and_empty_content_is_no_block() {
        let message = json!({
            "role": "assistant",
            "content": null,
            "reasoning_content": "thinking out loud",
            "tool_calls": [
                {"id": "a", "type": "function", "function": {"name": "one", "arguments": "{\"k\":1}"}},
                {"id": "b", "type": "function", "function": {"name": "two", "arguments": ""}}
            ]
        });
        let reply = to_bridge(&response(Some("tool_calls"), message), VERSION).unwrap();
        assert_eq!(
            reply.content,
            [
                Content::Thinking {
                    provider: PROVIDER.into(),
                    data: json!("thinking out loud"),
                },
                Content::ToolCall {
                    id: "a".into(),
                    name: "one".into(),
                    input: json!({"k": 1})
                },
                Content::ToolCall {
                    id: "b".into(),
                    name: "two".into(),
                    input: json!({})
                },
            ]
        );
        let message = json!({"role": "assistant", "content": ""});
        let reply = to_bridge(&response(Some("stop"), message), VERSION).unwrap();
        assert!(reply.content.is_empty());
    }

    /// #123: Ollama's `reasoning` and vLLM's `reasoning_content` each seal
    /// as one `thinking` block ahead of the text; an absent, `null`, or
    /// empty field is no block; when both keys arrive, `reasoning_content`
    /// is the one read.
    #[test]
    fn in_band_reasoning_under_either_key_is_one_thinking_block_first() {
        let thinking = |text: &str| Content::Thinking {
            provider: PROVIDER.into(),
            data: json!(text),
        };
        let answer = Content::Text { text: "391".into() };
        for key in ["reasoning", "reasoning_content"] {
            let message = json!({"role": "assistant", "content": "391", key: "17 times 23"});
            let reply = to_bridge(&response(Some("stop"), message), VERSION).unwrap();
            assert_eq!(
                reply.content,
                [thinking("17 times 23"), answer.clone()],
                "{key}"
            );
        }
        for empty in [json!(null), json!("")] {
            let message = json!({
                "role": "assistant", "content": "391",
                "reasoning": empty, "reasoning_content": empty
            });
            let reply = to_bridge(&response(Some("stop"), message), VERSION).unwrap();
            assert_eq!(reply.content, std::slice::from_ref(&answer), "{empty}");
        }
        let message = json!({
            "role": "assistant", "content": "391",
            "reasoning": "ollama", "reasoning_content": "vllm"
        });
        let reply = to_bridge(&response(Some("stop"), message), VERSION).unwrap();
        assert_eq!(reply.content, [thinking("vllm"), answer]);
    }

    /// #123: the block this driver seals is read-only. An assistant turn
    /// that carries it maps to the same message it would without it, and
    /// neither reasoning key reaches the wire.
    #[test]
    fn the_drivers_own_thinking_block_is_not_sent_back() {
        let mut request: ModelRequest = serde_json::from_str(REQUEST).unwrap();
        request.messages.push(Message {
            role: Role::Assistant,
            content: vec![
                Content::Thinking {
                    provider: PROVIDER.into(),
                    data: json!("17 times 23"),
                },
                Content::Text { text: "391".into() },
            ],
        });
        let mapped = to_provider(&request, "m", 8_000, OutputCap::MaxTokens).unwrap();
        assert_eq!(
            mapped.messages.last(),
            Some(&RequestMessage::Assistant {
                content: Some("391".into()),
                tool_calls: vec![],
            })
        );
        let body = serde_json::to_string(&mapped).unwrap();
        assert!(!body.contains("reasoning"), "{body}");
        assert!(!body.contains("thinking"), "{body}");
    }

    #[test]
    fn bad_arguments_an_unknown_call_type_and_no_choices_are_provider_errors() {
        let message = json!({"role": "assistant", "tool_calls": [
            {"id": "a", "type": "function", "function": {"name": "one", "arguments": "not json"}}
        ]});
        let err = to_bridge(&response(Some("tool_calls"), message), VERSION).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.message.contains("`a`"), "{}", err.message);

        let message = json!({"role": "assistant", "tool_calls": [
            {"id": "a", "type": "custom", "custom": {"name": "one", "input": "x"}, "function": {"name": "one", "arguments": "{}"}}
        ]});
        let err = to_bridge(&response(Some("tool_calls"), message), VERSION).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.message.contains("custom"), "{}", err.message);

        let empty: Response = serde_json::from_value(json!({
            "model": "m", "choices": [],
            "usage": {"prompt_tokens": 1, "completion_tokens": 0}
        }))
        .unwrap();
        let err = to_bridge(&empty, VERSION).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.message.contains("no choices"), "{}", err.message);
    }

    #[test]
    fn both_error_body_shapes_parse_and_nothing_else_does() {
        let wrapped = br#"{"error":{"message":"slow down","type":"rate_limit_error","code":"rate_limit_exceeded"}}"#;
        assert_eq!(
            parse_error(wrapped).unwrap(),
            ErrorDetail {
                kind: "rate_limit_error".into(),
                message: "slow down".into()
            }
        );
        let flat = br#"{"object":"error","message":"model `x` does not exist","type":"NotFoundError","param":null,"code":404}"#;
        assert_eq!(
            parse_error(flat).unwrap(),
            ErrorDetail {
                kind: "NotFoundError".into(),
                message: "model `x` does not exist".into()
            }
        );
        assert_eq!(parse_error(b"{}"), None);
        assert_eq!(parse_error(b"<html>bad gateway</html>"), None);
    }
}
