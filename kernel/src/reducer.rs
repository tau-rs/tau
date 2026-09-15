//! The reducer: `fn apply(state, entry) -> state`, and nothing else.
//!
//! This is the constitution of ADR-0003 made executable. [`State`] is a pure
//! function of the log: no clock, no randomness, no hash-ordered iteration.
//! `clippy.toml` denies the types and methods that would break that, so the
//! compiler reviews this file before a human does.
//!
//! # Check, then apply
//!
//! Every entry is [checked](State::check) against the current state before it
//! is [applied](State::apply). The kernel runs the check *before* appending, so
//! a refused syscall never reaches the log; the fold runs it again on replay,
//! so a log that was tampered with — or written by a buggy kernel — is refused
//! at the first entry that could not have happened.
//!
//! # Allocation
//!
//! Every kernel-allocated id (agent, correlation, capability, sequence) is a
//! monotonic counter in this state. An entry carries the id it was allocated,
//! and the check verifies it equals the counter. Replay therefore does not
//! re-derive ids and hope; it confirms them.
//!
//! # Time
//!
//! The reducer never reads a clock. Time is a [`Tick`](Entry::Tick) entry, and
//! [`State::now`] is the reading of the last one applied. A cancel deadline is
//! an absolute reading on the agent's record, and the hard abort happens
//! *inside* the apply of the tick that reaches it — so a fold reproduces every
//! abort exactly, and nothing outside the log can postpone one.
//!
//! Wall time is a budget dimension the clock spends on the agent's behalf:
//! applying a tick moves the elapsed reading out of every live agent's
//! `wall_ms` grant, and the tick that empties one aborts that agent. An agent
//! with no `wall_ms` grant is *untimed* — the clock charges it nothing — which
//! is the one place "absent" does not mean "cannot spend", because here the
//! spender is the clock, not the agent. The exception cannot be used to
//! escape a limit: a child of a timed parent must itself be timed.
//!
//! # Budgets
//!
//! Every reserved dimension is enforced. `spawn` carves the child's grant
//! atomically from the parent's, except `depth`, which is not a resource but
//! a shape limit: a child is born one level shallower than its parent, or
//! shallower still if asked, a parent at zero cannot spawn, and nothing
//! comes back at exit — a parent's level is unchanged by a child's death. `send`
//! reserves the driver's declared ceiling plus one `calls` *before* delivery,
//! so a request that could not be paid for is refused rather than overdrawn;
//! the reply settles the reservation against what the driver reports. A
//! report above the ceiling is charged in full and the excess recorded as
//! [`overdraft`](Agent::overdraft) — never taken from budget the agent still
//! holds, because the ceiling was the harness's declaration, not the agent's.
//!
//! # Hooks
//!
//! The fold never runs a hook program (ADR-0008 §3). An `Attached` entry is
//! the registry; a `Verdicts` entry is confirmed — every hook named is
//! attached at that point, in `HookId` order, nothing after a `Deny` — and
//! applied as nothing, because the effect of a `Deny` is the absence of the
//! next entry; an `Emitted` entry is a notice delivered to a live mailbox and
//! dropped otherwise. A log whose `Attached` entries name a native this
//! binary does not have refolds to the same hash.
//!
//! The invariant every test of this module leans on: over the whole tree, at
//! every step, budgets plus reservations plus spent sum to the root's grant
//! (plus overdraft, zero in a healthy run), along every dimension but `depth`.
//! Unspent budget returns to the nearest *live* ancestor at exit — or to the
//! root's record if there is none — so it is never stranded on a dead one,
//! and the order in which a family exits does not change where it ends up.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::abi::{
    AgentId, BlobRef, Budget, BudgetError, Capability, Consumption, Corr, DimKey, DriverId,
    Endpoint, HookId, Msg, MsgKind, Namespace, Seq, ABI,
};
use crate::blob::sha256;
use crate::hook::{FailureMode, HookPoint, HookRecord, Roll, Ruling};
use crate::log::Entry;

/// Where an agent is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Running, unrestricted.
    Live,
    /// Cancelled and in its grace period: it may `recv`, `wait`, and `exit`,
    /// but may not `spawn`, `send`, or `cancel`. Hard-aborted at its deadline.
    Cancelling,
    /// Exited on its own; its record persists for accounting.
    Exited,
    /// Hard-aborted by the kernel at a cancel deadline; its record persists.
    Aborted,
}

/// How an agent finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It called `exit`; this is its result.
    Exited(BlobRef),
    /// The kernel aborted it at a cancel deadline. There is no result.
    Aborted,
}

/// A finished agent whose outcome has not been claimed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    /// The agent that finished.
    pub agent: AgentId,
    /// Its parent, who may `wait` for it. `None` for the root: the harness
    /// claims that one.
    pub parent: Option<AgentId>,
    /// How it finished.
    pub outcome: Outcome,
}

/// Everything the kernel knows about one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// The spawning agent; `None` for the root.
    pub parent: Option<AgentId>,
    /// The birth namespace. Authority is this set — there is no transfer yet.
    pub ns: Namespace,
    /// What remains of the grant and is not held for an open request.
    pub budget: Budget,
    /// Budget held for each open request: the driver's ceiling, carved at
    /// `Sent` and settled at `Replied`. Released when the agent finishes.
    pub reserved: BTreeMap<Corr, Budget>,
    /// Everything charged to this agent, every dimension: driver reports,
    /// one `calls` per send, and wall time as the clock passes. For an
    /// untimed agent `wall_ms` here is a measurement, not a charge.
    pub spent: BTreeMap<DimKey, u64>,
    /// The part of [`spent`](Self::spent) no grant covered: a driver reported
    /// more than its ceiling. Visible so a misreport is loud in the state
    /// hash; driver supervision (M3) is what will act on it.
    pub overdraft: BTreeMap<DimKey, u64>,
    /// Where it is in its life.
    pub status: Status,
    /// Delivered, unresolved messages, in delivery order.
    pub mailbox: Vec<Msg>,
    /// While [`Status::Cancelling`]: the clock reading at which the kernel
    /// hard-aborts it. Set by `cancel`, only ever brought earlier, never later.
    pub deadline: Option<u64>,
}

impl Agent {
    fn is_live(&self) -> bool {
        matches!(self.status, Status::Live | Status::Cancelling)
    }

    fn is_frozen(&self) -> bool {
        matches!(self.status, Status::Cancelling)
    }

