//! The hook vocabulary that reaches the wire (ADR-0010 §1).
//!
//! Three entry kinds — `Attached`, `Verdicts`, `Emitted` — serialize these
//! types, and a frozen type is frozen through everything it serializes: a
//! `rename_all` on [`Ruling`] would change every `Verdicts` line without
//! touching [`Entry`](super::Entry). So the wire form of a hook lives here,
//! under the three gates, and the runtime half — what a hook *sees* and
//! *answers* — stays in [`crate::hook`], which re-exports these.

use core::fmt;

use serde::{Deserialize, Serialize};

use super::{AgentId, BlobRef, DimKey, HookId, Name};

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

/// One hook's verdict as recorded in the roll call of a `Verdicts` entry.
///
/// Reasons, notes, and errors are [`BlobRef`]s: payloads stay out of the
/// log, as everywhere else. This is the recorded form of
/// [`Verdict`](crate::hook::Verdict), which never reaches the log.
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
///
/// On the wire this is an array of pairs, `[[hook, ruling], …]`, in the
/// order the hooks answered (ADR-0010 §3). A map keyed by hook would lose
/// where the roll stopped and let a reader re-sort it.
pub type Roll = Vec<(HookId, Ruling)>;

/// How an installed program is recorded in its `Attached` entry: a native
/// by name, so the log is at least attributable; a rule by its source, so
/// the log is self-describing.
///
/// The rule's *grammar* is not frozen here: it is ADR-0008 §5's, amended
/// there. Only the field is on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSource {
    /// A native program, by the name the harness gave it.
    Native(Name),
    /// A rule, by its source text.
    Rule(String),
}
