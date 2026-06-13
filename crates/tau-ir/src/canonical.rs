//! Deterministic serialization of an `IrModule` to canonical bytes.
//!
//! Rules (per design spec D-6):
//! 1. Deserialize once, re-serialize via the canonical encoder. The
//!    canonical encoder writes fields in a fixed order, uses BTreeMap
//!    iteration (alphabetical) for every map, and serializes optional
//!    fields verbatim (None → null) — no skipping.
//! 2. No `SystemTime` in the bytes (i64-ms only — enforced by the type
//!    surface, not by this encoder).
//! 3. The encoder is idempotent: `decode(encode(x)) == x` and
//!    `encode(decode(encode(x))) == encode(x)`.

use alloc::vec::Vec;

use crate::module::IrModule;

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

/// Deserialize canonical bytes back to an `IrModule`. Pure inverse of
/// `to_canonical_bytes`.
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod pipeline_canonical_tests {
    use super::*;
    use crate::check::{Check, CheckVerify, JudgeRef, Locus, OnFail, RetryPolicy};
    use crate::ids::{AgentId, CheckId, PipelineStepId};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::pipeline::{Pipeline, PipelineStep, StepRun};
    use crate::GoalPredicate;
    use alloc::collections::BTreeMap;
    use tau_ports::target::registry;

    #[test]
    fn module_with_pipeline_round_trips_and_reports_v1_2() {
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
        assert_eq!(m.ir_format.0, "v1.2.0");
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

        // One deliverable check: path locus, builtin judge with no model
        // override.
        let deliverable_check = Check {
            id: CheckId("report".into()),
            verify: CheckVerify::Deliverable {
                locus: Locus::Path("/r.md".into()),
                must_satisfy: "Must have sources section.".into(),
                judge: JudgeRef::Builtin { model: None },
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
}
