//! Check evaluation: `evaluate_goal` (deterministic predicate via registry)
//! and `evaluate_deliverable` (existence floor + LLM judge).
//!
//! Task 19 wires both into `run_pipeline`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;

use serde_json::{json, Value};
use tau_ir::check::{GoalPredicate, JudgeRef, Locus};
use tau_ir::{Agent, AgentBudget, AgentId, IrModule};

use crate::error::RuntimeError;
use crate::interpreter::agent_loop::{last_assistant_text, run_agent};
use crate::interpreter::artifact::ArtifactReader;
use crate::interpreter::deterministic::DeterministicRegistry;
use crate::interpreter::output_store::OutputStore;
use crate::interpreter::pipeline::user_message;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::vocabulary::{
    FN_BUILTIN_EQUALS, FN_BUILTIN_EXISTS, FN_BUILTIN_MATCHES, FN_BUILTIN_MIN_COUNT,
    FN_BUILTIN_NON_EMPTY, FN_BUILTIN_SCHEMA_VALID,
};

/// Outcome of evaluating a postcondition check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckVerdict {
    /// Whether the postcondition held.
    pub met: bool,
    /// Human-readable rationale (load-bearing — fed into the retry loop).
    pub rationale: String,
}

/// Resolve a locus to `(present, content)`.
///
/// Returns `(true, Some(string))` when the locus resolves to content,
/// or `(false, None)` when the path/step-output does not exist.
pub(crate) fn resolve_locus(
    locus: &Locus,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
) -> Result<(bool, Option<String>), RuntimeError> {
    match locus {
        Locus::Output(step_id) => Ok(match store.get(&step_id.0) {
            Some(v) => (true, Some(value_to_string(v))),
            None => (false, None),
        }),
        Locus::Path(p) => {
            let r = reader.ok_or_else(|| RuntimeError::Internal {
                message: format!("check needs an artifact reader to read {p}"),
            })?;
            Ok(match r.read_path(p)? {
                Some(bytes) => (true, Some(String::from_utf8_lossy(&bytes).into_owned())),
                None => (false, None),
            })
        }
    }
}

/// Map a `GoalPredicate` to `(fn_name, extra_args_object)`.
fn predicate_call(p: &GoalPredicate) -> (&str, Value) {
    match p {
        GoalPredicate::Exists => (FN_BUILTIN_EXISTS, json!({})),
        GoalPredicate::NonEmpty => (FN_BUILTIN_NON_EMPTY, json!({})),
        GoalPredicate::Equals(x) => (FN_BUILTIN_EQUALS, json!({ "equals": x })),
        GoalPredicate::Matches(x) => (FN_BUILTIN_MATCHES, json!({ "pattern": x })),
        GoalPredicate::MinCount(n) => (FN_BUILTIN_MIN_COUNT, json!({ "min_count": n })),
        GoalPredicate::SchemaValid(v) => (FN_BUILTIN_SCHEMA_VALID, json!({ "schema": v })),
        GoalPredicate::NativeFn(fn_ref) => (fn_ref.name.as_str(), json!({})),
    }
}

/// Convert an `OutputStore` value to a string, mirroring `OutputStore::template_map`.
///
/// `Value::String` passes through; all other values are compact-JSON encoded.
pub(crate) fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Normalize a registry response into a `CheckVerdict`.
///
/// Accepts:
/// - `Value::Bool(b)` — synthesizes a rationale.
/// - An object `{"met": bool, "rationale": "..."}` — uses those fields.
/// - Anything else — treat as not-met with a synthesized rationale.
fn normalize_verdict(raw: Value, predicate: &GoalPredicate) -> CheckVerdict {
    match raw {
        Value::Bool(b) => CheckVerdict {
            met: b,
            rationale: if b {
                format!("goal predicate {predicate:?} returned true")
            } else {
                format!("goal predicate {predicate:?} returned false")
            },
        },
        Value::Object(ref map) => {
            let met = map.get("met").and_then(|v| v.as_bool()).unwrap_or(false);
            let rationale = map
                .get("rationale")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if met {
                        format!("goal predicate {predicate:?} returned true")
                    } else {
                        format!("goal predicate {predicate:?} returned false")
                    }
                });
            CheckVerdict { met, rationale }
        }
        other => CheckVerdict {
            met: false,
            rationale: format!("goal predicate {predicate:?} returned unexpected shape: {other}"),
        },
    }
}

/// Evaluate a goal predicate via the deterministic registry.
///
/// Resolves the locus to `(present, content)`, builds a JSON args object
/// `{ present, content, ...predicate_params }`, invokes the predicate fn
/// through `registry`, and normalizes the result into a [`CheckVerdict`].
pub fn evaluate_goal(
    evaluates: &Locus,
    predicate: &GoalPredicate,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
    registry: &dyn DeterministicRegistry,
) -> Result<CheckVerdict, RuntimeError> {
    let (present, content) = resolve_locus(evaluates, store, reader)?;
    let (fn_name, mut args) = predicate_call(predicate);

    // Inject the standard `present` and `content` fields into the args object.
    if let Value::Object(ref mut m) = args {
        m.insert("present".into(), Value::Bool(present));
        m.insert(
            "content".into(),
            content.map(Value::String).unwrap_or(Value::Null),
        );
    }

    let raw = registry.invoke(fn_name, &args)?;
    Ok(normalize_verdict(raw, predicate))
}

