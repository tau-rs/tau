//! Capabilities, namespaces, and endpoints — the authority model.
//!
//! A capability is an address **and** a permission at once: holding one is what
//! it means to be allowed to talk to the thing it names. There is no separate
//! permission check, no ambient authority, and no way to name an endpoint you
//! do not hold.
//!
//! # What "unforgeable" actually means here
//!
//! [`Capability`] has no public constructor, which stops accidental forgery in
//! ordinary code. That is defence in depth, not the enforcement. The
//! enforcement is that **the kernel checks the sender's holder set on every
//! `send`**: an agent's authority is its birth namespace ∪ the capabilities it
//! has been transferred, both of which are kernel state derived from the log.
//! A value conjured from a deserialized log, a `transmute`, or a future
//! refactor still buys nothing, because the holder table — not the value —
//! is what grants.
//!
//! This distinction matters for replay: the log must round-trip capability
//! values faithfully, so they *are* deserializable. Security does not depend on
//! them not being.

use core::fmt;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{AgentId, DriverId};

/// An unforgeable grant: the address of an endpoint and the permission to send
/// to it, inseparably.
///
/// Obtained only from an agent's birth [`Namespace`] or by transfer in a
/// message. Transfer is a logged, hookable act, and a sender may only transfer
/// what it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capability(u64);

impl Capability {
    /// Mints a capability.
    ///
    /// Minting is an act of the kernel's capability allocator. Until that
    /// allocator exists (M0), the only public constructor is this one, gated
    /// behind the `testing` feature that this crate enables for its own test
    /// targets and nothing else enables at all.
    #[cfg(feature = "testing")]
    #[must_use]
    pub const fn mint(raw: u64) -> Self {
        Self(raw)
    }

    /// The opaque handle value, for logging and equality.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cap:{}", self.0)
    }
}

/// The set of capabilities an agent is born holding.
///
/// Snapshotted at `spawn` and immutable thereafter in the sense that matters:
/// it never changes *without a log entry*. A child's namespace must be a subset
/// of its parent's — authority can only narrow down the tree, never widen. That
/// single invariant is what makes prompt injection a contained failure rather
/// than a breach: a hijacked model in a search-only subtree can, at worst,
/// search.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Namespace {
    caps: BTreeSet<Capability>,
}

impl Namespace {
    /// An empty namespace: an agent that can do nothing at all.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Builds a namespace from an iterator of capabilities.
    pub fn from_caps<I: IntoIterator<Item = Capability>>(caps: I) -> Self {
        Self {
            caps: caps.into_iter().collect(),
        }
    }

    /// Whether this namespace grants `cap`.
    #[must_use]
    pub fn holds(&self, cap: Capability) -> bool {
        self.caps.contains(&cap)
    }

    /// Whether every capability here is also in `other`.
    ///
    /// The spawn precondition: `child.is_subset_of(parent)` must hold, or the
    /// spawn is refused.
    #[must_use]
    pub fn is_subset_of(&self, other: &Self) -> bool {
        self.caps.is_subset(&other.caps)
    }

    /// Iterates the capabilities in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.caps.iter().copied()
    }

    /// How many capabilities this namespace grants.
    #[must_use]
    pub fn len(&self) -> usize {
        self.caps.len()
    }

    /// Whether this namespace grants nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }
}

/// Who a message is from, or where it is bound.
///
/// Three kinds and no fourth: agents, drivers, and the harness. The harness is
/// deliberately *not* an agent — it is the pre-agent code that holds the kernel
/// handle and owns the `attach` privilege. Writing that down is how the design
/// resists a future elegance that would make it one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Endpoint {
    /// An agent in the tree.
    Agent {
        /// Which agent.
        id: AgentId,
    },
    /// A driver: the only border with the world.
    Driver {
        /// Which driver.
        id: DriverId,
    },
    /// The harness itself.
    Harness,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent { id } => write!(f, "{id}"),
            Self::Driver { id } => write!(f, "{id}"),
            Self::Harness => f.write_str("harness"),
        }
    }
}