    /// Whether this agent holds a `wall_ms` grant at all.
    fn is_timed(&self) -> bool {
        self.budget.get(&DimKey::WallMs).is_some()
    }
}

/// The kernel's state: a pure fold over the log.
///
/// Only the canonical fields are serialized, so [`State::hash`] is a function
/// of the log alone: the derived indexes are skipped, and `completed` goes
/// out as a sequence in completion order without its ordinal keys. On
/// deserialize the indexes are rebuilt from the canonical fields via
/// [`Canonical`], and the ordinals are compacted to `0..n`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Canonical")]
pub struct State {
    next_seq: u64,
    next_agent: u64,
    next_corr: u64,
    next_cap: u64,
    next_hook: u64,
    now: u64,
    drivers: BTreeMap<DriverId, Capability>,
    /// What one request to each driver may cost at most, as registered.
    ceilings: BTreeMap<DriverId, Budget>,
    caps: BTreeMap<Capability, Endpoint>,
    /// Every installed hook, by id: the `Attached` entries at the head of
    /// the log. Canonical, so the hash says which policy governed the run.
    hooks: BTreeMap<HookId, HookRecord>,
    agents: BTreeMap<AgentId, Agent>,
    corrs: BTreeMap<Corr, AgentId>,
    /// Finished agents whose outcome is unclaimed, keyed by the order they
    /// finished in: `wait(Any)` returns in completion order, which is not id
    /// order. Serialized as the sequence of completions; the keys are an
    /// implementation detail and stay out of the hash.
    #[serde(serialize_with = "completions_in_order")]
    completed: BTreeMap<u64, Completion>,
    /// The completion ordinal each unclaimed outcome sits under, so a claim
    /// finds it without scanning.
    #[serde(skip)]
    unclaimed: BTreeMap<AgentId, u64>,
    /// The ordinal the next finish will take. Never reused, so completion
    /// order survives claims in between.
    #[serde(skip)]
    next_completion: u64,
    /// The agents that have not finished. Every per-entry sweep — the wall
    /// charge, deadlines, expiry, liveness — walks this, not the records,
    /// which persist after exit and so grow without bound (#45).
    #[serde(skip)]
    live: BTreeSet<AgentId>,
    /// Each agent's children, for walking a subtree without filtering every
    /// record ever spawned. Never pruned: the record stays, so does the edge.
    #[serde(skip)]
    children: BTreeMap<AgentId, BTreeSet<AgentId>>,
}

/// Serializes the unclaimed completions as a sequence in completion order.
fn completions_in_order<S>(
    completed: &BTreeMap<u64, Completion>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.collect_seq(completed.values())
}

/// The canonical fields of a [`State`], as serialized: what a log determines
/// and nothing derived from it. Deserialization lands here and rebuilds the
/// indexes, so a cache added to `State` never reaches the wire.
#[derive(Deserialize)]
struct Canonical {
    next_seq: u64,
    next_agent: u64,
    next_corr: u64,
    next_cap: u64,
    next_hook: u64,
    now: u64,
    drivers: BTreeMap<DriverId, Capability>,
    ceilings: BTreeMap<DriverId, Budget>,
    caps: BTreeMap<Capability, Endpoint>,
    hooks: BTreeMap<HookId, HookRecord>,
    agents: BTreeMap<AgentId, Agent>,
    corrs: BTreeMap<Corr, AgentId>,
    completed: Vec<Completion>,
}

impl From<Canonical> for State {
    fn from(c: Canonical) -> Self {
        let live = c
            .agents
            .iter()
            .filter(|(_, a)| a.is_live())
            .map(|(id, _)| *id)
            .collect();
        let mut children: BTreeMap<AgentId, BTreeSet<AgentId>> = BTreeMap::new();
        for (id, a) in &c.agents {
            if let Some(parent) = a.parent {
                children.entry(parent).or_default().insert(*id);
            }
        }
        let completed: BTreeMap<u64, Completion> = (0..).zip(c.completed).collect();
        let unclaimed = completed
            .iter()
            .map(|(ordinal, completion)| (completion.agent, *ordinal))
            .collect();
        let next_completion = completed.len() as u64;
        Self {
            next_seq: c.next_seq,
            next_agent: c.next_agent,
            next_corr: c.next_corr,
            next_cap: c.next_cap,
            next_hook: c.next_hook,
            now: c.now,
            drivers: c.drivers,
            ceilings: c.ceilings,
            caps: c.caps,
            hooks: c.hooks,
            agents: c.agents,
            corrs: c.corrs,
            completed,
            unclaimed,
            next_completion,
            live,
            children,
        }
    }
}

/// A digest of a [`State`], for comparing folds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateHash([u8; 32]);

impl StateHash {
    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for StateHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for StateHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StateHash({self})")
    }
}

