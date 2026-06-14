//! v1 built-in context transformers: `trim_old`, `compact_tool_outputs`,
//! `fit_budget`.
//!
//! All three are `DeterminismClass::Pure` and require no additional
//! capabilities. They operate turn-granularly (a *turn* = one
//! `Address::User` / `MessagePayload::Text` message plus the agent
//! messages that follow it). The **live turn** (the last turn) is
//! never dropped or compacted.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use tau_domain::{Address, Message, MessagePayload};

use super::{
    CapabilityNeed, ContextError, ContextFuture, ContextTransformer, DeterminismClass, TransformCx,
};

/// Returns the start index of each turn in `msgs` (ascending).
///
/// A turn begins at every `Address::User` / `MessagePayload::Text` message.
/// If the first such message is not at index 0, an implicit turn starting at
/// index 0 is prepended so that pre-conversation messages (e.g. system-turn
/// injections) are always owned by *some* turn.
fn turn_starts(msgs: &[Message]) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        if matches!(
            (&m.sender, &m.payload),
            (Address::User, MessagePayload::Text { .. })
        ) {
            starts.push(i);
        }
    }
    // Ensure there is always at least one turn starting at 0.
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts
}

/// Empty capability slice shared by all v1 builtins.
const NO_CAPS: &[CapabilityNeed] = &[];

// ---------------------------------------------------------------------------
// trim_old
// ---------------------------------------------------------------------------

/// Drop all but the `keep_last_turns` most-recent turns.
///
/// The live turn (last turn) is always preserved regardless of the setting.
/// A value of `0` is a no-op (keep everything).
///
/// # Configuration (`config` map key `keep_last_turns`)
///
/// | Key | Type | Default |
/// |-----|------|---------|
/// | `keep_last_turns` | u64 | 8 |
#[derive(Debug, Clone)]
pub struct TrimOld {
    /// Number of turns to retain (counting from the end).
    pub keep_last_turns: u32,
}

impl TrimOld {
    /// Build from a transformer `config` map (values are `serde_json::Value`).
    pub fn from_config(cfg: &BTreeMap<String, serde_json::Value>) -> Self {
        let keep = cfg
            .get("keep_last_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(8) as u32;
        Self {
            keep_last_turns: keep,
        }
    }

    /// Pure synchronous implementation (exposed for testing and pipeline
    /// runners that do not need the boxed-future wrapper).
    pub fn apply(&self, msgs: Vec<Message>) -> Vec<Message> {
        if self.keep_last_turns == 0 {
            return msgs;
        }
        let starts = turn_starts(&msgs);
        if (starts.len() as u32) <= self.keep_last_turns {
            return msgs;
        }
        let keep_from_turn = starts.len() - self.keep_last_turns as usize;
        let cut = starts[keep_from_turn];
        msgs.into_iter().skip(cut).collect()
    }
}

impl ContextTransformer for TrimOld {
    fn name(&self) -> &str {
        "trim_old"
    }

    fn determinism(&self) -> DeterminismClass {
        DeterminismClass::Pure
    }

    fn required_capabilities(&self) -> &[CapabilityNeed] {
        NO_CAPS
    }

    fn transform<'a>(&'a self, _cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move { Ok(self.apply(msgs)) })
    }
}

// ---------------------------------------------------------------------------
// compact_tool_outputs
// ---------------------------------------------------------------------------

/// Truncate `ToolResult` bodies in *non-live* turns to `max_bytes`.
///
/// The live turn (last turn) is never touched. Truncated bodies are replaced
/// with a string of the form `"<kept prefix>…[truncated N bytes]…"`.
///
/// # Configuration (`config` map key `max_bytes`)
///
/// | Key | Type | Default |
/// |-----|------|---------|
/// | `max_bytes` | u64 | 1024 |
#[derive(Debug, Clone)]
pub struct CompactToolOutputs {
    /// Maximum serialized byte length allowed for a tool-result body.
    pub max_bytes: usize,
}

impl CompactToolOutputs {
    /// Build from a transformer `config` map.
    pub fn from_config(cfg: &BTreeMap<String, serde_json::Value>) -> Self {
        let max = cfg
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(1024) as usize;
        Self { max_bytes: max }
    }

    /// Pure synchronous implementation.
    pub fn apply(&self, mut msgs: Vec<Message>) -> Vec<Message> {
        let starts = turn_starts(&msgs);
        let live_start = *starts.last().unwrap_or(&0);
        for (i, m) in msgs.iter_mut().enumerate() {
            if i >= live_start {
                break;
            }
            if let MessagePayload::ToolResult { body } = &m.payload {
                let s = serde_json::to_string(body).unwrap_or_default();
                if s.len() > self.max_bytes {
                    let kept: String = s.chars().take(self.max_bytes).collect();
                    let dropped = s.len() - kept.len();
                    let marker = format!("{kept}\u{2026}[truncated {dropped} bytes]\u{2026}");
                    m.payload = MessagePayload::ToolResult {
                        body: tau_domain::Value::String(marker),
                    };
                }
            }
        }
        msgs
    }
}

impl ContextTransformer for CompactToolOutputs {
    fn name(&self) -> &str {
        "compact_tool_outputs"
    }

    fn determinism(&self) -> DeterminismClass {
        DeterminismClass::Pure
    }

    fn required_capabilities(&self) -> &[CapabilityNeed] {
        NO_CAPS
    }

    fn transform<'a>(&'a self, _cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move { Ok(self.apply(msgs)) })
    }
}

// ---------------------------------------------------------------------------
// fit_budget
// ---------------------------------------------------------------------------

