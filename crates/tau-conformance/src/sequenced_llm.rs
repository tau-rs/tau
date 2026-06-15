//! Scripted `LlmBackend` for conformance runs.
//!
//! [`SequencedLlm`] pops pre-recorded [`CompletionResponse`] values from a
//! queue in order, replaying a `mock_llm.jsonl` script against the IR
//! interpreter. [`parse_mock_llm`] decodes that JSONL into the response
//! sequence. Ported from `tau-ir-conformance::dev_mode` so DevMode and
//! WasmProfile drive the IR interpreter through the exact same in-process
//! scripted backend and consume identical script bytes.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::Value as JsonValue;

use tau_ports::error::LlmError;
use tau_ports::llm::{
    batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend,
    StopReason,
};
use tau_ports::ToolUse;

// ---------------------------------------------------------------------------
// SequencedLlm — pops scripted responses in order
// ---------------------------------------------------------------------------

/// A [`LlmBackend`] that pops scripted [`CompletionResponse`] values from a
/// queue in the order they were added. Used to replay a `mock_llm.jsonl`
/// script against the IR interpreter.
///
/// Once the queue is exhausted, [`SequencedLlm::complete`] returns
/// [`LlmError::Internal`] — a fixture that drives more turns than it
/// scripts is a fixture bug and should surface loudly, not silently.
pub(crate) struct SequencedLlm {
    name: String,
    queue: Mutex<VecDeque<CompletionResponse>>,
}

impl SequencedLlm {
    pub(crate) fn new(name: impl Into<String>, responses: Vec<CompletionResponse>) -> Self {
        Self {
            name: name.into(),
            queue: Mutex::new(responses.into()),
        }
    }
}

impl LlmBackend for SequencedLlm {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.queue
            .lock()
            .expect("SequencedLlm mutex poisoned")
            .pop_front()
            .ok_or_else(|| LlmError::Internal {
                message: "SequencedLlm: no more scripted responses".into(),
            })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let resp = LlmBackend::complete(self, req).await?;
        Ok(batch_to_stream(resp))
    }
}

// ---------------------------------------------------------------------------
// mock_llm.jsonl parser
// ---------------------------------------------------------------------------

/// One line of `mock_llm.jsonl`. Represents one scripted LLM response.
///
/// ```json
/// {"turn": 0, "response": {"tool_uses": [...], "stop_reason": "tool_use"}}
/// {"turn": 1, "response": {"text": "ok", "stop_reason": "end_turn"}}
/// ```
#[derive(serde::Deserialize)]
struct MockLlmLine {
    response: MockLlmResponse,
}

#[derive(serde::Deserialize)]
struct MockLlmResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool_uses: Vec<MockToolUse>,
    stop_reason: String,
}

#[derive(serde::Deserialize)]
struct MockToolUse {
    id: String,
    name: String,
    #[serde(default)]
    input: JsonValue,
}

/// Parse `mock_llm.jsonl` into a vec of [`CompletionResponse`] values in
/// turn order. Turn order is preserved by file order; the `turn` field in
/// each line is advisory only. Blank lines are skipped; lines that fail to
/// deserialize are dropped.
pub(crate) fn parse_mock_llm(jsonl: &str) -> Vec<CompletionResponse> {
    let lines: Vec<MockLlmLine> = jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    lines
        .into_iter()
        .map(|line| {
            let resp = line.response;
            let tool_uses: Vec<ToolUse> = resp
                .tool_uses
                .into_iter()
                .map(|tu| {
                    let input: tau_domain::Value =
                        serde_json::from_value(tu.input).unwrap_or(tau_domain::Value::Null);
                    tau_ports::fixtures::make_tool_use(tu.id, tu.name, input)
                })
                .collect();

            let stop_reason = match resp.stop_reason.as_str() {
                "tool_use" => StopReason::ToolUse,
                _ => StopReason::EndTurn,
            };

            tau_ports::fixtures::make_completion_response(resp.text, tool_uses, stop_reason, None)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::LlmBackend;

    fn test_text_response(text: &str, stop_reason: &str) -> CompletionResponse {
        let stop = match stop_reason {
            "tool_use" => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        tau_ports::fixtures::make_completion_response(text.to_string(), Vec::new(), stop, None)
    }

    fn test_request() -> CompletionRequest {
        tau_ports::fixtures::make_completion_request("mock-model".to_string())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pops_responses_in_order_then_errors() {
        let r0 = test_text_response("hi", "end_turn");
        let llm = SequencedLlm::new("mock-llm", vec![r0]);
        let _got = llm.complete(test_request()).await.expect("first response");
        assert!(
            llm.complete(test_request()).await.is_err(),
            "exhausted queue errors"
        );
    }

    #[test]
    fn parse_mock_llm_reads_turn_order() {
        let jsonl = "{\"response\": {\"text\": \"a\", \"stop_reason\": \"end_turn\"}}\n";
        let v = parse_mock_llm(jsonl);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn parse_mock_llm_decodes_tool_use() {
        let jsonl = "{\"turn\": 0, \"response\": {\"tool_uses\": [{\"id\": \"t1\", \"name\": \"fan\", \"input\": {\"x\": 1}}], \"stop_reason\": \"tool_use\"}}\n{\"turn\": 1, \"response\": {\"text\": \"done\", \"stop_reason\": \"end_turn\"}}\n";
        let v = parse_mock_llm(jsonl);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].stop_reason, StopReason::ToolUse);
        assert_eq!(v[0].tool_uses.len(), 1);
        assert_eq!(v[0].tool_uses[0].name, "fan");
        assert_eq!(v[1].stop_reason, StopReason::EndTurn);
        assert_eq!(v[1].text, "done");
    }

    #[test]
    fn parse_mock_llm_skips_blank_lines() {
        let jsonl = "\n{\"response\": {\"text\": \"a\", \"stop_reason\": \"end_turn\"}}\n\n";
        assert_eq!(parse_mock_llm(jsonl).len(), 1);
    }
}