/// Why an entry cannot be applied to the current state.
///
/// From a live kernel this is a syscall error and the entry is never appended.
/// From a fold it means the log is not one this reducer could have produced.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Refusal {
    /// The entry's sequence number is not the next one.
    #[error("entry {found} is out of order; expected {expected}")]
    OutOfOrder {
        /// The position the log is at.
        expected: Seq,
        /// The position the entry claims.
        found: Seq,
    },
    /// An envelope was stamped with an ABI newer than this build.
    #[error("envelope abi {found} is newer than this build's {ABI}")]
    Envelope {
        /// The offending version.
        found: u16,
    },
    /// An entry carries an id other than the one the allocator would issue.
    #[error("{what} allocation mismatch: expected {expected}, found {found}")]
    BadAllocation {
        /// Which allocator.
        what: &'static str,
        /// The counter's value.
        expected: u64,
        /// The entry's value.
        found: u64,
    },
    /// A driver registered after the root was spawned.
    #[error("drivers register at boot, before the root agent exists")]
    AfterBoot,
    /// The driver is already registered.
    #[error("driver {0} is already registered")]
    DriverExists(DriverId),
    /// A second root.
    #[error("a root agent already exists")]
    RootExists,
    /// No such agent.
    #[error("unknown agent {0}")]
    UnknownAgent(AgentId),
    /// The agent has already exited or been aborted.
    #[error("agent {0} has exited")]
    AgentExited(AgentId),
    /// The agent is cancelled and in its grace period; it may not spawn, send,
    /// or cancel.
    #[error("agent {0} is cancelled and may not start new work")]
    Frozen(AgentId),
    /// A child omitted a dimension its parent is bounded on. Only `wall_ms`
    /// can be omitted at all — an untimed agent — and only under an untimed
    /// parent: a limit cannot be escaped by spawning.
    #[error("a child of {parent} may not be unbounded along `{dim}`")]
    Unbounded {
        /// The bounded parent.
        parent: AgentId,
        /// The dimension the child left out.
        dim: DimKey,
    },
    /// A child namespace that is not a subset of its parent's.
    #[error("namespace for a child of {parent} is not a subset of the parent's")]
    NotSubset {
        /// The parent.
        parent: AgentId,
    },
    /// A budget carve or restore failed.
    #[error("budget: {0}")]
    Budget(#[from] BudgetError),
    /// A capability the kernel never minted.
    #[error("unknown capability {0}")]
    UnknownCapability(Capability),
    /// The sender does not hold the capability it used.
    #[error("agent {agent} does not hold {cap}")]
    NotHeld {
        /// The sender.
        agent: AgentId,
        /// The capability.
        cap: Capability,
    },
    /// The capability names an endpoint the kernel cannot deliver to yet.
    #[error("capability {0} names an endpoint that is not routable")]
    Unroutable(Capability),
    /// The envelope's sender is not what the entry kind requires.
    #[error("unexpected sender {0}")]
    WrongSender(Endpoint),
    /// The envelope's kind is not what the entry kind requires.
    #[error("expected a {expected:?}, found a {found:?}")]
    WrongKind {
        /// What the entry requires.
        expected: MsgKind,
        /// What the envelope says.
        found: MsgKind,
    },
    /// The envelope has no correlation, or one the kernel does not know.
    #[error("unknown correlation {0:?}")]
    UnknownCorr(Option<Corr>),
    /// A reply addressed to someone other than the correlation's owner.
    #[error("correlation {corr} is owned by {expected}, not {found}")]
    WrongOwner {
        /// The correlation.
        corr: Corr,
        /// Its owner.
        expected: AgentId,
        /// Where the reply was addressed.
        found: AgentId,
    },
    /// A resolution names a message that is not in the mailbox.
    #[error("message {seq} is not in the mailbox of {agent}")]
    NotInMailbox {
        /// The receiver.
        agent: AgentId,
        /// The message.
        seq: Seq,
    },
    /// A claim for an agent with no unclaimed outcome.
    #[error("agent {0} has no unclaimed result")]
    NoResult(AgentId),
    /// An agent claimed, or waited for, an agent that is not its child.
    #[error("agent {child} is not a child of {agent}")]
    NotChild {
        /// The claimant.
        agent: AgentId,
        /// The agent it claimed.
        child: AgentId,
    },
    /// An agent cancelled an agent outside its own subtree.
    #[error("agent {target} is not a descendant of {agent}")]
    NotDescendant {
        /// The canceller.
        agent: AgentId,
        /// The agent it tried to cancel.
        target: AgentId,
    },
    /// The agent is already cancelled; a deadline cannot be renegotiated.
    #[error("agent {0} is already cancelled")]
    AlreadyCancelling(AgentId),
    /// A tick that reads earlier than the last one applied.
    #[error("clock rewound: last tick read {now}, this one {found}")]
    ClockRewound {
        /// The last reading.
        now: u64,
        /// The offending reading.
        found: u64,
    },
    /// A hook that fails open at a point where it could veto: "if my guard
    /// breaks, let everything through" is not a policy (ADR-0008 §4).
    #[error("a hook at {0} may not fail open")]
    OpenAtVetoPoint(HookPoint),
    /// A roll call or a notice names a hook that is not attached at that
    /// point.
    #[error("hook {hook} is not attached at {point}")]
    UnknownHook {
        /// The hook named.
        hook: HookId,
        /// The point the entry claims.
        point: HookPoint,
    },
    /// A roll call this kernel could not have written: out of `HookId`
    /// order, incomplete, a verdict after a `Deny`, or a `Deny` at a point
    /// that admits none.
    #[error("roll call at {point} is not one the kernel writes: {reason}")]
    BadRoll {
        /// The point.
        point: HookPoint,
        /// What is wrong with it.
        reason: &'static str,
    },
}

impl State {
    /// The state before any entry: nothing allocated, nothing known.
    #[must_use]
    pub fn initial() -> Self {
        Self::default()
    }

    /// The position the next entry must carry.
    #[must_use]
    pub fn next_seq(&self) -> Seq {
        Seq::new(self.next_seq)
    }

    /// The id the next spawn will be allocated.
    #[must_use]
    pub fn next_agent(&self) -> AgentId {
        AgentId::new(self.next_agent)
    }

    /// The correlation the next send will be allocated.
    #[must_use]
    pub fn next_corr(&self) -> Corr {
        Corr::new(self.next_corr)
    }

    /// The raw value the next minted capability will carry.
    #[must_use]
    pub fn next_cap(&self) -> u64 {
        self.next_cap
    }

    /// The id the next `attach` will be allocated.
    #[must_use]
    pub fn next_hook(&self) -> HookId {
        HookId::new(self.next_hook)
    }

    /// One installed hook's record.
    #[must_use]
    pub fn hook(&self, id: HookId) -> Option<&HookRecord> {
        self.hooks.get(&id)
    }

