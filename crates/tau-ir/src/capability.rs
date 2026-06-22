//! Capability requirements as carried in the IR.
//!
//! v0 wraps `tau_domain::Capability` (the existing source-of-truth shape) in
//! a `CapabilityTable` newtype keyed by [`crate::ToolId`]. Per the D-3b
//! decision, the lowering pass intersects this table against the target
//! triple's `supported_shapes` at build time and refuses the build on any
//! miss.

// schemars 0.8 derive generates code using bare `Box`/`String`/`vec!`
// from the std prelude — import it when the feature is active.
#[cfg(feature = "schema")]
#[allow(unused_imports)]
use std::prelude::rust_2021::*;

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
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<serde_json::Value>")
    )]
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