/// Drop turns from oldest to newest until the total token estimate fits
/// within `max_tokens`. Must be the last transformer in the pipeline.
///
/// **Protected content** (system prompt + live turn) is never dropped. If
/// the protected content alone exceeds the budget,
/// [`ContextError::BudgetUnsatisfiable`] is returned.
///
/// # Configuration (`config` map key `max_tokens`)
///
/// | Key | Type | Default |
/// |-----|------|---------|
/// | `max_tokens` | u64 | 8192 |
#[derive(Debug, Clone)]
pub struct FitBudget {
    /// Token budget; messages are dropped until the estimate fits.
    pub max_tokens: u32,
}

impl FitBudget {
    /// Build from a transformer `config` map.
    pub fn from_config(cfg: &BTreeMap<String, serde_json::Value>) -> Self {
        let max = cfg
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(8192) as u32;
        Self { max_tokens: max }
    }

    /// Pure synchronous implementation.
    pub fn apply(
        &self,
        cx: &TransformCx<'_>,
        msgs: Vec<Message>,
    ) -> Result<Vec<Message>, ContextError> {
        // System-prompt cost: approximate as ceil(bytes / 4).
        let sys_tokens = cx
            .system_prompt()
            .map(|s| (s.len() as u32).div_ceil(4))
            .unwrap_or(0);

        let starts = turn_starts(&msgs);
        let live_start = *starts.last().unwrap_or(&0);

        let est = |m: &Message| cx.estimate_tokens(m);

        // Protected = system prompt + live turn (never droppable).
        let live_tokens: u32 = msgs[live_start..].iter().map(est).sum();
        let protected = sys_tokens.saturating_add(live_tokens);
        if protected > self.max_tokens {
            return Err(ContextError::BudgetUnsatisfiable {
                protected_tokens: protected,
                max_tokens: self.max_tokens,
            });
        }

        // Walk cut-points from the beginning (oldest turn first).
        let mut cut_turn = 0usize;
        loop {
            let cut = if cut_turn == 0 { 0 } else { starts[cut_turn] };
            let total: u32 = sys_tokens.saturating_add(msgs[cut..].iter().map(est).sum());
            if total <= self.max_tokens || cut >= live_start {
                return Ok(msgs.into_iter().skip(cut).collect());
            }
            cut_turn += 1;
        }
    }
}

impl ContextTransformer for FitBudget {
    fn name(&self) -> &str {
        "fit_budget"
    }

    fn determinism(&self) -> DeterminismClass {
        DeterminismClass::Pure
    }

    fn required_capabilities(&self) -> &[CapabilityNeed] {
        NO_CAPS
    }

    fn transform<'a>(&'a self, cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a> {
        Box::pin(async move { self.apply(cx, msgs) })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::HeuristicEstimator;
    use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};

    fn user(c: &str) -> Message {
        Message::new(
            Address::User,
            Address::Agent(AgentInstanceId::new()),
            MessagePayload::Text { content: c.into() },
        )
    }

    fn assistant(c: &str) -> Message {
        Message::new(
            Address::Agent(AgentInstanceId::new()),
            Address::User,
            MessagePayload::Text { content: c.into() },
        )
    }

    fn tool_result(b: &str) -> Message {
        Message::new(
            Address::Tool("t".into()),
            Address::Agent(AgentInstanceId::new()),
            MessagePayload::ToolResult {
                body: tau_domain::Value::String(b.into()),
            },
        )
    }

    #[test]
    fn trim_old_keeps_last_n_turns() {
        let msgs = alloc::vec![
            user("t1"),
            assistant("a1"),
            user("t2"),
            assistant("a2"),
            user("t3")
        ];
        let out = TrimOld { keep_last_turns: 2 }.apply(msgs);
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0].payload, MessagePayload::Text { content } if content == "t2"));
    }

    #[test]
    fn compact_truncates_prior_tool_output_not_live() {
        let big = "x".repeat(100);
        let msgs = alloc::vec![user("t1"), tool_result(&big), user("t2"), tool_result(&big)];
        let out = CompactToolOutputs { max_bytes: 10 }.apply(msgs);
        let first = serde_json::to_string(match &out[1].payload {
            MessagePayload::ToolResult { body } => body,
            _ => panic!("expected ToolResult at index 1"),
        })
        .unwrap();
        let last = serde_json::to_string(match &out[3].payload {
            MessagePayload::ToolResult { body } => body,
            _ => panic!("expected ToolResult at index 3"),
        })
        .unwrap();
        assert!(
            first.contains("truncated"),
            "prior turn should be truncated: {first}"
        );
        assert!(
            !last.contains("truncated"),
            "live turn should not be truncated: {last}"
        );
    }

    #[test]
    fn fit_budget_drops_until_fits() {
        let big = "x".repeat(400);
        let msgs = alloc::vec![user(&big), assistant(&big), user("live")];
        let cx = TransformCx::pure(&HeuristicEstimator, None);
        let out = FitBudget { max_tokens: 60 }.apply(&cx, msgs).unwrap();
        assert!(matches!(&out[0].payload, MessagePayload::Text { content } if content == "live"));
    }

    #[test]
    fn fit_budget_unsatisfiable_when_live_too_big() {
        let big = "x".repeat(4000);
        let msgs = alloc::vec![user(&big)];
        let cx = TransformCx::pure(&HeuristicEstimator, None);
        let err = FitBudget { max_tokens: 10 }.apply(&cx, msgs).unwrap_err();
        assert!(matches!(err, ContextError::BudgetUnsatisfiable { .. }));
    }
}