    /// Every installed hook, in id order — which is install order.
    pub fn hooks(&self) -> impl Iterator<Item = (HookId, &HookRecord)> + '_ {
        self.hooks.iter().map(|(id, h)| (*id, h))
    }

    /// The hooks attached at `point`, in id order: the roll call's order.
    pub fn hooks_at<'a>(&'a self, point: &'a HookPoint) -> impl Iterator<Item = HookId> + 'a {
        self.hooks
            .iter()
            .filter(move |(_, h)| h.point == *point)
            .map(|(id, _)| *id)
    }

    /// Every `OnBudget` point with at least one hook attached, each once,
    /// in point order.
    pub fn budget_points(&self) -> impl Iterator<Item = &HookPoint> + '_ {
        let mut seen: Option<&HookPoint> = None;
        self.hooks
            .values()
            .map(|h| &h.point)
            .filter(|p| matches!(p, HookPoint::OnBudget { .. }))
            .filter(move |p| {
                if seen == Some(*p) {
                    false
                } else {
                    seen = Some(*p);
                    true
                }
            })
    }

    /// The agents whose remaining grant `entry` may lower — the candidates
    /// for an `OnBudget` crossing (ADR-0008 §1): the parent a `Spawned`
    /// carves from, the sender a `Sent` reserves from, the owner a
    /// `Replied` settles, every live agent a `Tick` charges wall to. Every
    /// other entry only returns budget.
    #[must_use]
    pub fn budget_candidates(&self, entry: &Entry) -> Vec<AgentId> {
        match entry {
            Entry::Spawned {
                parent: Some(parent),
                ..
            } => vec![*parent],
            Entry::Sent { msg, .. } => match msg.from {
                Endpoint::Agent { id } => vec![id],
                _ => Vec::new(),
            },
            Entry::Replied { to, .. } => vec![*to],
            Entry::Tick { .. } => self.live.iter().copied().collect(),
            _ => Vec::new(),
        }
    }

    /// The remaining grant of each of `agents`, for a crossing check after
    /// the entry. See [`crate::hook::crossings`].
    #[must_use]
    pub fn remaining(&self, agents: &[AgentId]) -> Vec<(AgentId, Budget)> {
        agents
            .iter()
            .filter_map(|id| Some((*id, self.agents.get(id)?.budget.clone())))
            .collect()
    }

    /// The remaining grant of `agent`, whatever its status.
    #[must_use]
    pub fn budget_of(&self, agent: AgentId) -> Option<&Budget> {
        self.agents.get(&agent).map(|a| &a.budget)
    }

    /// The depth a child of `parent` — or the root, for `None` — would be
    /// born at with `requested`: what an `OnSpawn` event reports. Zero if
    /// the spawn would be refused.
    #[must_use]
    pub fn birth_depth(&self, parent: Option<AgentId>, requested: &Budget) -> u64 {
        match parent.and_then(|p| self.agents.get(&p)) {
            Some(p) => child_depth(p, requested).unwrap_or(0),
            None => requested.get(&DimKey::Depth).unwrap_or(0),
        }
    }

    /// A mark on the completion sequence: hand it back to
    /// [`completed_since`](Self::completed_since) after an entry to see who
    /// finished in it, in the order they finished.
    #[must_use]
    pub fn completion_mark(&self) -> u64 {
        self.next_completion
    }

    /// The agents that finished since `mark`, in completion order — which
    /// is deepest first when one entry ends several.
    pub fn completed_since(&self, mark: u64) -> impl Iterator<Item = &Completion> + '_ {
        self.completed.range(mark..).map(|(_, c)| c)
    }

    /// The reading of the last tick applied; zero before any.
    #[must_use]
    pub fn now(&self) -> u64 {
        self.now
    }

    /// How many entries have been applied.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.next_seq
    }

    /// Whether no entry has been applied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.next_seq == 0
    }

    /// One agent's record, whatever its status.
    #[must_use]
    pub fn agent(&self, id: AgentId) -> Option<&Agent> {
        self.agents.get(&id)
    }

    /// Every agent ever spawned, in id order.
    pub fn agents(&self) -> impl Iterator<Item = (AgentId, &Agent)> + '_ {
        self.agents.iter().map(|(id, a)| (*id, a))
    }

    /// The capability minted for `driver`, if registered.
    #[must_use]
    pub fn driver_cap(&self, driver: &DriverId) -> Option<Capability> {
        self.drivers.get(driver).copied()
    }

    /// The endpoint a capability names.
    #[must_use]
    pub fn resolve(&self, cap: Capability) -> Option<&Endpoint> {
        self.caps.get(&cap)
    }

    /// What one request to `driver` may cost at most, as registered.
    #[must_use]
    pub fn ceiling(&self, driver: &DriverId) -> Option<&Budget> {
        self.ceilings.get(driver)
    }

    /// The ceiling of the driver a capability names, if it names one.
    fn ceiling_via(&self, cap: Capability) -> Option<&Budget> {
        match self.caps.get(&cap)? {
            Endpoint::Driver { id } => self.ceilings.get(id),
            _ => None,
        }
    }

    /// Where `agent`'s unspent budget goes when it finishes: the nearest live
    /// ancestor, or the root's record if every ancestor is dead. `None` for
    /// the root itself, whose remainder stays where it is.
    fn heir(&self, agent: AgentId) -> Option<AgentId> {
        let mut cursor = self.agents.get(&agent)?.parent?;
        loop {
            let p = self.agents.get(&cursor)?;
            if p.is_live() {
                return Some(cursor);
            }
            match p.parent {
                Some(grandparent) => cursor = grandparent,
                None => return Some(cursor),
            }
        }
    }

    /// The agent that owns a correlation, while it is open.
    #[must_use]
    pub fn owner(&self, corr: Corr) -> Option<AgentId> {
        self.corrs.get(&corr).copied()
    }

    /// Every open correlation owned by `agent`, in id order.
    pub fn open_corrs(&self, agent: AgentId) -> impl Iterator<Item = Corr> + '_ {
        self.corrs
            .iter()
            .filter(move |(_, owner)| **owner == agent)
            .map(|(corr, _)| *corr)
    }

    /// An unclaimed outcome.
    #[must_use]
    pub fn result(&self, agent: AgentId) -> Option<Outcome> {
        self.completion(agent).map(|c| c.outcome)
    }

    /// The unclaimed completion of `agent`, if there is one.
    fn completion(&self, agent: AgentId) -> Option<&Completion> {
        self.completed.get(self.unclaimed.get(&agent)?)
    }

    /// Every unclaimed outcome, in completion order.
    pub fn completed(&self) -> impl ExactSizeIterator<Item = &Completion> + DoubleEndedIterator {
        self.completed.values()
    }

    /// The earliest-finished child of `parent` whose outcome is unclaimed.
    /// This is what `wait(Any)` returns.
    #[must_use]
    pub fn next_completed_child(&self, parent: AgentId) -> Option<&Completion> {
        self.completed.values().find(|c| c.parent == Some(parent))
    }

    /// Whether `parent` has a child that has not finished.
    #[must_use]
    pub fn has_live_children(&self, parent: AgentId) -> bool {
        self.children
            .get(&parent)
            .is_some_and(|kids| kids.iter().any(|kid| self.live.contains(kid)))
    }

    /// Whether `ancestor` is a strict ancestor of `agent`.
    #[must_use]
    pub fn is_descendant(&self, agent: AgentId, ancestor: AgentId) -> bool {
        let mut cursor = self.agents.get(&agent).and_then(|a| a.parent);
        while let Some(id) = cursor {
            if id == ancestor {
                return true;
            }
            cursor = self.agents.get(&id).and_then(|a| a.parent);
        }
        false
    }

    /// `root` and every agent below it, whatever their status, in id order.
    #[must_use]
    pub fn subtree(&self, root: AgentId) -> Vec<AgentId> {
        if !self.agents.contains_key(&root) {
            return Vec::new();
        }
        let mut found = vec![root];
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if let Some(kids) = self.children.get(&id) {
                found.extend(kids.iter().copied());
                pending.extend(kids.iter().copied());
            }
        }
        found.sort_unstable();
        found
    }

    /// The agents a tick reading `now` would end, deepest first: cancelled
    /// ones whose deadline it reaches, and timed ones whose wall grant it
    /// exhausts.
    ///
    /// Deepest first because ids are allocated in spawn order, so a child's id
    /// is always greater than its parent's: aborting in descending id order
    /// returns a child's budget to its parent *before* the parent's is returned
    /// to the grandparent.
    #[must_use]
    pub fn expiring(&self, now: u64) -> Vec<AgentId> {
        let elapsed = now.saturating_sub(self.now);
        self.live_agents()
            .rev()
            .filter(|(_, a)| {
                (a.is_frozen() && a.deadline.is_some_and(|d| d <= now))
                    || a.budget.get(&DimKey::WallMs).is_some_and(|w| w <= elapsed)
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// Every agent that has not finished, with its record, in id order.
    fn live_agents(&self) -> impl DoubleEndedIterator<Item = (AgentId, &Agent)> + '_ {
        self.live
            .iter()
            .filter_map(|id| Some((*id, self.agents.get(id)?)))
    }

    /// The cancelled agents whose deadline the current reading has reached,
    /// deepest first. What a grace of zero aborts at the cancel itself.
    fn deadline_reached(&self) -> Vec<AgentId> {
        self.live_agents()
            .rev()
            .filter(|(_, a)| a.is_frozen() && a.deadline.is_some_and(|d| d <= self.now))
            .map(|(id, _)| id)
            .collect()
    }

    /// How many agents have not finished.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// Whether a root was spawned and every agent has since finished.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        !self.agents.is_empty() && self.live.is_empty()
    }

    /// A digest of this state.
    ///
    /// Two folds of the same log must produce the same hash on every platform;
    /// that is the property the determinism jobs test. Serialization is
    /// order-stable because every collection here is a B-tree or a Vec.
    #[must_use]
    pub fn hash(&self) -> StateHash {
        // Serializing a struct of B-trees, integers, and validated strings
        // cannot fail; the fallback keeps the kernel free of `unwrap`.
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        StateHash(sha256(&bytes))
    }

    fn live(&self, id: AgentId) -> Result<&Agent, Refusal> {
        let agent = self.agents.get(&id).ok_or(Refusal::UnknownAgent(id))?;
        if !agent.is_live() {
            return Err(Refusal::AgentExited(id));
        }
        Ok(agent)
    }

    /// Live and not frozen: allowed to start new work.
    fn active(&self, id: AgentId) -> Result<&Agent, Refusal> {
        let agent = self.live(id)?;
        if agent.is_frozen() {
            return Err(Refusal::Frozen(id));
        }
        Ok(agent)
    }

    fn check_envelope(&self, msg: &Msg) -> Result<(), Refusal> {
        if msg.abi > ABI {
            return Err(Refusal::Envelope { found: msg.abi });
        }
        Ok(())
    }

    /// Whether `entry` could be applied now. Pure; changes nothing.
    ///
    /// # Errors
    ///
    /// The first [`Refusal`] the entry would trigger.
    pub fn check(&self, entry: &Entry) -> Result<(), Refusal> {
        let found = entry.seq();
        if found.get() != self.next_seq {
            return Err(Refusal::OutOfOrder {
                expected: self.next_seq(),
                found,
            });
        }
        match entry {
            Entry::DriverRegistered { driver, cap, .. } => {
                if !self.agents.is_empty() {
                    return Err(Refusal::AfterBoot);
                }
                if self.drivers.contains_key(driver) {
                    return Err(Refusal::DriverExists(driver.clone()));
                }
                if cap.get() != self.next_cap {
                    return Err(Refusal::BadAllocation {
                        what: "capability",
                        expected: self.next_cap,
                        found: cap.get(),
                    });
                }
            }
            Entry::Spawned {
                parent,
                agent,
                ns,
                budget,
                ..
            } => {
                if agent.get() != self.next_agent {
                    return Err(Refusal::BadAllocation {
                        what: "agent",
                        expected: self.next_agent,
                        found: agent.get(),
                    });
                }
                match parent {
                    None => {
                        if !self.agents.is_empty() {
                            return Err(Refusal::RootExists);
                        }
                        if let Some(cap) = ns.iter().find(|c| !self.caps.contains_key(c)) {
                            return Err(Refusal::UnknownCapability(cap));
                        }
                    }
                    Some(parent) => {
                        let p = self.active(*parent)?;
                        if !ns.is_subset_of(&p.ns) {
                            return Err(Refusal::NotSubset { parent: *parent });
                        }
                        child_depth(p, budget)?;
                        if p.is_timed() && budget.get(&DimKey::WallMs).is_none() {
                            return Err(Refusal::Unbounded {
                                parent: *parent,
                                dim: DimKey::WallMs,
                            });
                        }
                        p.budget.clone().carve(&without_depth(budget))?;
                    }
                }
            }
            Entry::Sent { msg, via } => {
                self.check_envelope(msg)?;
                let Endpoint::Agent { id } = msg.from else {
                    return Err(Refusal::WrongSender(msg.from.clone()));
                };
                let sender = self.active(id)?;
                if msg.kind != MsgKind::Request {
                    return Err(Refusal::WrongKind {
                        expected: MsgKind::Request,
                        found: msg.kind,
                    });
                }
                match msg.corr {
                    Some(corr) if corr.get() == self.next_corr => {}
                    Some(corr) => {
                        return Err(Refusal::BadAllocation {
                            what: "correlation",
                            expected: self.next_corr,
                            found: corr.get(),
                        })
                    }
                    None => return Err(Refusal::UnknownCorr(None)),
                }
                if !sender.ns.holds(*via) {
                    return Err(Refusal::NotHeld {
                        agent: id,
                        cap: *via,
                    });
                }
                let driver = match self.caps.get(via) {
                    Some(Endpoint::Driver { id }) => id,
                    Some(_) => return Err(Refusal::Unroutable(*via)),
                    None => return Err(Refusal::UnknownCapability(*via)),
                };
                // Reservation before the call: the ceiling plus one `calls`,
                // or the send does not happen.
                let ceiling = self.ceilings.get(driver).ok_or(Refusal::Unroutable(*via))?;
                sender.budget.clone().carve(&ask_for(ceiling))?;
            }
            Entry::Replied { msg, to } => {
                self.check_envelope(msg)?;
                let Endpoint::Driver { id } = &msg.from else {
                    return Err(Refusal::WrongSender(msg.from.clone()));
                };
                if !self.drivers.contains_key(id) {
                    return Err(Refusal::WrongSender(msg.from.clone()));
                }
                if msg.kind != MsgKind::Reply {
                    return Err(Refusal::WrongKind {
                        expected: MsgKind::Reply,
                        found: msg.kind,
                    });
                }
                let corr = msg.corr.ok_or(Refusal::UnknownCorr(None))?;
                let owner = self.owner(corr).ok_or(Refusal::UnknownCorr(Some(corr)))?;
                if owner != *to {
                    return Err(Refusal::WrongOwner {
                        corr,
                        expected: owner,
                        found: *to,
                    });
                }
                self.live(*to)?;
            }
            Entry::Resolved { agent, matched, .. } => {
                let a = self.live(*agent)?;
                if !a.mailbox.iter().any(|m| m.seq == *matched) {
                    return Err(Refusal::NotInMailbox {
                        agent: *agent,
                        seq: *matched,
                    });
                }
            }
            Entry::Exited { agent, .. } => {
                let a = self.live(*agent)?;
                let mut unspent = a.budget.clone();
                for held in a.reserved.values() {
                    unspent.restore(held)?;
                }
                if let Some(heir) = self.heir(*agent) {
                    let h = self.agents.get(&heir).ok_or(Refusal::UnknownAgent(heir))?;
                    h.budget.clone().restore(&without_depth(&unspent))?;
                }
            }
            Entry::Claimed { agent, by, .. } => {
                let completion = self.completion(*agent).ok_or(Refusal::NoResult(*agent))?;
                if let Some(by) = by {
                    self.live(*by)?;
                    if completion.parent != Some(*by) {
                        return Err(Refusal::NotChild {
                            agent: *by,
                            child: *agent,
                        });
                    }
                }
            }
            Entry::Cancelled { by, agent, .. } => {
                // Who is asking, before what they ask for.
                if let Some(by) = by {
                    self.active(*by)?;
                    if !self.is_descendant(*agent, *by) {
                        return Err(Refusal::NotDescendant {
                            agent: *by,
                            target: *agent,
                        });
                    }
                }
                let target = self.live(*agent)?;
                if target.is_frozen() {
                    return Err(Refusal::AlreadyCancelling(*agent));
                }
            }
            Entry::Tick { now, .. } => {
                if *now < self.now {
                    return Err(Refusal::ClockRewound {
                        now: self.now,
                        found: *now,
                    });
                }
            }
            Entry::Attached {
                hook,
                point,
                failure,
                ..
            } => {
                if !self.agents.is_empty() {
                    return Err(Refusal::AfterBoot);
                }
                if hook.get() != self.next_hook {
                    return Err(Refusal::BadAllocation {
                        what: "hook",
                        expected: self.next_hook,
                        found: hook.get(),
                    });
                }
                if *failure == FailureMode::Open && point.admits_deny() {
                    return Err(Refusal::OpenAtVetoPoint(point.clone()));
                }
            }
            Entry::Verdicts {
                point,
                subject,
                roll,
                ..
            } => {
                self.check_roll(point, roll)?;
                // The subject of a pre-spawn moment does not exist yet: it is
                // the id the spawn will be allocated. Every other subject is
                // an agent the log has seen, live or not.
                match point {
                    HookPoint::OnSpawn if subject.get() != self.next_agent => {
                        return Err(Refusal::BadAllocation {
                            what: "agent",
                            expected: self.next_agent,
                            found: subject.get(),
                        });
                    }
                    HookPoint::OnSpawn => {}
                    _ if !self.agents.contains_key(subject) => {
                        return Err(Refusal::UnknownAgent(*subject));
                    }
                    _ => {}
                }
            }
            Entry::Emitted { hook, msg, .. } => {
                self.check_envelope(msg)?;
                let Endpoint::Hook { id } = msg.from else {
                    return Err(Refusal::WrongSender(msg.from.clone()));
                };
                let record = self.hooks.get(&id).ok_or(Refusal::UnknownHook {
                    hook: id,
                    point: HookPoint::PreSend,
                })?;
                if id != *hook {
                    return Err(Refusal::UnknownHook {
                        hook: *hook,
                        point: record.point.clone(),
                    });
                }
                if msg.kind != MsgKind::Notice {
                    return Err(Refusal::WrongKind {
                        expected: MsgKind::Notice,
                        found: msg.kind,
                    });
                }
                if let Some(corr) = msg.corr {
                    return Err(Refusal::UnknownCorr(Some(corr)));
                }
            }
        }
        Ok(())
    }

    /// Confirms a roll call: the hooks at `point`, in id order, complete —
    /// every one attached there answered — unless one stopped it, in which
    /// case it is the prefix ending there. A `Deny` where none is admitted
    /// is a roll this kernel never writes.
    fn check_roll(&self, point: &HookPoint, roll: &Roll) -> Result<(), Refusal> {
        let bad = |reason| Refusal::BadRoll {
            point: point.clone(),
            reason,
        };
        if roll.is_empty() {
            return Err(bad("empty: a point with no hook writes nothing"));
        }
        let mut expected = self.hooks_at(point);
        let mut stopped = false;
        for (hook, ruling) in roll {
            if stopped {
                return Err(bad("a verdict after the one that stopped it"));
            }
            match expected.next() {
                Some(id) if id == *hook => {}
                _ if !self.hooks.get(hook).is_some_and(|h| h.point == *point) => {
                    return Err(Refusal::UnknownHook {
                        hook: *hook,
                        point: point.clone(),
                    })
                }
                _ => return Err(bad("out of install order, or a hook skipped")),
            }
            if matches!(ruling, Ruling::Deny(_)) && !point.admits_deny() {
                return Err(bad("a deny at a point that admits none"));
            }
            stopped = ruling.stops(point);
        }
        if !stopped && expected.next().is_some() {
            return Err(bad("incomplete: a hook attached here was not asked"));
        }
        Ok(())
    }

    /// Applies `entry`. Checks first; on refusal the state is unchanged.
    ///
    /// # Errors
    ///
    /// Whatever [`State::check`] would return.
    pub fn apply(&mut self, entry: &Entry) -> Result<(), Refusal> {
        self.check(entry)?;
        match entry {
            Entry::DriverRegistered {
                driver,
                cap,
                ceiling,
                ..
            } => {
                self.drivers.insert(driver.clone(), *cap);
                self.ceilings.insert(driver.clone(), ceiling.clone());
                self.caps
                    .insert(*cap, Endpoint::Driver { id: driver.clone() });
                self.next_cap = self.next_cap.saturating_add(1);
            }
            Entry::Spawned {
                parent,
                agent,
                ns,
                budget,
                ..
            } => {
                let granted = match parent {
                    None => budget.clone(),
                    Some(parent) => {
                        let p = self
                            .agents
                            .get_mut(parent)
                            .ok_or(Refusal::UnknownAgent(*parent))?;
                        // Depth is derived, not carved: the parent keeps its
                        // own level; the child is born one shallower.
                        let depth = child_depth(p, budget)?;
                        let mut granted = p.budget.carve(&without_depth(budget))?;
                        granted.restore(&single(DimKey::Depth, depth))?;
                        granted
                    }
                };
                self.agents.insert(
                    *agent,
                    Agent {
                        parent: *parent,
                        ns: ns.clone(),
                        budget: granted,
                        reserved: BTreeMap::new(),
                        spent: BTreeMap::new(),
                        overdraft: BTreeMap::new(),
                        status: Status::Live,
                        mailbox: Vec::new(),
                        deadline: None,
                    },
                );
                self.live.insert(*agent);
                if let Some(parent) = parent {
                    self.children.entry(*parent).or_default().insert(*agent);
                }
                self.next_agent = self.next_agent.saturating_add(1);
            }
            Entry::Sent { msg, via } => {
                if let (Endpoint::Agent { id }, Some(corr)) = (&msg.from, msg.corr) {
                    self.corrs.insert(corr, *id);
                    let ceiling = self.ceiling_via(*via).cloned().unwrap_or_default();
                    let a = self.agents.get_mut(id).ok_or(Refusal::UnknownAgent(*id))?;
                    a.budget.carve(&ask_for(&ceiling))?;
                    add(&mut a.spent, &DimKey::Calls, 1);
                    a.reserved.insert(corr, ceiling);
                }
                self.next_corr = self.next_corr.saturating_add(1);
            }
            Entry::Replied { msg, to } => {
                let a = self.agents.get_mut(to).ok_or(Refusal::UnknownAgent(*to))?;
                let reservation = msg
                    .corr
                    .and_then(|corr| a.reserved.remove(&corr))
                    .unwrap_or_default();
                settle(a, &reservation, msg.consumed.as_ref())?;
                a.mailbox.push(msg.clone());
                if let Some(corr) = msg.corr {
                    self.corrs.remove(&corr);
                }
            }
            Entry::Resolved { agent, matched, .. } => {
                let a = self
                    .agents
                    .get_mut(agent)
                    .ok_or(Refusal::UnknownAgent(*agent))?;
                a.mailbox.retain(|m| m.seq != *matched);
            }
            Entry::Exited { agent, result, .. } => {
                self.finish(*agent, Outcome::Exited(*result))?;
            }
            Entry::Claimed { agent, .. } => {
                if let Some(ordinal) = self.unclaimed.remove(agent) {
                    self.completed.remove(&ordinal);
                }
            }
            Entry::Cancelled {
                seq,
                by,
                agent,
                grace,
                reason,
            } => {
                // Phase one: the atomic freeze. Every live agent in the subtree
                // is frozen by this one entry, gets the notice, and gets the
                // deadline — brought earlier if an inner cancel already set
                // one, never pushed later.
                let deadline = self.now.saturating_add(*grace);
                let from = by.map_or(Endpoint::Harness, |id| Endpoint::Agent { id });
                let notice = Msg::new(*seq, from, MsgKind::Notice, *reason);
                for id in self.subtree(*agent) {
                    let Some(a) = self.agents.get_mut(&id) else {
                        continue;
                    };
                    if !a.is_live() {
                        continue;
                    }
                    a.status = Status::Cancelling;
                    a.deadline = Some(a.deadline.map_or(deadline, |d| d.min(deadline)));
                    a.mailbox.push(notice.clone());
                }
                // A grace of zero is a deadline already reached.
                for id in self.deadline_reached() {
                    self.finish(id, Outcome::Aborted)?;
                }
            }
            Entry::Tick { now, .. } => {
                // The clock spends wall time on every live agent's behalf;
                // then whoever it emptied, and whoever's deadline it reached,
                // is ended inside this same apply.
                let elapsed = now.saturating_sub(self.now);
                self.now = *now;
                for id in &self.live {
                    if let Some(a) = self.agents.get_mut(id) {
                        charge_wall(a, elapsed)?;
                    }
                }
                for id in self.expiring(*now) {
                    self.finish(id, Outcome::Aborted)?;
                }
            }
            Entry::Attached {
                hook,
                point,
                failure,
                program,
                ..
            } => {
                self.hooks.insert(
                    *hook,
                    HookRecord {
                        point: point.clone(),
                        failure: *failure,
                        program: program.clone(),
                    },
                );
                self.next_hook = self.next_hook.saturating_add(1);
            }
            // Confirmed by the check; applied as nothing. The effect of a
            // `Deny` is the absence of the next entry, and the effect of an
            // `Emit` is the `Emitted` entry that follows.
            Entry::Verdicts { .. } => {}
            Entry::Emitted { to, msg, .. } => {
                // Delivered if the recipient is live; dead letter otherwise,
                // exactly as a reply to an exited owner.
                if let Some(a) = self.agents.get_mut(to) {
                    if a.is_live() {
                        a.mailbox.push(msg.clone());
                    }
                }
            }
        }
        self.next_seq = self.next_seq.saturating_add(1);
        Ok(())
    }

    /// Ends an agent, by exit or by abort: the record stays, the mailbox and
    /// open correlations go (HANDOFF §4.9), reservations for those requests
    /// are released, unspent budget returns up the tree, and the outcome is
    /// stored until claimed.
    ///
    /// Unspent budget goes to the nearest *live* ancestor, or to the root's
    /// record if there is none — the root's record is where the harness's
    /// grant is accounted for. Never to a dead non-root record, where nobody
    /// could ever spend or return it: the outcome is the same as if the
    /// family had exited youngest-first, whatever order it actually did.
    ///
    /// The restores here cannot overflow, so a tick or a cancel that ends
    /// several agents in one apply cannot fail half-way: along every
    /// dimension but `depth`, budgets plus reservations plus spent sum to
    /// the root's grant (overdraft lives in `spent` only), and each restore
    /// moves part of that sum into another part of it; `depth` never moves.
    fn finish(&mut self, id: AgentId, outcome: Outcome) -> Result<(), Refusal> {
        let heir = self.heir(id);
        let a = self.agents.get_mut(&id).ok_or(Refusal::UnknownAgent(id))?;
        a.status = match outcome {
            Outcome::Exited(_) => Status::Exited,
            Outcome::Aborted => Status::Aborted,
        };
        a.deadline = None;
        a.mailbox.clear();
        let released = core::mem::take(&mut a.reserved);
        for held in released.values() {
            a.budget.restore(held)?;
        }
        let parent = a.parent;
        // `depth` was never carved from the heir, so it is not handed back:
        // a shape limit does not grow because a child came and went.
        let unspent = match heir {
            Some(_) => without_depth(&core::mem::replace(&mut a.budget, Budget::empty())),
            None => Budget::empty(),
        };
        // Its open requests are exactly the reservations it held.
        for corr in released.keys() {
            self.corrs.remove(corr);
        }
        self.live.remove(&id);
        if let Some(heir) = heir {
            let h = self
                .agents
                .get_mut(&heir)
                .ok_or(Refusal::UnknownAgent(heir))?;
            h.budget.restore(&unspent)?;
        }
        let ordinal = self.next_completion;
        self.next_completion = self.next_completion.saturating_add(1);
        self.completed.insert(
            ordinal,
            Completion {
                agent: id,
                parent,
                outcome,
            },
        );
        self.unclaimed.insert(id, ordinal);
        Ok(())
    }
}

