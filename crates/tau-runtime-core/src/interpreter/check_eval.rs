//! Deterministic goal-predicate evaluation (C1) and LLM judge invocation
//! (C2.2). Pure predicate evaluation over already-materialized locus bytes —
//! the caller (pipeline executor) reads the artifact or named output first,
//! then calls these. Judge invocation is async (drives an agent loop) and
//! returns the parsed `Verdict` plus token usage.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;

use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_ir::{Agent, AgentId, IrModule, Predicate};
use tau_ir::check::JudgeRef;

use crate::error::RuntimeError;
use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::interpreter::verdict::{parse_verdict, Verdict};
use crate::options::TokenUsage;
use crate::outcome::RunOutcome;

// ---------------------------------------------------------------------------
// Built-in judge constants
// ---------------------------------------------------------------------------

/// Default model for the built-in minimalist judge when no `judge_model`
/// override is given.
const DEFAULT_JUDGE_MODEL: &str = "claude-haiku-4-5";

/// Canonical system prompt for the built-in minimalist judge. Instructs
/// the strict `{ met, rationale }` JSON verdict shape (D5).
const BUILTIN_JUDGE_PROMPT: &str = "You are a strict, terse deliverable evaluator. \
You are given a CRITERION and an ARTIFACT. Decide whether the artifact satisfies the criterion. \
Respond with ONLY a JSON object and nothing else, in exactly this shape: \
{\"met\": <true|false>, \"rationale\": \"<one sentence explaining why>\"}. \
Do not include any prose outside the JSON object.";

// ---------------------------------------------------------------------------
// run_judge — public(crate) entry
// ---------------------------------------------------------------------------

/// Run the judge for a deliverable check and parse its verdict.
///
/// `Builtin` synthesises a minimalist judge agent (canonical prompt +
/// chosen/default model, no tools); `Agent` runs the named user agent.
/// The criterion + artifact are passed as the user turn. Returns the
/// parsed [`Verdict`] (an unparseable judge reply is a soft `met=false`)
/// and the judge run's token usage for budget accounting.
pub(crate) async fn run_judge<D>(
    module: Arc<IrModule>,
    dispatcher: Arc<D>,
    judge: &JudgeRef,
    must_satisfy: &str,
    artifact: Option<&[u8]>,
) -> Result<(Verdict, TokenUsage), RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let artifact_text = match artifact {
        Some(b) => String::from_utf8_lossy(b).into_owned(),
        None => "<artifact missing>".to_string(),
    };
    let user_turn = alloc::format!(
        "CRITERION:\n{must_satisfy}\n\nARTIFACT:\n{artifact_text}"
    );

    // Resolve the judge agent (synthesised builtin, or a clone of the named
    // agent). Both variants produce an `Agent` with no tool_refs so the inner
    // agent loop never reaches the dispatcher's `invoke`.
    let agent: Agent = match judge {
        JudgeRef::Builtin { model } => Agent {
            id: AgentId("__builtin_judge".to_string()),
            prompt: BUILTIN_JUDGE_PROMPT.to_string(),
            model: model.clone().unwrap_or_else(|| DEFAULT_JUDGE_MODEL.to_string()),
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: tau_ir::AgentBudget::default(),
        },
        JudgeRef::Agent(id) => module
            .workflow
            .agents
            .get(id)
            .cloned()
            .ok_or_else(|| RuntimeError::Internal {
                message: alloc::format!(
                    "judge agent {} not found (typecheck should have caught this)",
                    id.0
                ),
            })?,
    };

    let initial = vec![user_message_judge(&user_turn)];
    let outcome = alloc::boxed::Box::pin(run_agent(module.clone(), &agent, dispatcher, initial)).await?;
    let usage = match &outcome {
        RunOutcome::Completed { token_usage, .. } => *token_usage,
        RunOutcome::Failed { token_usage, .. } => *token_usage,
    };
    let text = last_assistant_text(&outcome);
    Ok((parse_verdict(&text), usage))
}

