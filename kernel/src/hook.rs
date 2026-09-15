//! Hooks: mandatory, cross-cutting policy consulted at five pinned points
//! (ADR-0008).
//!
//! A hook is a program the harness installs with [`Kernel::attach`] before
//! the root agent exists. At each [`HookPoint`] the kernel builds one
//! [`HookEvent`] — plain data, no handles into kernel state — hands it to
//! every hook attached there, in [`HookId`] order, under the syscall's lock,
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

use crate::abi::{
    AgentId, BlobRef, Budget, Corr, DimKey, DriverId, HookId, MsgKind, Name, Namespace, Seq,
};
use crate::reducer::Outcome;

pub use rule::{Rule, RuleError};

/// A pinned moment where hooks are consulted (ADR-0008 §1).
///
/// The *pre* points — `PreSend`, `PreDeliver`, `OnSpawn` — fire before the
/// governed entry is committed and admit `Deny`. The *on* points — `OnExit`,
/// `OnBudget` — fire after the fact and admit only `Allow` and `Emit`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    /// An agent's request has passed the authority and budget checks and is
    /// about to be logged as `Sent`. Subject: the sender.
    PreSend,
    /// A driver's reply is about to be logged as `Replied` and enter its
    /// owner's mailbox. Subject: the owner. Kernel-originated notices — a
    /// cancel, a hook's own `Emit` — are not subject to this point.
    PreDeliver,
    /// A `spawn` has passed the subset and carve checks and is about to be
    /// logged as `Spawned`; the root included. Subject: the child.
    OnSpawn,
    /// An agent has finished: `Exited` applied, or aborted inside the apply
    /// of a `Tick` or a zero-grace `Cancelled`. Subject: the finished agent.
    OnExit,
    /// Applying an entry moved the subject's remaining grant on `dim` from at
    /// or above `below` to under it. Edge-triggered: once per crossing, and
    /// again if the grant rises back over the line and crosses it again.
    OnBudget {
        /// The dimension watched.
        dim: DimKey,
        /// The absolute remaining amount that is the line.
        below: u64,
    },
}

impl HookPoint {
    /// Whether a hook here may stop the governed entry: the pre points.
    #[must_use]
    pub const fn admits_deny(&self) -> bool {
        matches!(self, Self::PreSend | Self::PreDeliver | Self::OnSpawn)
    }
}

impl fmt::Display for HookPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreSend => f.write_str("pre_send"),
            Self::PreDeliver => f.write_str("pre_deliver"),
            Self::OnSpawn => f.write_str("on_spawn"),
            Self::OnExit => f.write_str("on_exit"),
            Self::OnBudget { dim, below } => write!(f, "on_budget({dim}, {below})"),
        }
    }
}

/// What a verdict a hook *failed to produce* counts as (ADR-0008 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    /// A failure is an `Allow`. Refused at a pre point: "if my guard breaks,
    /// let everything through" is not a policy anyone writes on purpose.
    Open,
    /// A failure is a `Deny`.
    Closed,
}

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

/// One hook's verdict as recorded in the roll call of a `Verdicts` entry.
///
/// Reasons, notes, and errors are [`BlobRef`]s: payloads stay out of the
/// log, as everywhere else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ruling {
    /// No objection.
    Allow,
    /// Stopped it; the reason.
    Deny(BlobRef),
    /// Let it through and left a note, which is the `Emitted` entry that
    /// follows.
    Emit {
        /// The recipient.
        to: AgentId,
        /// The note.
        payload: BlobRef,
    },
    /// The hook failed to answer — or answered `Deny` at an on point, which
    /// is the same failure — and this is what the failure counted as.
    Failed {
        /// The hook's failure mode.
        mode: FailureMode,
        /// The error.
        error: BlobRef,
    },
}

impl Ruling {
    /// Whether this ruling stops the roll call at `point`: a `Deny`, or a
    /// failure that closed. At an on point nothing stops it — the mode is
    /// recorded and does not matter.
    #[must_use]
    pub fn stops(&self, point: &HookPoint) -> bool {
        point.admits_deny()
            && matches!(
                self,
                Self::Deny(_)
                    | Self::Failed {
                        mode: FailureMode::Closed,
                        ..
                    }
            )
    }
}

/// The roll call of one moment: every hook consulted, in [`HookId`] order,
/// with its ruling. Nothing follows a ruling that stopped it.
pub type Roll = Vec<(HookId, Ruling)>;

/// How an installed program is recorded in its `Attached` entry: a native
/// by name, so the log is at least attributable; a rule by its source, so
/// the log is self-describing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSource {
    /// A native program, by the name the harness gave it.
    Native(Name),
    /// A rule, by its source text.
    Rule(String),
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
