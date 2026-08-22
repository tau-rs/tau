//! Deterministic in-process LLM backend. A real product swaps this for an
//! Anthropic/OpenAI-backed adapter implementing the same `LlmBackend` port.
use std::collections::VecDeque;
use std::sync::Mutex;

use tau_domain::Value;
use tau_ports::{
    batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError,
    StopReason, ToolUse,
};

/// One scripted LLM turn.
pub enum Turn {
    /// Emit a tool-call response (`StopReason::ToolUse`).
    ToolCall {
        /// Provider-supplied tool-use id.
        id: String,
        /// Tool name (matches a tau tool id).
        name: String,
        /// Tool arguments.
        input: Value,
    },
    /// Emit a final text response (`StopReason::EndTurn`).
    Text(String),
}

/// Replays `turns` FIFO; an exhausted script ends the turn cleanly.
pub struct ScriptedLlmBackend {
    turns: Mutex<VecDeque<Turn>>,
}

impl ScriptedLlmBackend {
    pub fn new(turns: Vec<Turn>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
        }
    }

    fn next_response(&self) -> CompletionResponse {
        let turn = self
            .turns
            .lock()
            .expect("ScriptedLlmBackend mutex poisoned")
            .pop_front();
        match turn {
            Some(Turn::ToolCall { id, name, input }) => CompletionResponse::new(
                String::new(),
                vec![ToolUse::new(id, name, input)],
                StopReason::ToolUse,
                None,
            ),
            Some(Turn::Text(text)) => {
                CompletionResponse::new(text, Vec::new(), StopReason::EndTurn, None)
            }
            None => CompletionResponse::new(String::new(), Vec::new(), StopReason::EndTurn, None),
        }
    }
}

impl LlmBackend for ScriptedLlmBackend {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(self.next_response())
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        Ok(batch_to_stream(self.next_response()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::{LlmBackend, StopReason};

    #[tokio::test]
    async fn scripted_backend_replays_turns_in_order() {
        let b = ScriptedLlmBackend::new(vec![
            Turn::ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                input: serde_json::from_value(serde_json::json!({"text": "hi"})).unwrap(),
            },
            Turn::Text("done".into()),
        ]);
        let req = tau_ports::CompletionRequest::new("m".into());
        let first = b.complete(req.clone()).await.unwrap();
        assert_eq!(first.stop_reason, StopReason::ToolUse);
        assert_eq!(first.tool_uses.len(), 1);
        let second = b.complete(req).await.unwrap();
        assert_eq!(second.stop_reason, StopReason::EndTurn);
        assert_eq!(second.text, "done");
    }
}