/// User-turn message carrying the judge criterion + artifact (mirrors
/// `pipeline.rs::user_message`).
fn user_message_judge(content: &str) -> Message {
    Message::new(
        Address::User,
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text {
            content: content.to_string(),
        },
    )
}

/// Evaluate a goal predicate against the materialized locus bytes.
///
/// `bytes` is `None` when the locus does not exist (e.g. a missing file
/// or an absent named output). Returns `(passed, rationale)`; on pass the
/// rationale is a short confirmation, on fail it explains why.
///
/// Does NOT handle [`Predicate::NativeFn`] — that variant is dispatched
/// through the `DeterministicRegistry` by the pipeline executor (C1.3),
/// never here; this returns `(false, ...)` defensively if reached.
pub(crate) fn eval_predicate(pred: &Predicate, bytes: Option<&[u8]>) -> (bool, String) {
    match pred {
        Predicate::Exists => match bytes {
            Some(_) => (true, "locus exists".to_string()),
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::NonEmpty => match bytes {
            Some(b) if !b.is_empty() => (true, "locus is non-empty".to_string()),
            Some(_) => (false, "locus is empty".to_string()),
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::Equals(expected) => match bytes {
            Some(b) => {
                let actual = String::from_utf8_lossy(b);
                if actual == expected.as_str() {
                    (true, "locus equals expected".to_string())
                } else {
                    (false, "locus does not equal expected value".to_string())
                }
            }
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::Matches(pattern) => match_count(pattern, bytes).map_or_else(
            |e| (false, e),
            |(count, _)| {
                if count >= 1 {
                    (true, "pattern matched".to_string())
                } else {
                    (false, format!("pattern {pattern:?} did not match"))
                }
            },
        ),
        Predicate::MinCount { pattern, min } => match_count(pattern, bytes).map_or_else(
            |e| (false, e),
            |(count, _)| {
                if count as u64 >= *min {
                    (true, format!("{count} matches >= {min}"))
                } else {
                    (false, format!("{count} matches of {pattern:?}, need >= {min}"))
                }
            },
        ),
        Predicate::SchemaValid(schema) => match bytes {
            Some(b) => match serde_json::from_slice::<serde_json::Value>(b) {
                Ok(instance) => match jsonschema::options().build(schema) {
                    Ok(validator) => {
                        if validator.is_valid(&instance) {
                            (true, "instance is schema-valid".to_string())
                        } else {
                            (false, "instance does not satisfy schema".to_string())
                        }
                    }
                    Err(e) => (false, format!("invalid schema: {e}")),
                },
                Err(e) => (false, format!("locus is not valid JSON: {e}")),
            },
            None => (false, "locus does not exist".to_string()),
        },
        Predicate::NativeFn(_) => (
            false,
            "native-fn predicate must be dispatched via the deterministic registry".to_string(),
        ),
    }
}

/// Count regex matches in the (utf8-lossy) bytes. Returns `Err(message)` on
/// missing locus or invalid regex.
fn match_count(pattern: &str, bytes: Option<&[u8]>) -> Result<(usize, ()), String> {
    let b = bytes.ok_or_else(|| "locus does not exist".to_string())?;
    let text = String::from_utf8_lossy(b);
    let re = regex::Regex::new(pattern)
        .map_err(|e| format!("invalid regex {pattern:?}: {e}"))?;
    Ok((re.find_iter(&text).count(), ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_true_and_false() {
        let (ok, _) = eval_predicate(
            &Predicate::Matches("(?m)^## Sources".to_string()),
            Some(b"## Sources\nfoo"),
        );
        assert!(ok);
        let (ok2, _) = eval_predicate(
            &Predicate::Matches("(?m)^## Sources".to_string()),
            Some(b"no header"),
        );
        assert!(!ok2);
    }

    #[test]
    fn non_empty() {
        assert!(eval_predicate(&Predicate::NonEmpty, Some(b"x")).0);
        assert!(!eval_predicate(&Predicate::NonEmpty, Some(b"")).0);
        assert!(!eval_predicate(&Predicate::NonEmpty, None).0);
    }

    #[test]
    fn min_count_boundary() {
        let p = Predicate::MinCount {
            pattern: "a".to_string(),
            min: 2,
        };
        assert!(!eval_predicate(&p, Some(b"a")).0);
        assert!(eval_predicate(&p, Some(b"aa")).0);
    }

    #[test]
    fn exists_and_missing() {
        assert!(eval_predicate(&Predicate::Exists, Some(b"")).0);
        assert!(!eval_predicate(&Predicate::Exists, None).0);
    }

    #[test]
    fn equals_match_and_mismatch() {
        let p = Predicate::Equals("hello".to_string());
        assert!(eval_predicate(&p, Some(b"hello")).0);
        assert!(!eval_predicate(&p, Some(b"world")).0);
        assert!(!eval_predicate(&p, None).0);
    }

    #[test]
    fn schema_valid_pass_and_fail() {
        let schema = serde_json::json!({ "type": "object", "required": ["name"] });
        let p = Predicate::SchemaValid(schema);
        assert!(eval_predicate(&p, Some(br#"{"name":"tau"}"#)).0);
        assert!(!eval_predicate(&p, Some(br#"{"other":"x"}"#)).0);
        assert!(!eval_predicate(&p, Some(b"not-json")).0);
        assert!(!eval_predicate(&p, None).0);
    }

    #[test]
    fn native_fn_is_rejected() {
        let (ok, msg) = eval_predicate(&Predicate::NativeFn("check_links".to_string()), Some(b"x"));
        assert!(!ok);
        assert!(msg.contains("deterministic registry"));
    }

    #[test]
    fn matches_missing_locus() {
        let (ok, msg) = eval_predicate(&Predicate::Matches("foo".to_string()), None);
        assert!(!ok);
        assert!(msg.contains("does not exist"));
    }

    #[test]
    fn invalid_regex_error() {
        let (ok, msg) = eval_predicate(&Predicate::Matches("[invalid".to_string()), Some(b"text"));
        assert!(!ok);
        assert!(msg.contains("invalid regex"));
    }
}

// ---------------------------------------------------------------------------
// run_judge unit tests (async, require test-fixtures for MockLlmBackend)
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "test-fixtures"))]
mod run_judge_tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::sync::Arc;
    use core::future::Future;
    use core::pin::Pin;

    use serde_json::Value;
    use tau_ir::{
        capability::CapabilityTable,
        check::JudgeRef,
        ids::AgentId,
        module::{IrFormatVersion, IrModule, Workflow},
    };
    use tau_ports::fixtures::{make_completion_response, MockLlmBackend};
    use tau_ports::{CompletionResponse, StopReason};

    use crate::builder::DynLlmBackend;
    use crate::error::RuntimeError;
    use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

    // ── minimal ToolDispatcher backed by a MockLlmBackend ──────────────────

    struct JudgeTestDispatcher {
        backend: Arc<dyn DynLlmBackend>,
    }

    impl ToolDispatcher for JudgeTestDispatcher {
        fn invoke<'a>(
            &'a self,
            _tool_id: &'a tau_ir::ToolId,
            _args: &'a Value,
        ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
        {
            // The judge agent carries no tools; this must never be called.
            Box::pin(async move {
                Err(RuntimeError::Internal {
                    message: "JudgeTestDispatcher::invoke must not be called (judge has no tools)"
                        .into(),
                })
            })
        }

        fn llm_backend(&self) -> Arc<dyn DynLlmBackend> {
            self.backend.clone()
        }
    }

    /// Build a minimal `IrModule` with no agents, no tools — sufficient for a
    /// `JudgeRef::Builtin` judge (which synthesises its own agent).
    fn empty_module() -> Arc<IrModule> {
        let target = tau_ports::target::registry::list_available()
            .next()
            .expect("at least one available target")
            .triple;
        Arc::new(IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: env!("CARGO_PKG_VERSION").into(),
            target,
            triggers: alloc::vec::Vec::new(),
            workflow: Workflow {
                agents: BTreeMap::new(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                edges: alloc::vec::Vec::new(),
                capability_table: CapabilityTable(BTreeMap::new()),
                pipeline: None,
                checks: BTreeMap::new(),
            },
        })
    }

    /// Build a `CompletionResponse` with `text` as the final assistant text
    /// (no tool calls, EndTurn, no usage) — the canonically safe pattern for
    /// `#[non_exhaustive]` types using `make_completion_response`.
    fn text_response(text: &str) -> CompletionResponse {
        make_completion_response(
            text.to_string(),
            alloc::vec::Vec::new(),
            StopReason::EndTurn,
            None,
        )
    }

    // ── test cases ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_judge_builtin_met_true_on_valid_json() {
        // Backend replies with a valid `{"met": true, "rationale": "ok"}`.
        let mock = MockLlmBackend::new("mock-judge").with_response(text_response(
            r#"{"met": true, "rationale": "ok"}"#,
        ));
        let dispatcher = Arc::new(JudgeTestDispatcher {
            backend: Arc::new(mock),
        });

        let (verdict, _usage) = run_judge(
            empty_module(),
            dispatcher,
            &JudgeRef::Builtin { model: None },
            "the artifact must be non-empty",
            Some(b"hello world"),
        )
        .await
        .expect("run_judge must succeed");

        assert!(verdict.met, "verdict.met should be true");
        assert_eq!(verdict.rationale, "ok");
    }

    #[tokio::test]
    async fn run_judge_builtin_met_false_on_prose_reply() {
        // Backend replies with prose (no JSON) — parse_verdict soft-fails to met=false.
        let mock = MockLlmBackend::new("mock-judge")
            .with_response(text_response("I think it looks fine, honestly."));
        let dispatcher = Arc::new(JudgeTestDispatcher {
            backend: Arc::new(mock),
        });

        let (verdict, _usage) = run_judge(
            empty_module(),
            dispatcher,
            &JudgeRef::Builtin { model: None },
            "the artifact must be non-empty",
            Some(b"hello world"),
        )
        .await
        .expect("run_judge must succeed even on prose reply");

        assert!(!verdict.met, "prose reply should produce met=false");
        assert!(
            verdict.rationale.contains("unparseable verdict"),
            "rationale should explain parse failure; got: {}",
            verdict.rationale
        );
    }

    #[tokio::test]
    async fn run_judge_missing_artifact_passes_placeholder() {
        // When `artifact` is `None`, the judge receives "<artifact missing>".
        // The backend still returns valid JSON here — we just verify the call
        // succeeds without panicking.
        let mock = MockLlmBackend::new("mock-judge").with_response(text_response(
            r#"{"met": false, "rationale": "artifact missing"}"#,
        ));
        let dispatcher = Arc::new(JudgeTestDispatcher {
            backend: Arc::new(mock),
        });

        let (verdict, _usage) = run_judge(
            empty_module(),
            dispatcher,
            &JudgeRef::Builtin { model: None },
            "must exist",
            None,
        )
        .await
        .expect("run_judge with None artifact must succeed");

        assert!(!verdict.met);
    }

    #[tokio::test]
    async fn run_judge_agent_not_found_returns_error() {
        // `JudgeRef::Agent` referencing a missing agent must return RuntimeError::Internal.
        let mock = MockLlmBackend::new("mock-judge");
        let dispatcher = Arc::new(JudgeTestDispatcher {
            backend: Arc::new(mock),
        });

        let err = run_judge(
            empty_module(),
            dispatcher,
            &JudgeRef::Agent(AgentId("no-such-agent".to_string())),
            "crit",
            Some(b"artifact"),
        )
        .await
        .expect_err("missing agent must produce RuntimeError");

        match err {
            RuntimeError::Internal { message } => {
                assert!(
                    message.contains("no-such-agent"),
                    "error message should name the missing agent; got: {message}"
                );
            }
            other => panic!("expected RuntimeError::Internal, got {other:?}"),
        }
    }
}
