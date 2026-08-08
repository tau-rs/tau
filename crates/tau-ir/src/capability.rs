//! Capability requirements as carried in the IR.
//!
//! v0 wraps `tau_domain::Capability` (the existing source-of-truth shape) in
//! a `CapabilityTable` newtype keyed by [`crate::ToolId`]. Per the D-3b
//! decision, the lowering pass intersects this table against the target
//! triple's `supported_shapes` at build time and refuses the build on any
//! miss.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use tau_domain::Capability;

use crate::ids::ToolId;

/// The capability-requirement set for one tool.
///
/// Re-export shape over `Vec<tau_domain::Capability>` — the IR does not
/// re-define what a capability *is*; it just carries the existing type
/// across the boundary. Future evolution (capability narrowing in the IR
/// pre-hash, etc.) lands here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapabilityRequirements {
    /// Declared capabilities; order is taken from the source TOML verbatim
    /// (capability list order is taken from the source TOML verbatim; two
    /// permuted capability arrays produce different canonical bytes. If
    /// order-independent hashing becomes required, sort in the lowering
    /// parse stage — `Capability` needs `Ord` first.)
    pub declared: Vec<Capability>,
}

/// Per-tool capability table for a `Workflow`.
///
/// Built by the lowering pass from per-tool TOML declarations; consumed
/// by the capability-fit check (D-3b) and embedded in the bundle's
/// `tau.caps` custom section (D-3).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapabilityTable(pub BTreeMap<ToolId, CapabilityRequirements>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{from_canonical_bytes, to_canonical_bytes};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::node::{Tool, ToolSpec};
    use crate::tool_impl::ToolImpl;
    use alloc::string::ToString;
    use serde_json::Value;
    use tau_domain::Capability;
    use tau_ports::target::registry;

    /// D7-B / issue #481: a stray field on a KNOWN capability kind must be
    /// rejected when the whole `IrModule` is decoded — not just when a bare
    /// `Capability` is deserialized. This walks the real interchange path
    /// (`from_canonical_bytes` → `IrModule` → `CapabilityRequirements` →
    /// `Vec<Capability>` → `Capability`'s strict custom `Deserialize`) to
    /// prove the strictness survives the IR wrapper types, which use derived
    /// `Deserialize` with no `deny_unknown_fields` of their own.
    #[test]
    fn unknown_field_on_known_kind_rejected_through_ir_module_decode() {
        let target = registry::list_available().next().unwrap().triple;
        let tool = Tool {
            id: ToolId("reader".to_string()),
            impl_: ToolImpl::Step {
                id: crate::ids::StepId("noop".to_string()),
            },
            capabilities: CapabilityRequirements {
                // `FsCapability` is `#[non_exhaustive]`; build the fs.read cap
                // through the same strict decode the interchange path uses.
                declared: alloc::vec![serde_json::from_str::<Capability>(
                    r#"{"kind":"fs.read","paths":["/x"]}"#
                )
                .unwrap()],
            },
            spec: ToolSpec {
                name: "reader".to_string(),
                description: "reads".to_string(),
                input_schema: Value::Null,
            },
        };
        let mut tools = BTreeMap::new();
        tools.insert(ToolId("reader".to_string()), tool);
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".to_string(),
            target,
            workflow: Workflow {
                tools,
                ..Workflow::default()
            },
            triggers: Vec::new(),
        };

        // Sanity: the clean module decodes.
        let bytes = to_canonical_bytes(&m);
        from_canonical_bytes(&bytes).expect("clean module round-trips");

        // Inject a stray key onto the fs.read capability and re-encode.
        let mut v: Value = serde_json::from_slice(&bytes).unwrap();
        let cap = &mut v["workflow"]["tools"]["reader"]["capabilities"]["declared"][0];
        assert_eq!(cap["kind"], "fs.read", "test setup: expected fs.read cap");
        cap.as_object_mut()
            .unwrap()
            .insert("bogus".to_string(), Value::from(1));
        let tampered = serde_json::to_vec(&v).unwrap();

        let err = from_canonical_bytes(&tampered)
            .expect_err("stray field on a known capability kind must fail decode");
        let msg = alloc::format!("{err}");
        assert!(
            msg.contains("fs.read") && msg.contains("bogus"),
            "decode error should name the kind and offending field; got: {msg}"
        );
    }
}
