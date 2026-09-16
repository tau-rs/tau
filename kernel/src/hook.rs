//! Hooks: mandatory, cross-cutting policy consulted at five pinned points
//! (ADR-0008).
//!
//! A hook is a program the harness installs with [`Kernel::attach`] before
//! the root agent exists. At each [`HookPoint`] the kernel builds one
//! [`HookEvent`] — plain data, no handles into kernel state — hands it to
//! every hook attached there, in [`HookId`](crate::abi::HookId) order, under the syscall's lock,
//! and stops at the first `Deny`. What the hooks answered is one `Verdicts`
//! entry in the log; what an `Emit` produced is an `Emitted` entry of its
//! own. The fold applies those entries and never runs a program: a log whose
//! `Attached` entries name a native hook this binary does not have refolds
//! to the same state hash.
//!
//! # Two tiers
//!
//! [`HookProgram::Native`] is the harness author's own closure: trusted,
//! synchronous, unmetered, and run under the kernel lock, so it must not do
//! I/O, `await`, take a lock of its own, call back into the kernel, or
//! panic. [`HookProgram::Rule`] is the one-line declarative language of
//! ADR-0008 §5: parsed at `attach`, total and stateless at the point, so
//! it can never fail at runtime. See [`rule`].
//!
//! [`Kernel::attach`]: crate::kernel::Kernel::attach

pub mod rule;

use core::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::abi::{AgentId, Budget, Corr, DimKey, DriverId, MsgKind, Name, Namespace, Seq};
use crate::reducer::Outcome;

pub use crate::abi::{FailureMode, HookPoint, HookSource, Roll, Ruling};
pub use rule::{Rule, RuleError};

/// What a hook sees: one event per moment, the same for every tier.
///
/// Plain data — facts the reducer already holds and bytes already in memory
/// at that moment. `seq` is the position of the moment: the `Verdicts` entry
/// it produces at a pre point, the entry that caused it at an on point.
/// `remaining` is the subject's grant before the reservation or settle the
/// event is about; `payload` is the bytes, opaque, which the kernel copies
/// in and never reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookEvent {
    /// See [`HookPoint::PreSend`].
    PreSend {
        /// The moment's position.
        seq: Seq,
        /// The sender.
        subject: AgentId,
        /// Its parent; `None` for the root.
        parent: Option<AgentId>,
        /// Its `depth` grant.
        depth: u64,
        /// The driver the capability names.
        driver: DriverId,
        /// The correlation the request will carry.
        corr: Corr,
        /// The request bytes.
        payload: Vec<u8>,
        /// The sender's grant before the reservation.
        remaining: Budget,
    },
    /// See [`HookPoint::PreDeliver`].
    PreDeliver {
        /// The moment's position.
        seq: Seq,
        /// The owner of the correlation.
        subject: AgentId,
        /// Its parent; `None` for the root.
        parent: Option<AgentId>,
        /// Its `depth` grant.
        depth: u64,
        /// The driver that answered.
        driver: DriverId,
        /// The correlation answered.
        corr: Corr,
        /// `Reply` or `Partial`.
        kind: MsgKind,
        /// The reply bytes.
        payload: Vec<u8>,
        /// The owner's grant before the settle.
        remaining: Budget,
    },
    /// See [`HookPoint::OnSpawn`].
    OnSpawn {
        /// The moment's position.
        seq: Seq,
        /// The child about to be born.
        subject: AgentId,
        /// The spawning agent; `None` for the root.
        parent: Option<AgentId>,
        /// The depth the child is born at.
        depth: u64,
        /// The child's birth namespace.
        ns: Namespace,
        /// The grant asked for the child.
        remaining: Budget,
    },
    /// See [`HookPoint::OnExit`].
    OnExit {
        /// The position of the entry that finished the agent.
        seq: Seq,
        /// The finished agent.
        subject: AgentId,
        /// Its parent; `None` for the root.
        parent: Option<AgentId>,
        /// The `depth` grant it held.
        depth: u64,
        /// How it finished.
        outcome: Outcome,
        /// Its result bytes; `None` for an abort, because there was none.
        result: Option<Vec<u8>>,
        /// What it held, budget and reservations, when it finished.
        unspent: Budget,
    },
    /// See [`HookPoint::OnBudget`].
    OnBudget {
        /// The position of the entry that moved the grant.
        seq: Seq,
        /// The agent whose grant crossed.
        subject: AgentId,
        /// Its parent; `None` for the root.
        parent: Option<AgentId>,
        /// Its `depth` grant.
        depth: u64,
        /// The dimension that crossed.
        dim: DimKey,
        /// The line it crossed.
        below: u64,
        /// The grant before the entry moved it.
        remaining: Budget,
    },
}

