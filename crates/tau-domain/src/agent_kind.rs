//! Per-kind agent definitions (ADR-0024): a named, spawnable agent *kind*
//! carrying its own capability set.
//!
//! This is the static `kind → capabilities` map whose absence deferred
//! build-time `agent ⊇ spawn` enforcement in EPIC 1 story 1.5 ("no static
//! kind→agent map"). EPIC 4.4 uses it to check, at `tau check` time, that a
//! spawned kind's caps ⊆ the spawning agent's / dynamic region's effective
//! caps via the sound `capability_subset` lattice primitive.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Capability;

/// A named spawnable agent kind and its capability set.
///
/// The `name` matches the string used in `Agent(Spawn { allowed_kinds })`
/// and in a dynamic region's `spawns` list. `capabilities` is the kind's
/// grant, authored as raw caps in `[agent.kinds.<name>]`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct AgentKind {
    /// The kind name (referenced by spawn allow-lists and dynamic regions).
    pub name: String,
    /// The kind's capability grant.
    pub capabilities: Vec<Capability>,
}

impl AgentKind {
    /// Construct a kind from a name and its capability grant.
    pub fn new(name: String, capabilities: Vec<Capability>) -> Self {
        Self { name, capabilities }
    }
}
