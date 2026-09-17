//! The log line, frozen (ADR-0010).
//!
//! A log is a [`LogHeader`](super::LogHeader) followed by one [`Entry`] per
//! line, and this is the enum every line is. It moved here from `log.rs`
//! when the replay CLI needed the line format to be a promise rather than a
//! snapshot of whatever the reducer happened to emit: from `ABI` 2, a
//! reader that accepts the header may assume every line is one of the kinds
//! below, in the shape the `entry_*` snapshots pin.
//!
//! Position lives in two places on purpose. Eleven kinds carry a top-level
//! `seq`; `Sent`, `Replied`, `Emitted` and `Unanswered` carry a [`Msg`], and
//! a message's position *is* its envelope's `seq` — duplicating it would be
//! a second source of truth for the reducer's `OutOfOrder` refusal to
//! disagree with. [`Entry::seq`] is the accessor and its rule is frozen with
//! the shape.
//!
//! The enum is `#[non_exhaustive]`: a new kind is an additive ABI event, not
//! a source break for a reader outside the crate. The reducer's `apply`
//! stays exhaustive inside the crate, so a kind with no arm is a compile
//! error rather than a silent no-op. ABI 3 (ADR-0014) added three: the
//! driver health transitions and the entry that closes a request its driver
//! never answered.

use serde::{Deserialize, Serialize};

use super::{
    AgentId, BlobRef, Budget, Capability, DriverId, FailureMode, HookId, HookPoint, HookSource,
    Msg, Namespace, Roll, Seq,
};

/// Why a driver went down (ADR-0014 §2). A closed tag, never text: a panic
/// message or a retirement reason is the harness's log line, not the
/// kernel's, because the kernel's log cannot be shredded and a panic message
/// can carry anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DownCause {
    /// Its `handle` unwound. The kernel's loop observed it.
    Crashed,
    /// The harness retired or replaced it.
    Retired,
}

/// Why a request closed without its driver's answer (ADR-0014 §1). Which
/// one says nothing about the bill: that is chosen from whether the driver
/// had taken the request (§3), and the two are orthogonal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnansweredCause {
    /// The driver's `handle` unwound while the request was taken.
    Crashed,
    /// The request passed the driver's registered `reply_within`, counted
    /// from its `Sent`. The driver is not declared down for it.
    Overdue,
    /// The harness retired or replaced the driver while the request was
    /// open, taken or queued.
    Retired,
}

