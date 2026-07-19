//! Deterministic serialization of an `IrModule` to canonical bytes.
//!
//! `to_canonical_bytes` writes the derived `serde_json` compact encoding:
//! fields in Rust declaration order, `BTreeMap` fields alphabetically,
//! `skip_serializing_if` honored (absent optional fields are omitted, not
//! `null`). Two successive calls yield identical bytes.
//!
//! `from_canonical_bytes` is version-gated (D8-B): it rejects a bundle whose
//! `ir_format` MAJOR differs from this crate's before attempting a full decode.
//!
//! NOTE: the canonical form is scheduled to become a specification (sorted-key
//! compact JSON) under D9-C / `ir_format 3.0.0`; until then it is the derived
//! serde encoding described above.

use alloc::vec::Vec;

use crate::error::IrError;
use crate::module::{IrFormatVersion, IrModule};
use serde::Deserialize;

/// Serialize an `IrModule` to canonical bytes.
///
/// Uses `serde_json`'s compact (no-pretty) encoder over the IrModule's
/// derived `Serialize` impl. Map iteration is `BTreeMap` (alphabetical)
/// because every map field in `IrModule`/`Workflow` is a `BTreeMap`.
/// All fields serialize unconditionally: `Option::None` becomes JSON
/// `null` (no `skip_serializing_if`), and `Vec` order is preserved
/// as-given.
pub fn to_canonical_bytes(module: &IrModule) -> Vec<u8> {
    serde_json::to_vec(module).expect("IrModule serializes cleanly to JSON")
}

/// Phase-1 peek: read only `ir_format` from the canonical bytes. serde
/// ignores every other field, so this is shape-independent — it decodes
/// even when the full `IrModule` shape has changed across a major.
#[derive(Deserialize)]
struct VersionPeek {
    ir_format: IrFormatVersion,
}

/// Deserialize canonical bytes back to an `IrModule`, gated on IR-format
/// MAJOR compatibility.
///
/// Two-phase:
/// 1. Decode `VersionPeek` to read `ir_format`; compare its major against
///    [`IrFormatVersion::CURRENT_MAJOR`]. A different major is a breaking
///    IR-shape change → [`IrError::FormatMajorMismatch`] (the full decode
///    is never attempted). An unparseable version →
///    [`IrError::FormatUnparseable`].
/// 2. Same major → full `IrModule` decode; a serde failure maps to
///    [`IrError::Decode`].
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, IrError> {
    use alloc::string::ToString;

    // Phase 1: peek the version.
    let peek: VersionPeek =
        serde_json::from_slice(bytes).map_err(|e| IrError::Decode(e.to_string()))?;
    let bundle_major = peek
        .ir_format
        .major()
        .map_err(|()| IrError::FormatUnparseable {
            value: peek.ir_format.0.clone(),
        })?;
    if bundle_major != IrFormatVersion::CURRENT_MAJOR {
        return Err(IrError::FormatMajorMismatch {
            bundle: peek.ir_format.0.clone(),
            current: IrFormatVersion::CURRENT.to_string(),
            bundle_major,
        });
    }

    // Phase 2: full structural decode.
    serde_json::from_slice(bytes).map_err(|e| IrError::Decode(e.to_string()))
}

#[cfg(test)]
mod pipeline_canonical_tests {
    use super::*;
    use crate::check::{Check, CheckVerify, Condition, JudgeRef, Locus, OnFail, RetryPolicy};
    use crate::ids::{AgentId, CheckId, PipelineStepId};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::pipeline::{Pipeline, PipelineStep, StepRun};
    use crate::GoalPredicate;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use tau_ports::target::registry;

