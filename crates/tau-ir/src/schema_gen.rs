//! JSON-Schema generation + canonical sample IR modules (EPIC 2.2).
//! Feature-gated (`schema`, std) — the published authoring contract is
//! generated from these serde types so it can never drift (ADR-0056).

use alloc::vec::Vec;

use crate::module::{IrFormatVersion, IrModule};

/// The IR JSON Schema for `IrModule`, with the current `ir_format` embedded
/// in `$id`/`title`. Generated from the serde types via schemars.
pub fn ir_json_schema() -> serde_json::Value {
    let mut root = schemars::schema_for!(IrModule);
    // Embed the version so consumers read it from the schema (ADR-0056:
    // versioned by ir_format).
    let meta = root.schema.metadata();
    meta.title = Some(alloc::format!(
        "tau IR (ir_format {})",
        IrFormatVersion::CURRENT
    ));
    meta.id = Some(alloc::format!(
        "https://tau-rs.github.io/tau/schema/ir/tau-ir.schema.json#{}",
        IrFormatVersion::CURRENT
    ));
    serde_json::to_value(&root).expect("schema serializes")
}

/// Representative IR modules exercising the tagged enums (`Node`, `ToolImpl`,
/// `StepRun`) — the published conformance samples. Built directly (mirrors
/// the literal in `canonical.rs`).
pub fn sample_modules() -> Vec<(&'static str, IrModule)> {
    Vec::from([
        (
            "agent_native_tool",
            crate::schema_gen_samples::agent_native_tool(),
        ),
        (
            "agent_mcp_tool",
            crate::schema_gen_samples::agent_mcp_tool(),
        ),
        (
            "deterministic_step",
            crate::schema_gen_samples::deterministic_step(),
        ),
        ("subflow", crate::schema_gen_samples::subflow()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_object_with_version_and_defs() {
        let s = ir_json_schema();
        assert_eq!(
            s["title"],
            serde_json::json!(alloc::format!(
                "tau IR (ir_format {})",
                IrFormatVersion::CURRENT
            ))
        );
        assert!(
            s.get("$defs").or_else(|| s.get("definitions")).is_some(),
            "schema must carry type defs"
        );
    }

    #[test]
    fn samples_build_and_serialize() {
        for (name, m) in sample_modules() {
            serde_json::to_value(&m).unwrap_or_else(|e| panic!("{name} serializes: {e}"));
        }
    }
}
