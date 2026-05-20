//! `TargetCapabilityProfile` + `TripleStatus`. The profile is the
//! materialised form of a registry entry — owns its `CapabilityShapeSet`
//! and is suitable for cloning into a check result or serialising.

use tau_domain::CapabilityShapeSet;

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
    /// Whether this triple ships in v1 or is reserved.
    pub status: TripleStatus,
}