impl HookEvent {
    /// The agent this event is about.
    #[must_use]
    pub fn subject(&self) -> AgentId {
        match self {
            Self::PreSend { subject, .. }
            | Self::PreDeliver { subject, .. }
            | Self::OnSpawn { subject, .. }
            | Self::OnExit { subject, .. }
            | Self::OnBudget { subject, .. } => *subject,
        }
    }

    /// The position of the moment.
    #[must_use]
    pub fn seq(&self) -> Seq {
        match self {
            Self::PreSend { seq, .. }
            | Self::PreDeliver { seq, .. }
            | Self::OnSpawn { seq, .. }
            | Self::OnExit { seq, .. }
            | Self::OnBudget { seq, .. } => *seq,
        }
    }
}

/// A hook's answer (ADR-0008 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No objection.
    Allow,
    /// Stop it, for this reason. Admitted at pre points only; at an on
    /// point it is recorded as a program failure.
    Deny(String),
    /// Let it through, and leave a note: a `Notice` from
    /// `Endpoint::Hook { id }` is delivered to `to`'s mailbox, or
    /// dead-lettered if `to` is not live.
    Emit {
        /// The recipient.
        to: AgentId,
        /// The note's bytes.
        payload: Vec<u8>,
    },
}

/// A native hook could not produce a verdict. What that counts as is the
/// hook's [`FailureMode`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("hook failed: {message}")]
pub struct HookFailure {
    /// What went wrong, for the log.
    pub message: String,
}

impl HookFailure {
    /// A failure with this message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// What the reducer keeps for each installed hook: the `Attached` entry,
/// minus its position. Canonical state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRecord {
    /// Where it is consulted.
    pub point: HookPoint,
    /// What a failure counts as.
    pub failure: FailureMode,
    /// What it is.
    pub program: HookSource,
}

/// A native hook's body.
pub type NativeFn = dyn Fn(&HookEvent) -> Result<Verdict, HookFailure> + Send + Sync;

/// A program to install (ADR-0008 §5).
#[derive(Clone)]
pub enum HookProgram {
    /// Trusted harness code. Synchronous, unmetered, sees the whole event.
    Native {
        /// The name the log records it under.
        name: Name,
        /// The body.
        run: Arc<NativeFn>,
    },
    /// Declarative, total, stateless, linear. Parsed at `attach`.
    Rule(Rule),
}

impl HookProgram {
    /// A native program with this name and body.
    pub fn native<F>(name: Name, run: F) -> Self
    where
        F: Fn(&HookEvent) -> Result<Verdict, HookFailure> + Send + Sync + 'static,
    {
        Self::Native {
            name,
            run: Arc::new(run),
        }
    }

    /// How this program is recorded in the log.
    #[must_use]
    pub fn source(&self) -> HookSource {
        match self {
            Self::Native { name, .. } => HookSource::Native(name.clone()),
            Self::Rule(rule) => HookSource::Rule(rule.source().to_owned()),
        }
    }

    /// Consults the program.
    ///
    /// # Errors
    ///
    /// A native program's own failure. A rule cannot fail.
    pub fn run(&self, event: &HookEvent) -> Result<Verdict, HookFailure> {
        match self {
            Self::Native { run, .. } => run(event),
            Self::Rule(rule) => Ok(rule.evaluate(event)),
        }
    }
}

impl fmt::Debug for HookProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native { name, .. } => f.debug_struct("Native").field("name", name).finish(),
            Self::Rule(rule) => f.debug_tuple("Rule").field(rule).finish(),
        }
    }
}

/// The budget crossings an entry caused (ADR-0008 §1, `OnBudget`).
///
/// `points` are the `OnBudget` points with a hook attached; `before` is the
/// remaining grant of each candidate agent before the entry; `after` reads
/// each one's grant now. A crossing is `at or above` becoming `under`, on a
/// dimension both sides hold: an agent that finished in the same entry has
/// no grant left and no crossing. Returned in point order, then agent order
/// — one moment per (point, subject), each with the grant before the move.
///
/// Pure and shared: the kernel calls it to fire the point, the sim to write
/// the same roll calls the kernel would.
pub fn crossings<'a, P, F>(
    points: P,
    before: &[(AgentId, Budget)],
    after: F,
) -> Vec<(HookPoint, AgentId, Budget)>
where
    P: IntoIterator<Item = &'a HookPoint>,
    F: Fn(AgentId) -> Option<&'a Budget>,
{
    let mut fired = Vec::new();
    for point in points {
        let HookPoint::OnBudget { dim, below } = point else {
            continue;
        };
        for (agent, was) in before {
            let (Some(had), Some(has)) = (was.get(dim), after(*agent).and_then(|b| b.get(dim)))
            else {
                continue;
            };
            if had >= *below && has < *below {
                fired.push((point.clone(), *agent, was.clone()));
            }
        }
    }
    fired
}