/// One effect, as recorded.
///
/// Each variant carries the kernel-allocated ids it introduces (`agent`, `cap`,
/// the `corr` inside a `msg`), so the reducer can *verify* an allocation on
/// replay rather than re-deriving it and hoping it matches. What a variant
/// does *not* carry is anything the reducer can derive: `Cancelled` names the
/// subtree's root, not its members, and a hard abort is not an entry at all
/// but a consequence of applying the `Tick` that reaches the deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Entry {
    /// A driver endpoint was registered at boot and a capability minted for it.
    DriverRegistered {
        /// Log position.
        seq: Seq,
        /// The driver.
        driver: DriverId,
        /// The capability that names it.
        cap: Capability,
        /// The most one request to this driver may cost, as declared by the
        /// harness. Every `Sent` through `cap` reserves this much from the
        /// sender before delivery; the `Replied` settles against it.
        ceiling: Budget,
        /// The most clock units a request to this driver may wait for its
        /// answer, counted from its `Sent`, in `Tick.now`'s units. `None` —
        /// the default, and what every log written before ABI 3 reads as —
        /// is unbounded. The harness's declaration, like `ceiling`, and on
        /// the wire for the same reason: an `Unanswered { overdue }` must be
        /// legible without the harness's source (ADR-0014 §2). The fold does
        /// not store it; the kernel enforces it at `tick`.
        #[serde(default)]
        reply_within: Option<u64>,
    },
    /// An agent was created.
    Spawned {
        /// Log position.
        seq: Seq,
        /// The spawning agent, or `None` for the root, which the harness spawns.
        parent: Option<AgentId>,
        /// The new agent.
        agent: AgentId,
        /// Its birth namespace — a subset of the parent's.
        ns: Namespace,
        /// Its grant — carved atomically from the parent's.
        budget: Budget,
    },
    /// An agent sent a request through a capability it holds.
    Sent {
        /// The envelope. `msg.seq` is this entry's position.
        msg: Msg,
        /// The capability the sender used; the reducer resolves it to an
        /// endpoint, and the log keeps the evidence of authority.
        via: Capability,
    },
    /// A driver answered a request.
    Replied {
        /// The envelope, carrying the driver's consumption report.
        msg: Msg,
        /// The owner of the correlation — where the reply is delivered.
        to: AgentId,
    },
    /// A `recv` resolved: one message left an agent's mailbox.
    ///
    /// The filter is not logged, only the outcome. Replay does not re-evaluate
    /// user intent; it re-applies what happened.
    Resolved {
        /// Log position.
        seq: Seq,
        /// The receiving agent.
        agent: AgentId,
        /// The position of the message that matched.
        matched: Seq,
    },
    /// An agent's last act.
    Exited {
        /// Log position.
        seq: Seq,
        /// The agent.
        agent: AgentId,
        /// Its result, held until claimed.
        result: BlobRef,
    },
    /// A stored outcome was claimed: by the parent through `wait`, or by the
    /// harness for a result the tree left behind.
    Claimed {
        /// Log position.
        seq: Seq,
        /// Whose outcome.
        agent: AgentId,
        /// The claimant, or `None` for the harness.
        by: Option<AgentId>,
    },
    /// An agent's subtree was cancelled: phase one of `cancel`, the atomic
    /// freeze. Applying this entry freezes every live agent under `agent`,
    /// delivers each a `Notice` from `by` carrying `reason`, and sets each one's
    /// deadline to the current clock reading plus `grace`.
    Cancelled {
        /// Log position.
        seq: Seq,
        /// The canceller, or `None` for the harness.
        by: Option<AgentId>,
        /// The root of the cancelled subtree.
        agent: AgentId,
        /// How many clock units the subtree has to exit on its own.
        grace: u64,
        /// The payload of the notice each frozen agent receives.
        reason: BlobRef,
    },
    /// The clock advanced. Time enters the system only this way (ADR-0003):
    /// applying a tick is when wall budgets are charged and cancel deadlines
    /// are enforced.
    Tick {
        /// Log position.
        seq: Seq,
        /// The clock reading. Non-decreasing across a log.
        now: u64,
    },
    /// A hook program was installed at boot, before any agent existed
    /// (ADR-0008 §4). The registry is these entries, at the head of the log.
    Attached {
        /// Log position.
        seq: Seq,
        /// The hook, allocated by the kernel at `attach`.
        hook: HookId,
        /// Where it is consulted.
        point: HookPoint,
        /// What a verdict it fails to produce counts as.
        failure: FailureMode,
        /// What it is: a native by name, a rule by source.
        program: HookSource,
    },
    /// The hooks at `point` were consulted about `subject`, and this is what
    /// each answered, in `HookId` order, stopping at the first `Deny`
    /// (ADR-0008 §3). At a pre point this precedes the governed entry — or
    /// nothing, if a hook denied it; at an on point it follows the entry that
    /// caused the moment. A point with no hook attached writes nothing.
    ///
    /// The fold confirms the roll call and never runs a program: the effect
    /// of a `Deny` is the absence of the next entry, and the effect of an
    /// `Emit` is the `Emitted` entry that follows.
    Verdicts {
        /// Log position.
        seq: Seq,
        /// The point consulted.
        point: HookPoint,
        /// The agent the moment was about.
        subject: AgentId,
        /// Who answered what.
        roll: Roll,
    },
    /// A hook's `Emit` verdict produced a notice. Its own entry, because a
    /// notice is a message and every message has its own position: two
    /// notices sharing a `seq` in one mailbox would make a `Resolved`
    /// ambiguous. Delivered to `to` if live; dead-lettered otherwise.
    Emitted {
        /// The hook that emitted it.
        hook: HookId,
        /// The recipient.
        to: AgentId,
        /// The envelope, from `Endpoint::Hook`. `msg.seq` is this entry's
        /// position.
        msg: Msg,
    },
    /// A driver went down (ADR-0014 §2): its loop observed `handle`
    /// unwinding, or the harness retired or replaced it. A health
    /// transition the fold confirms and applies as nothing — health is the
    /// kernel's cache, not canonical state (§4).
    DriverDown {
        /// Log position.
        seq: Seq,
        /// The driver.
        driver: DriverId,
        /// Why.
        cause: DownCause,
    },
    /// A driver came back: the harness installed a fresh instance under the
    /// same id and the same capability. Only ever a return — a driver is up
    /// from `DriverRegistered`. Applied as nothing, like `DriverDown`.
    DriverUp {
        /// Log position.
        seq: Seq,
        /// The driver.
        driver: DriverId,
    },
    /// A request closed without its driver's answer (ADR-0014 §2–§3). One
    /// entry per request, so every message keeps its own position. Carries
    /// the envelope the owner receives, exactly as `Replied` does: `from`
    /// is `Endpoint::Kernel`, `kind` is `Reply` so a `recv` on the
    /// correlation matches it, `payload` is empty — the kernel authors no
    /// bytes — and `consumed` is the driver's ceiling if the driver had
    /// taken the request, `None` if it was still queued. The fold settles it
    /// with the same arithmetic as a `Replied`.
    Unanswered {
        /// The envelope, from `Endpoint::Kernel`. `msg.seq` is this entry's
        /// position.
        msg: Msg,
        /// The owner of the correlation — where the reply is delivered.
        to: AgentId,
        /// The driver that did not answer. Evidence, like `via` on `Sent`;
        /// the fold confirms it is registered and nothing more.
        driver: DriverId,
        /// Why it did not.
        cause: UnansweredCause,
    },
}

impl Entry {
    /// This entry's position in the log.
    #[must_use]
    pub fn seq(&self) -> Seq {
        match self {
            Self::DriverRegistered { seq, .. }
            | Self::Spawned { seq, .. }
            | Self::Resolved { seq, .. }
            | Self::Exited { seq, .. }
            | Self::Claimed { seq, .. }
            | Self::Cancelled { seq, .. }
            | Self::Tick { seq, .. }
            | Self::Attached { seq, .. }
            | Self::Verdicts { seq, .. }
            | Self::DriverDown { seq, .. }
            | Self::DriverUp { seq, .. } => *seq,
            Self::Sent { msg, .. }
            | Self::Replied { msg, .. }
            | Self::Emitted { msg, .. }
            | Self::Unanswered { msg, .. } => msg.seq,
        }
    }
}
