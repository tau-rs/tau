//! `IrFeature` — the vocabulary of optional IR execution features a target may
//! or may not support.
//!
//! Analogous to [`CapabilityShape`](crate::CapabilityShape): where a shape says
//! "this workflow needs filesystem-write, does the target enforce it?", an
//! `IrFeature` says "this workflow uses a `Loop`, can the target execute it?".
//! Leaf pipeline steps (agent/tool/deterministic/check) are the base surface and
//! are **not** features — only the EPIC 4.1 control-flow constructs are.
//!
//! Placement rationale: both `tau-ports` (targets declare `supported_features`)
//! and `tau-ir` (maps `StepRun` → `IrFeature`) reference this type, and neither
//! may depend on the other. Like `CapabilityShape`, it lives in the innermost
//! `tau-domain` vocabulary crate so dependencies point inward.

/// An optional IR execution feature. A target's
/// [`supported_features`](../../tau_ports/target/profile/struct.TargetCapabilityProfile.html)
/// lists the ones it can execute; a workflow that uses a feature the target does
/// not support is refused at build time (see `tau-ir-lower`'s `feature_fit`).
///
/// `Copy` (no heap-allocating variant), so sets are plain `&'static [IrFeature]`
/// — no owned-set type is needed (contrast `CapabilityShapeSet`, whose
/// `Custom { name }` variant forces an owned collection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IrFeature {
    /// Conditional `Branch { on, then, otherwise }` (EPIC 4.1 / 4.2a).
    Branch,
    /// Concurrent fan-out/join `Parallel { branches }` (EPIC 4.1 / 4.2b).
    Parallel,
    /// Bounded `Loop { body, until, max_iters }` (EPIC 4.1 / 4.2c).
    Loop,
    /// Human-in-the-loop `Suspend { resume_signal }` (EPIC 4.1 / 4.3).
    Suspend,
}

impl IrFeature {
    /// Every control-flow feature, for targets that support all of them
    /// (every native/host target today). Kept here so a new variant forces a
    /// compile-visible update of this list alongside the enum.
    pub const ALL: &'static [IrFeature] = &[
        IrFeature::Branch,
        IrFeature::Parallel,
        IrFeature::Loop,
        IrFeature::Suspend,
    ];
}
