//! `tau target show <triple>` — detail view + parse-error suggestions.

use std::str::FromStr;

use serde_json::json;
use tau_ports::target::{TargetTriple, TripleStatus};

use crate::cli::TargetShowArgs;
use crate::cmd::skill::levenshtein::closest_match;
use crate::cmd::target::render;
use crate::output::Output;

/// Run `tau target show`.
pub fn run(args: &TargetShowArgs, output: &mut Output) -> anyhow::Result<()> {
    let triple = match TargetTriple::from_str(&args.triple) {
        Ok(t) => t,
        Err(e) => {
            emit_parse_error(&args.triple, &e, output)?;
            std::process::exit(64);
        }
    };

    let entry = match tau_ports::target::lookup(&triple) {
        Some(e) => e,
        None => {
            emit_unknown(&triple, output)?;
            std::process::exit(64);
        }
    };

    if output.is_json() {
        render::render_json_event(entry, output)?;
        return Ok(());
    }

    let shapes = (entry.shapes_fn)();
    let mut shape_names: Vec<&'static str> = Vec::new();
    for s in [
        tau_domain::CapabilityShape::FilesystemRead,
        tau_domain::CapabilityShape::FilesystemWrite,
        tau_domain::CapabilityShape::ProcessExec,
        tau_domain::CapabilityShape::NetworkHttp,
        tau_domain::CapabilityShape::AgentSpawn,
    ] {
        if shapes.contains(&s) {
            shape_names.push(render::shape_display(&s));
        }
    }
    let shapes_csv = shape_names.join(", ");
    let status_line = match entry.status {
        TripleStatus::Available => "Available".to_string(),
        TripleStatus::Reserved { reason } => format!("Reserved ({reason})"),
        _ => "Unknown".to_string(),
    };

    output.human(&format!("{}", entry.triple))?;
    output.human(&format!("  status:   {status_line}"))?;
    output.human(&format!("  platform: {}", entry.triple.platform))?;
    output.human(&format!("  adapter:  {}", entry.triple.adapter_family))?;
    output.human(&format!(
        "  tier:     {}",
        render::tier_str(entry.triple.tier)
    ))?;
    output.human(&format!("  shapes:   {shapes_csv}"))?;
    Ok(())
}

fn emit_parse_error(
    input: &str,
    err: &tau_ports::target::ParseError,
    output: &mut Output,
) -> anyhow::Result<()> {
    if output.is_json() {
        output.json(&json!({
            "event": "error",
            "kind": "parse",
            "input": input,
            "reason": err.to_string(),
        }))?;
    } else {
        output.error(format!("could not parse triple `{input}`: {err}"))?;
        if let Some(hint) = suggest(input) {
            output.human(&format!("  did you mean: {hint}?"))?;
        }
    }
    Ok(())
}

fn emit_unknown(triple: &TargetTriple, output: &mut Output) -> anyhow::Result<()> {
    if output.is_json() {
        output.json(&json!({
            "event": "error",
            "kind": "unknown",
            "input": triple.to_string(),
        }))?;
    } else {
        output.error(format!(
            "unknown triple `{triple}` (parses but not registered)"
        ))?;
        if let Some(hint) = suggest(&triple.to_string()) {
            output.human(&format!("  did you mean: {hint}?"))?;
        }
    }
    Ok(())
}

fn suggest(input: &str) -> Option<String> {
    let candidates: Vec<String> = tau_ports::target::list_all()
        .map(|e| e.triple.to_string())
        .collect();
    closest_match(input, &candidates, 4).map(|s| s.to_string())
}
