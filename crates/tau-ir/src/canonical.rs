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
    use crate::ids::{AgentId, PipelineStepId};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::pipeline::{Pipeline, PipelineStep, StepRun};
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
        };
        assert_eq!(m.ir_format.0, "v1.2.0");
        let bytes = to_canonical_bytes(&m);
        let back = from_canonical_bytes(&bytes).expect("round-trips");
        assert_eq!(m, back);
    }
}