/// The depth a child of `parent` is born with: one less than the parent's, or
/// what the request asks for if that is smaller. A parent without a depth
/// grant, or at zero, cannot spawn at all.
fn child_depth(parent: &Agent, requested: &Budget) -> Result<u64, BudgetError> {
    let available = parent
        .budget
        .get(&DimKey::Depth)
        .ok_or(BudgetError::NoGrant { dim: DimKey::Depth })?
        .checked_sub(1)
        .ok_or(BudgetError::Insufficient {
            dim: DimKey::Depth,
            available: 0,
            requested: 1,
        })?;
    match requested.get(&DimKey::Depth) {
        None => Ok(available),
        Some(asked) if asked <= available => Ok(asked),
        Some(asked) => Err(BudgetError::Insufficient {
            dim: DimKey::Depth,
            available,
            requested: asked,
        }),
    }
}

/// Adds to a per-dimension tally. Nothing is recorded for zero: an absent
/// key and a zero are the same fact, and only one of them may appear in the
/// state hash.
fn add(map: &mut BTreeMap<DimKey, u64>, dim: &DimKey, amount: u64) {
    if amount == 0 {
        return;
    }
    let slot = map.entry(dim.clone()).or_insert(0);
    *slot = slot.saturating_add(amount);
}

/// One dimension as a budget, for carving and restoring a single amount.
fn single(dim: DimKey, amount: u64) -> Budget {
    Budget::from_dims([(dim, amount)])
}