    #[test]
    fn module_with_pipeline_round_trips_and_reports_v2_1() {
        let target = registry::list_available().next().unwrap().triple;
        let wf = Workflow {
            pipeline: Some(Pipeline {
                steps: alloc::vec![PipelineStep {
                    id: PipelineStepId("a".into()),
                    run: StepRun::Agent(AgentId("a".into())),
                    input: "${input}".into(),
                }],
            }),
            ..Workflow::default()
        };
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: wf,
            triggers: alloc::vec::Vec::new(),
        };
        assert_eq!(m.ir_format.0, "v2.4.0");
        let bytes = to_canonical_bytes(&m);
        let back = from_canonical_bytes(&bytes).expect("round-trips");
        assert_eq!(m, back);
    }

    #[test]
    fn module_with_checks_round_trips_and_bytes_are_stable() {
        let target = registry::list_available().next().unwrap().triple;

        // One goal check: regex match predicate over a path locus.
        let goal_check = Check {
            id: CheckId("has_header".into()),
            verify: CheckVerify::Goal {
                evaluates: Locus::Path("/x".into()),
                predicate: GoalPredicate::Matches("^#".into()),
            },
            retry: RetryPolicy {
                on_fail: OnFail::Abort,
                max_attempts: 1,
                gate: PipelineStepId("writer".into()),
            },
        };

        // One deliverable check: path locus, canonical judge on a resolved
        // model.
        let deliverable_check = Check {
            id: CheckId("report".into()),
            verify: CheckVerify::Deliverable {
                locus: Locus::Path("/r.md".into()),
                must_satisfy: "Must have sources section.".into(),
                judge: JudgeRef::Default {
                    model_ref: crate::model_ref::ModelRef {
                        backend: "anthropic".into(),
                        model_id: "claude-haiku-4-5".into(),
                    },
                },
            },
            retry: RetryPolicy {
                on_fail: OnFail::Abort,
                max_attempts: 1,
                gate: PipelineStepId("writer".into()),
            },
        };

        let mut checks = BTreeMap::new();
        checks.insert(CheckId("has_header".into()), goal_check);
        checks.insert(CheckId("report".into()), deliverable_check);

        let wf = Workflow {
            pipeline: Some(Pipeline {
                steps: alloc::vec![
                    PipelineStep {
                        id: PipelineStepId("writer".into()),
                        run: StepRun::Agent(AgentId("writer".into())),
                        input: "${input}".into(),
                    },
                    PipelineStep {
                        id: PipelineStepId("check-has-header".into()),
                        run: StepRun::Check(CheckId("has_header".into())),
                        input: "${input}".into(),
                    },
                    PipelineStep {
                        id: PipelineStepId("check-report".into()),
                        run: StepRun::Check(CheckId("report".into())),
                        input: "${input}".into(),
                    },
                ],
            }),
            checks,
            ..Workflow::default()
        };
        let module = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: wf,
            triggers: alloc::vec::Vec::new(),
        };

        // --- structural round-trip via from_canonical_bytes ---
        let bytes = to_canonical_bytes(&module);
        let back: IrModule = serde_json::from_slice(&bytes).expect("round-trips");
        assert_eq!(
            module, back,
            "structural round-trip must preserve all fields"
        );

        // --- byte-stability: two calls must yield identical bytes ---
        let bytes2 = to_canonical_bytes(&module);
        assert_eq!(
            bytes, bytes2,
            "to_canonical_bytes must be idempotent (same bytes on second call)"
        );

        // Confirm checks key is present in the serialized form.
        let obj: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert!(
            obj["workflow"]["checks"].is_object(),
            "serialized module must contain workflow.checks object"
        );
        assert_eq!(
            obj["workflow"]["checks"].as_object().unwrap().len(),
            2,
            "expected 2 checks in serialized form"
        );
    }

    #[test]
    fn pre_4_1_module_canonical_bytes_are_byte_stable() {
        // A module using ONLY the original leaf StepRuns must serialize to the
        // exact same canonical bytes as before 4.1 (golden), proving the appended
        // variants are non-disruptive. Build a minimal agent-only pipeline.
        let target = registry::list_available().next().unwrap().triple;
        let wf = Workflow {
            pipeline: Some(Pipeline {
                steps: alloc::vec![PipelineStep {
                    id: PipelineStepId("a".into()),
                    run: StepRun::Agent(AgentId("a".into())),
                    input: "${input}".into(),
                }],
            }),
            ..Workflow::default()
        };
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: wf,
            triggers: alloc::vec::Vec::new(),
        };
        let actual = String::from_utf8(to_canonical_bytes(&m)).unwrap();
        // Golden: the canonical JSON of a one-Agent-step module.
        let golden = include_str!("../tests/golden/pre_4_1_agent_module.canonical.json");
        assert_eq!(
            actual, golden,
            "canonical bytes of a pre-4.1 agent-only module must be byte-stable"
        );
    }

    #[test]
    fn new_control_flow_variants_round_trip() {
        let target = registry::list_available().next().unwrap().triple;

        // A nested agent step used inside the new variants.
        let inner_step = || PipelineStep {
            id: PipelineStepId("inner".into()),
            run: StepRun::Agent(AgentId("inner-agent".into())),
            input: "${input}".into(),
        };

        // Branch with a GoalPredicate::Exists condition.
        let branch_step = PipelineStep {
            id: PipelineStepId("b".into()),
            run: StepRun::Branch {
                on: Condition {
                    evaluates: Locus::Path("/flag".into()),
                    predicate: GoalPredicate::Exists,
                },
                then: alloc::vec![inner_step()],
                otherwise: alloc::vec![],
            },
            input: "${input}".into(),
        };

        // Parallel with two branches.
        let parallel_step = PipelineStep {
            id: PipelineStepId("p".into()),
            run: StepRun::Parallel {
                branches: alloc::vec![alloc::vec![inner_step()], alloc::vec![inner_step()]],
            },
            input: "${input}".into(),
        };

        // Loop with a Matches predicate and max_iters bound.
        let loop_step = PipelineStep {
            id: PipelineStepId("l".into()),
            run: StepRun::Loop {
                body: alloc::vec![inner_step()],
                until: Condition {
                    evaluates: Locus::Output(PipelineStepId("inner".into())),
                    predicate: GoalPredicate::Matches("^done".into()),
                },
                max_iters: 5,
            },
            input: "${input}".into(),
        };

        // Suspend.
        let suspend_step = PipelineStep {
            id: PipelineStepId("s".into()),
            run: StepRun::Suspend {
                resume_signal: "human-approval".into(),
            },
            input: "${input}".into(),
        };

        let wf = Workflow {
            pipeline: Some(Pipeline {
                steps: alloc::vec![branch_step, parallel_step, loop_step, suspend_step],
            }),
            ..Workflow::default()
        };
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: wf,
            triggers: alloc::vec::Vec::new(),
        };
        let bytes = to_canonical_bytes(&m);
        let back = from_canonical_bytes(&bytes).expect("round-trips");
        assert_eq!(
            back, m,
            "new control-flow variants must survive a round-trip"
        );
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::error::IrError;
    use crate::module::{IrFormatVersion, IrModule};
    use alloc::string::{String, ToString};
    use tau_ports::target::registry;

    /// A minimal, valid v2.4.0 module serialized to canonical bytes.
    fn base_module() -> IrModule {
        let target = registry::list_available().next().unwrap().triple;
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: crate::module::Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        }
    }

    /// Serialize `base_module`, then overwrite its `ir_format` field with
    /// `version` and return the bytes.
    fn bytes_with_version(version: &str) -> alloc::vec::Vec<u8> {
        let m = base_module();
        let mut v: serde_json::Value = serde_json::from_slice(&to_canonical_bytes(&m)).unwrap();
        v["ir_format"] = serde_json::Value::String(version.to_string());
        serde_json::to_vec(&v).unwrap()
    }

    #[test]
    fn same_major_decodes_ok() {
        for ver in ["v2.4.0", "v2.3.0", "v2.5.0", "v2.9.9"] {
            let out = from_canonical_bytes(&bytes_with_version(ver));
            assert!(out.is_ok(), "{ver} should decode: {out:?}");
        }
    }

    #[test]
    fn major_mismatch_is_rejected() {
        for ver in ["v3.0.0", "v1.0.0"] {
            match from_canonical_bytes(&bytes_with_version(ver)) {
                Err(IrError::FormatMajorMismatch { bundle, .. }) => {
                    assert_eq!(bundle, ver);
                }
                other => panic!("{ver} expected FormatMajorMismatch, got {other:?}"),
            }
        }
    }

    #[test]
    fn unparseable_version_is_rejected() {
        for ver in ["garbage", "", "v.4"] {
            match from_canonical_bytes(&bytes_with_version(ver)) {
                Err(IrError::FormatUnparseable { value }) => assert_eq!(value, ver),
                other => panic!("{ver:?} expected FormatUnparseable, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_additive_field_is_tolerated() {
        // Forward-compat: a same-major bundle carrying a field this tau
        // does not know (future D6-B `assets`) still decodes.
        let m = base_module();
        let mut v: serde_json::Value = serde_json::from_slice(&to_canonical_bytes(&m)).unwrap();
        v["ir_format"] = serde_json::Value::String("v2.5.0".to_string());
        v["assets"] = serde_json::json!({ "logo": "deadbeef" });
        let bytes = serde_json::to_vec(&v).unwrap();
        assert!(from_canonical_bytes(&bytes).is_ok());
    }

    #[test]
    fn valid_version_bad_body_is_decode_error() {
        // ir_format ok, but the body is not an IrModule.
        let bytes = br#"{"ir_format":"v2.4.0","nonsense":true}"#;
        match from_canonical_bytes(bytes) {
            Err(IrError::Decode(_)) => {}
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn not_even_json_is_decode_error() {
        match from_canonical_bytes(b"not json at all") {
            Err(IrError::Decode(_)) => {}
            other => panic!("expected Decode, got {other:?}"),
        }
    }
}