/// Build the canonical built-in judge prompt embedding `must_satisfy`.
///
/// Instructs the model to reply ONLY with JSON `{"met": bool, "rationale": string}`.
fn builtin_judge_prompt(must_satisfy: &str) -> String {
    format!(
        "You are a strict reviewer. Judge whether the artifact below satisfies this \
         criterion:\n\n{must_satisfy}\n\nReply ONLY with JSON: {{\"met\": bool, \"rationale\": string}}."
    )
}

/// Parse a judge reply into a [`CheckVerdict`].
///
/// Deserializes `{"met": bool, "rationale": string}` from `text.trim()`.
/// On parse failure the raw text becomes the rationale and `met` is `false`.
fn parse_verdict(text: &str) -> CheckVerdict {
    #[derive(serde::Deserialize)]
    struct Raw {
        met: bool,
        #[serde(default)]
        rationale: String,
    }
    match serde_json::from_str::<Raw>(text.trim()) {
        Ok(r) => CheckVerdict {
            met: r.met,
            rationale: r.rationale,
        },
        Err(_) => CheckVerdict {
            met: false,
            rationale: String::from(text),
        },
    }
}

/// Evaluate a deliverable: existence floor then LLM judge.
///
/// 1. Resolves `locus` to `(present, content)`. If not present, returns
///    `met: false` without invoking the LLM (existence floor).
/// 2. Builds the judge [`Agent`]: `JudgeRef::Builtin` synthesizes one with
///    the canonical prompt; `JudgeRef::Agent(id)` clones from the module.
/// 3. Runs the judge agent with the artifact content as the user turn.
/// 4. Parses the last assistant text as `{met, rationale}`.
///
/// `judge_model` inside `JudgeRef::Builtin` is stored in `Agent.model` but is
/// a runtime no-op in v1 — the ambient backend is always used (see Decision 6
/// in the deliverables-and-goals plan).
pub async fn evaluate_deliverable<D>(
    module: Arc<IrModule>,
    locus: &Locus,
    must_satisfy: &str,
    judge: &JudgeRef,
    store: &OutputStore,
    reader: Option<&dyn ArtifactReader>,
    dispatcher: Arc<D>,
) -> Result<CheckVerdict, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let (present, content) = resolve_locus(locus, store, reader)?;
    if !present {
        return Ok(CheckVerdict {
            met: false,
            rationale: format!("deliverable {locus:?} was not produced"),
        });
    }
    let artifact = content.unwrap_or_default();

    let judge_agent: Agent = match judge {
        JudgeRef::Agent(id) => module
            .workflow
            .agents
            .get(id)
            .ok_or_else(|| RuntimeError::AgentNotFound {
                agent: id.0.clone(),
            })?
            .clone(),
        JudgeRef::Default { model_ref } => Agent {
            id: AgentId(String::from("__judge")),
            prompt: builtin_judge_prompt(must_satisfy),
            model_ref: model_ref.clone(),
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            produces: alloc::vec::Vec::new(),
            budget: AgentBudget {
                max_turns: Some(1),
                max_tokens: None,
            },
            output_schema: None,
        },
    };

    let outcome = Box::pin(run_agent(
        module.clone(),
        &judge_agent,
        dispatcher,
        vec![user_message(&artifact)],
    ))
    .await?;

    let text = last_assistant_text(&outcome);
    Ok(parse_verdict(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::artifact::InMemoryArtifactReader;

    /// Minimal test registry: answers `FN_BUILTIN_NON_EMPTY` by inspecting
    /// `present` and `content` fields from the args object.
    struct TestRegistry;

    impl DeterministicRegistry for TestRegistry {
        fn invoke(&self, fn_name: &str, args: &Value) -> Result<Value, RuntimeError> {
            if fn_name == FN_BUILTIN_NON_EMPTY {
                let present = args["present"].as_bool().unwrap_or(false);
                let content = args["content"].as_str().unwrap_or("");
                Ok(json!(present && !content.is_empty()))
            } else {
                Err(RuntimeError::Internal {
                    message: format!("TestRegistry: unknown fn {fn_name}"),
                })
            }
        }
    }

    #[test]
    fn goal_non_empty_passes_on_present_content() {
        let reg = TestRegistry;
        let store = OutputStore::new();
        let reader = InMemoryArtifactReader::new().with_file("/r.md", b"hello");
        let verdict = evaluate_goal(
            &Locus::Path("/r.md".into()),
            &GoalPredicate::NonEmpty,
            &store,
            Some(&reader as &dyn ArtifactReader),
            &reg,
        )
        .unwrap();
        assert!(verdict.met);
    }

    #[test]
    fn goal_non_empty_fails_when_absent() {
        let reg = TestRegistry;
        let store = OutputStore::new();
        let reader = InMemoryArtifactReader::new();
        let verdict = evaluate_goal(
            &Locus::Path("/missing".into()),
            &GoalPredicate::NonEmpty,
            &store,
            Some(&reader as &dyn ArtifactReader),
            &reg,
        )
        .unwrap();
        assert!(!verdict.met);
    }

    // -----------------------------------------------------------------------
    // evaluate_deliverable tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "test-fixtures")]
    mod deliverable_tests {
        use super::*;
        use alloc::collections::BTreeMap;
        use alloc::string::ToString;
        use alloc::sync::Arc;
        use core::future::Future;
        use core::pin::Pin;
        use serde_json::Value;
        use tau_ir::ToolId;
        use tau_ports::fixtures::{make_completion_response, MockLlmBackend};
        use tau_ports::llm::StopReason;

        use crate::builder::DynLlmBackend;
        use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};

        /// Minimal `ToolDispatcher` backed by a `MockLlmBackend`.
        struct TestDispatcher {
            backend: Arc<MockLlmBackend>,
        }

        impl ToolDispatcher for TestDispatcher {
            fn invoke<'a>(
                &'a self,
                _tool_id: &'a ToolId,
                _args: &'a Value,
            ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>>
            {
                Box::pin(async move {
                    Ok(ToolInvocationResult {
                        body: None,
                        error: None,
                    })
                })
            }

            fn llm_backend_for(
                &self,
                _backend: &str,
            ) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
                Ok(self.backend.clone())
            }
        }

        /// Build a minimal stub `IrModule` (no agents/tools/steps/checks/pipeline).
        fn stub_module() -> Arc<tau_ir::IrModule> {
            Arc::new(tau_ir::IrModule {
                ir_format: tau_ir::IrFormatVersion::current(),
                tau_version: env!("CARGO_PKG_VERSION").into(),
                target: tau_ports::target::registry::list_available()
                    .next()
                    .expect("at least one target")
                    .triple,
                workflow: tau_ir::Workflow {
                    agents: BTreeMap::new(),
                    tools: BTreeMap::new(),
                    steps: BTreeMap::new(),
                    edges: alloc::vec::Vec::new(),
                    capability_table: tau_ir::CapabilityTable(BTreeMap::new()),
                    pipeline: None,
                    checks: BTreeMap::new(),
                },
                triggers: alloc::vec::Vec::new(),
            })
        }

        #[tokio::test]
        async fn deliverable_builtin_judge_passes_on_good_artifact() {
            // Script the mock to return {"met": true, "rationale": "good"}.
            let backend = Arc::new(MockLlmBackend::new("mock-llm").with_response(
                make_completion_response(
                    r#"{"met": true, "rationale": "good"}"#.to_string(),
                    alloc::vec![],
                    StopReason::EndTurn,
                    None,
                ),
            ));
            let dispatcher = Arc::new(TestDispatcher {
                backend: backend.clone(),
            });

            let module = stub_module();
            let reader = InMemoryArtifactReader::new().with_file("/out.md", b"report content");
            let store = OutputStore::new();
            let judge = JudgeRef::Default {
                model_ref: tau_ir::ModelRef {
                    backend: "mock-llm".into(),
                    model_id: "m".into(),
                },
            };

            let verdict = evaluate_deliverable(
                module,
                &Locus::Path("/out.md".into()),
                "must be non-empty",
                &judge,
                &store,
                Some(&reader as &dyn ArtifactReader),
                dispatcher,
            )
            .await
            .unwrap();

            assert!(verdict.met, "expected met=true; got: {:?}", verdict);
            assert_eq!(verdict.rationale, "good");
        }

        #[tokio::test]
        async fn deliverable_absent_artifact_fails_without_calling_llm() {
            // Empty reader — artifact is absent.
            let backend = Arc::new(MockLlmBackend::new("mock-llm").with_response(
                make_completion_response(
                    r#"{"met": true, "rationale": "good"}"#.to_string(),
                    alloc::vec![],
                    StopReason::EndTurn,
                    None,
                ),
            ));
            let dispatcher = Arc::new(TestDispatcher {
                backend: backend.clone(),
            });

            let module = stub_module();
            let reader = InMemoryArtifactReader::new(); // nothing seeded
            let store = OutputStore::new();
            let judge = JudgeRef::Default {
                model_ref: tau_ir::ModelRef {
                    backend: "mock-llm".into(),
                    model_id: "m".into(),
                },
            };

            let verdict = evaluate_deliverable(
                module,
                &Locus::Path("/missing.md".into()),
                "must be non-empty",
                &judge,
                &store,
                Some(&reader as &dyn ArtifactReader),
                dispatcher,
            )
            .await
            .unwrap();

            assert!(!verdict.met, "expected met=false for absent artifact");
            // Existence floor: LLM must NOT have been called.
            assert_eq!(
                backend.invocation_count(),
                0,
                "LLM must not be called when artifact is absent (existence floor)"
            );
        }
    }
}
