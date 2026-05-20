//! Shared rendering helpers for `tau target list` and `tau target show`.

use serde_json::json;
use tau_ports::target::{TargetTripleEntry, TripleStatus};

use crate::output::Output;

/// Render one entry as a human-readable line.
pub(crate) fn render_human_line(e: &TargetTripleEntry, output: &mut Output) -> anyhow::Result<()> {
    let status = match e.status {
        TripleStatus::Available => "Available",
        TripleStatus::Reserved { .. } => "Reserved ",
        _ => "Unknown  ",
    };
    let shapes = (e.shapes_fn)();
    let mut shape_names: Vec<&'static str> = Vec::new();
    for s in [
        tau_domain::CapabilityShape::FilesystemRead,
        tau_domain::CapabilityShape::FilesystemWrite,
        tau_domain::CapabilityShape::ProcessExec,
        tau_domain::CapabilityShape::NetworkHttp,
        tau_domain::CapabilityShape::AgentSpawn,
    ] {
        if shapes.contains(&s) {
            shape_names.push(shape_display(&s));
        }
    }
    let shapes_csv = shape_names.join(", ");
    output.human(&format!(
        "{:<24} {}  {}",
        e.triple.to_string(),
        status,
        shapes_csv
    ))?;
    Ok(())
}

/// Render one entry as a JSONL event.
pub(crate) fn render_json_event(e: &TargetTripleEntry, output: &mut Output) -> anyhow::Result<()> {
    let (status_str, reason) = match e.status {
        TripleStatus::Available => ("available", None),
        TripleStatus::Reserved { reason } => ("reserved", Some(reason)),
        _ => ("unknown", None),
    };
    let shapes = (e.shapes_fn)();
    let mut shape_strs: Vec<String> = Vec::new();
    for s in [
        tau_domain::CapabilityShape::FilesystemRead,
        tau_domain::CapabilityShape::FilesystemWrite,
        tau_domain::CapabilityShape::ProcessExec,
        tau_domain::CapabilityShape::NetworkHttp,
        tau_domain::CapabilityShape::AgentSpawn,
    ] {
        if shapes.contains(&s) {
            shape_strs.push(shape_display(&s).to_string());
        }
    }
    output.json(&json!({
        "event": "target",
        "triple": e.triple.to_string(),
        "platform": e.triple.platform.as_str(),
        "adapter_family": e.triple.adapter_family.as_str(),
        "tier": tier_str(e.triple.tier),
        "status": status_str,
        "reason": reason,
        "required_shapes": shape_strs,
    }))?;
    Ok(())
}

/// Short display name for a `CapabilityShape`.
pub(crate) fn shape_display(s: &tau_domain::CapabilityShape) -> &'static str {
    match s {
        tau_domain::CapabilityShape::FilesystemRead => "fs.r",
        tau_domain::CapabilityShape::FilesystemWrite => "fs.w",
        tau_domain::CapabilityShape::ProcessExec => "exec",
        tau_domain::CapabilityShape::NetworkHttp => "net.http",
        tau_domain::CapabilityShape::AgentSpawn => "agent.spawn",
        tau_domain::CapabilityShape::SkillSpawn => "skill.spawn",
        // CapabilityShape::Custom shouldn't appear in v1 triples; fall back to a
        // stable placeholder rather than leaking storage.
        tau_domain::CapabilityShape::Custom { .. } => "custom",
        _ => "unknown",
    }
}

/// Lowercase string for a `SandboxTier`.
pub(crate) fn tier_str(t: tau_ports::SandboxTier) -> &'static str {
    match t {
        tau_ports::SandboxTier::None => "none",
        tau_ports::SandboxTier::Light => "light",
        tau_ports::SandboxTier::Strict => "strict",
        _ => "unknown",
    }
}