/// What a `Sent` must be able to pay for: the driver's ceiling plus the one
/// `calls` the reducer charges for the send itself.
fn ask_for(ceiling: &Budget) -> Budget {
    let calls = ceiling.get(&DimKey::Calls).unwrap_or(0).saturating_add(1);
    Budget::from_dims(
        ceiling
            .iter()
            .filter(|(dim, _)| **dim != DimKey::Calls)
            .map(|(dim, amount)| (dim.clone(), amount))
            .chain([(DimKey::Calls, calls)]),
    )
}

/// The request minus `depth`, which is derived rather than carved.
fn without_depth(requested: &Budget) -> Budget {
    Budget::from_dims(
        requested
            .iter()
            .filter(|(dim, _)| **dim != DimKey::Depth)
            .map(|(dim, amount)| (dim.clone(), amount)),
    )
}

/// Settles a reservation against what the driver reported.
///
/// Every reserved dimension refunds what was not used and records what was.
/// Anything reported above the reservation — or along a dimension that was
/// never reserved — is charged to `spent` in full and mirrored in `overdraft`,
/// and is *not* taken from the budget the agent still holds.
fn settle(
    agent: &mut Agent,
    reservation: &Budget,
    consumed: Option<&Consumption>,
) -> Result<(), BudgetError> {
    let mut refund = Budget::empty();
    for (dim, held) in reservation.iter() {
        let used = consumed.and_then(|c| c.get(dim)).unwrap_or(0);
        refund.restore(&single(dim.clone(), held.saturating_sub(used)))?;
        add(&mut agent.spent, dim, used);
        add(&mut agent.overdraft, dim, used.saturating_sub(held));
    }
    if let Some(consumed) = consumed {
        for (dim, used) in consumed.iter() {
            if reservation.get(dim).is_none() {
                add(&mut agent.spent, dim, used);
                add(&mut agent.overdraft, dim, used);
            }
        }
    }
    agent.budget.restore(&refund)
}

/// Charges `elapsed` clock units of wall time to a live agent.
///
/// A timed agent pays out of its `wall_ms` grant, down to zero and no
/// further: the charge is the min of the two, so budget and spent always sum
/// to what was granted. An untimed agent is charged nothing, and the elapsed
/// time is recorded on its receipt anyway, so a run without a limit still
/// reports how long it took.
fn charge_wall(agent: &mut Agent, elapsed: u64) -> Result<(), BudgetError> {
    let charged = match agent.budget.get(&DimKey::WallMs) {
        Some(have) => {
            let moved = elapsed.min(have);
            agent.budget.carve(&single(DimKey::WallMs, moved))?;
            moved
        }
        None => elapsed,
    };
    add(&mut agent.spent, &DimKey::WallMs, charged);
    Ok(())
}

/// Folds entries from the initial state.
///
/// # Errors
///
/// The first [`Refusal`], which means the log is not one this reducer could
/// have produced.
pub fn fold<'a, I>(entries: I) -> Result<State, Refusal>
where
    I: IntoIterator<Item = &'a Entry>,
{
    let mut state = State::initial();
    for entry in entries {
        state.apply(entry)?;
    }
    Ok(state)
}
