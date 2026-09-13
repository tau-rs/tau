//! Kernel-allocated identifiers.
//!
//! Every id here is allocated by the kernel and is **never reused within a
//! run**. Non-reuse is what makes a log entry self-describing: a reader that
//! encounters `AgentId(7)` at sequence 900 knows it means the same agent it
//! meant at sequence 3, with no scoping rules to apply.

use core::fmt;

use serde::{Deserialize, Serialize};

macro_rules! kernel_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Wraps a raw value.
            ///
            /// Allocation is the kernel's job; this constructor exists for the
            /// allocator, for replay (which re-materializes ids read from the
            /// log), and for tests. Constructing one does not confer anything:
            /// authority lives in the kernel's holder table, never in a value.
            #[must_use]
            pub const fn new(raw: u64) -> Self {
                Self(raw)
            }

            /// The raw value.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

kernel_id! {
    /// Identifies an agent within a run. Allocated at `spawn`, never reused.
    AgentId, "agent:"
}

kernel_id! {
    /// Correlates a request with its replies.
    ///
    /// Allocated by the kernel at `send`, and the *only* link between a request
    /// and a reply — there is no other channel, no ambient context, and no
    /// out-of-band matching. A correlation is owned by the agent that created
    /// it; unclaimed correlations dead-letter when that agent exits.
    Corr, "corr:"
}

kernel_id! {
    /// Identifies an installed hook program, returned by `attach`.
    HookId, "hook:"
}

kernel_id! {
    /// A position in the log.
    ///
    /// The log is the kernel; everything else is cache. A sequence number is
    /// therefore an absolute coordinate: state at `Seq(n)` is a pure function
    /// of entries `0..=n`.
    Seq, "seq:"
}

/// Identifies a driver endpoint.
///
/// Drivers are named, not numbered, because the identifier must survive a
/// restart with a different driver set: a log replayed six months later has to
/// resolve `driver:anthropic` to the same endpoint regardless of what order
/// drivers registered in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DriverId(crate::abi::Name);

impl DriverId {
    /// Wraps a validated name.
    #[must_use]
    pub const fn new(name: crate::abi::Name) -> Self {
        Self(name)
    }

    /// The driver's name.
    #[must_use]
    pub fn name(&self) -> &crate::abi::Name {
        &self.0
    }
}

impl fmt::Display for DriverId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "driver:{}", self.0)
    }
}
