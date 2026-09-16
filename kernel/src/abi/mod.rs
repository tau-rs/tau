//! The frozen ABI — this module is the constitution.
//!
//! # The rule
//!
//! Everything in this module is a "we do not break userspace" surface. It
//! evolves **additively** or not at all: new fields may be added with a
//! defaulted deserialization, new enum variants may be added to non-exhaustive
//! enums, and [`ABI`] is bumped when either happens. Nothing is ever removed,
//! renamed, reordered into a different wire position, or given a new meaning.
//!
//! Changes here are gated three ways, deliberately redundantly:
//!
//! 1. `CODEOWNERS` requires an explicit owner review for `kernel/src/abi/`.
//! 2. CI's ABI guard fails any diff under this directory unless the pull
//!    request also bumps [`ABI`] or carries the `abi-change` label with a
//!    linked ADR.
//! 3. `insta` snapshots pin the serialized form of every type here, so a
//!    change to the wire format cannot be made silently — only deliberately,
//!    by accepting a snapshot.
//!
//! # Why one directory and not a rulebook
//!
//! tau v1 governed stability with 47 lettered guidelines across three
//! documents and still had no frozen interface anywhere. This module is the
//! correction: one directory, three gates, and a version number. See
//! `docs/adr/0001-tau-rebooted.md`.
//!
//! # Determinism
//!
//! Every collection here is order-stable ([`BTreeMap`](std::collections::BTreeMap),
//! [`BTreeSet`](std::collections::BTreeSet)). Hash-ordered containers are
//! denied workspace-wide by `clippy.toml`, because their iteration order leaks
//! into the reducer's state hash and surfaces as a cross-platform replay
//! divergence far from the commit that caused it.

mod budget;
mod cap;
mod entry;
mod hook;
mod ids;
mod msg;
mod name;
mod snapshot;

pub use budget::{Budget, BudgetError, Consumption, DimKey};
pub use cap::{Capability, Endpoint, Namespace};
pub use entry::Entry;
pub use hook::{FailureMode, HookPoint, HookSource, Roll, Ruling};
pub use ids::{AgentId, Corr, DriverId, HookId, Seq};
pub use msg::{BlobRef, BlobRefError, LogHeader, Msg, MsgKind};
pub use name::{Name, NameError, NAME_MAX_LEN};
pub use snapshot::SnapshotHeader;

/// The ABI version carried by every [`Msg`] and every log header.
///
/// Bumped only for an additive change to this module. `tau 1.0` means this
/// number is frozen for good; until then it moves, but never silently — see
/// the module docs for the three gates.
///
/// | ABI | Change | ADR |
/// |---|---|---|
/// | 0 | the first freeze | ADR-0004 |
/// | 1 | `Endpoint::Hook` | ADR-0008 |
/// | 2 | `Entry` and the hook wire types join this module; no byte changes, but 2 is the first number that identifies the entry format | ADR-0010 |
pub const ABI: u16 = 2;
