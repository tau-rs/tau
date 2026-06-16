//! D-3b: strict build-time capability-fit check.
//!
//! Refuses the build (returns `LowerError::CapabilityFitFailed`) if any
//! tool's declared capabilities require a shape that the target's
//! `required_shapes` does not include. **No override flag** — the
//! caller's user-facing diagnostic must say so explicitly. Matches the
//! Rust-like build-time enforcement principle per
//! `feedback_tau_rust_like_build_enforcement`.
//!
//! # Field-name correction
//!
//! The plan stub references `entry.profile().supported_shapes`. The
//! actual field name in `tau_ports::target::profile::TargetCapabilityProfile`
//! is `required_shapes` (the set of shapes the adapter declares it can
//! enforce). This module uses `required_shapes` throughout.

use alloc::vec::Vec;
use tau_domain::CapabilityShape;
use tau_ports::target::{registry, TargetTriple};

use crate::error::LowerError;
use tau_ir::ids::ToolId;

use super::parse::Parsed;

/// Run the capability-fit check on a `Parsed` workflow against a target.
///
/// Returns `Ok(())` if every shape required by any tool in the workflow
/// is present in the target's `required_shapes` set. Returns
/// `Err(LowerError::CapabilityFitFailed)` on the first miss (with the
/// full list of missing shapes and the tools that caused them).
///
/// If the target triple is not found in the registry, the check also
/// fails with an empty `missing` list — the caller's diagnostic should
/// handle this as "unknown target".
pub(super) fn check(parsed: &Parsed, target: &TargetTriple) -> Result<(), LowerError> {
    let entry = registry::lookup(target).ok_or_else(|| LowerError::CapabilityFitFailed {
        missing: Vec::new(),
        tools: Vec::new(),
    })?;
    let supported = entry.profile().required_shapes;

    let mut missing: Vec<CapabilityShape> = Vec::new();
    let mut blamed_tools: Vec<ToolId> = Vec::new();

    for (tool_id, tool) in parsed.workflow.tools.iter() {
        for cap in tool.capabilities.declared.iter() {
            let shape = cap.required_shape();
            if !supported.contains(&shape) {
                if !missing.contains(&shape) {
                    missing.push(shape);
                }
                if !blamed_tools.contains(tool_id) {
                    blamed_tools.push(tool_id.clone());
                }
            }
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(LowerError::CapabilityFitFailed {
            missing,
            tools: blamed_tools,
        })
    }
}
