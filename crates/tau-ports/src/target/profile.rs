//! `TargetCapabilityProfile` + `TripleStatus`. The profile is the
//! materialised form of a registry entry — owns its `CapabilityShapeSet`
//! and is suitable for cloning into a check result or serialising.

use tau_domain::{CapabilityShapeSet, IrFeature};

use crate::target::triple::TargetTriple;

/// Status of a registered target triple.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub enum TripleStatus {
    /// Triple is supported; at least one adapter family + platform
    /// satisfies its constraints and the implementation has shipped.
    Available,
    /// Triple is reserved (name is taken; no shipping implementation).
    Reserved {
        /// Human-readable reason; surfaced in `tau target show` and
        /// `tau check --target` Warning findings.
        reason: &'static str,
    },
}

/// Materialised target triple profile (registry entry + owned shape set).
#[derive(Debug, Clone)]
pub struct TargetCapabilityProfile {
    /// The triple this profile is for.
    pub triple: TargetTriple,
    /// Capability shapes the target's adapter must enforce.
    pub required_shapes: CapabilityShapeSet,
    /// Optional IR execution features this target can run. A workflow that uses
    /// a feature absent here is refused at build time (see `tau-ir-lower`'s
    /// `feature_fit`). Wasm guests drive `run_ir_streaming`, not `run_pipeline`,
    /// so they list none — control-flow has no wasm execution path.
    pub supported_features: &'static [IrFeature],
    /// Whether this triple ships in v1 or is reserved.
    pub status: TripleStatus,
}
