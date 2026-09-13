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

use core::fmt;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::abi::{
    AgentId, BlobRef, Budget, BudgetError, Capability, Consumption, Corr, DimKey, DriverId,
    Endpoint, Msg, MsgKind, Namespace, Seq, ABI,
};
use crate::blob::sha256;
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
    /// What remains of the grant.
    pub budget: Budget,
    /// Everything drivers have reported against this agent, every dimension.
    /// Recorded in full; only `tokens` is *enforced* so far.
    pub spent: BTreeMap<DimKey, u64>,
    /// Whether the `tokens` grant is gone. An exhausted agent cannot send.
    pub exhausted: bool,
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
}

/// The kernel's state: a pure fold over the log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    next_seq: u64,
    next_agent: u64,
    next_corr: u64,
    next_cap: u64,
    now: u64,
    drivers: BTreeMap<DriverId, Capability>,
    caps: BTreeMap<Capability, Endpoint>,
    agents: BTreeMap<AgentId, Agent>,
    corrs: BTreeMap<Corr, AgentId>,
    /// Finished agents whose outcome is unclaimed, in completion order. A
    /// `Vec`, not a map: `wait(Any)` returns in completion order.
    completed: Vec<Completion>,
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
    /// The agent's `tokens` grant is spent.
    #[error("agent {0} has exhausted its tokens")]
    Exhausted(AgentId),
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
        self.completed
            .iter()
            .find(|c| c.agent == agent)
            .map(|c| c.outcome)
    }

    /// Every unclaimed outcome, in completion order.
    #[must_use]
    pub fn completed(&self) -> &[Completion] {
        &self.completed
    }

    /// The earliest-finished child of `parent` whose outcome is unclaimed.
    /// This is what `wait(Any)` returns.
    #[must_use]
    pub fn next_completed_child(&self, parent: AgentId) -> Option<&Completion> {
        self.completed.iter().find(|c| c.parent == Some(parent))
    }

    /// Whether `parent` has a child that has not finished.
    #[must_use]
    pub fn has_live_children(&self, parent: AgentId) -> bool {
        self.agents
            .values()
            .any(|a| a.parent == Some(parent) && a.is_live())
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
        self.agents
            .keys()
            .copied()
            .filter(|id| *id == root || self.is_descendant(*id, root))
            .collect()
    }

    /// The cancelled agents a tick reading `now` would abort, deepest first.
    ///
    /// Deepest first because ids are allocated in spawn order, so a child's id
    /// is always greater than its parent's: aborting in descending id order
    /// returns a child's budget to its parent *before* the parent's is returned
    /// to the grandparent, and nothing is stranded on a dead record.
    #[must_use]
    pub fn expiring(&self, now: u64) -> Vec<AgentId> {
        self.agents
            .iter()
            .rev()
            .filter(|(_, a)| a.is_frozen() && a.deadline.is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect()
    }

    /// How many agents have not finished.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.agents.values().filter(|a| a.is_live()).count()
    }

    /// Whether a root was spawned and every agent has since finished.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        !self.agents.is_empty() && self.live_count() == 0
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
                        p.budget.clone().carve(budget)?;
                    }
                }
            }
            Entry::Sent { msg, via } => {
                self.check_envelope(msg)?;
                let Endpoint::Agent { id } = msg.from else {
                    return Err(Refusal::WrongSender(msg.from.clone()));
                };
                let sender = self.active(id)?;
                if sender.exhausted {
                    return Err(Refusal::Exhausted(id));
                }
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
                match self.caps.get(via) {
                    Some(Endpoint::Driver { .. }) => {}
                    Some(_) => return Err(Refusal::Unroutable(*via)),
                    None => return Err(Refusal::UnknownCapability(*via)),
                }
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
                if let Some(parent) = a.parent {
                    let p = self
                        .agents
                        .get(&parent)
                        .ok_or(Refusal::UnknownAgent(parent))?;
                    p.budget.clone().restore(&a.budget)?;
                }
            }
            Entry::Claimed { agent, by, .. } => {
                let completion = self
                    .completed
                    .iter()
                    .find(|c| c.agent == *agent)
                    .ok_or(Refusal::NoResult(*agent))?;
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
            Entry::DriverRegistered { driver, cap, .. } => {
                self.drivers.insert(driver.clone(), *cap);
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
                        p.budget.carve(budget)?
                    }
                };
                self.agents.insert(
                    *agent,
                    Agent {
                        parent: *parent,
                        ns: ns.clone(),
                        exhausted: granted.get(&DimKey::Tokens).is_none_or(|t| t == 0),
                        budget: granted,
                        spent: BTreeMap::new(),
                        status: Status::Live,
                        mailbox: Vec::new(),
                        deadline: None,
                    },
                );
                self.next_agent = self.next_agent.saturating_add(1);
            }
            Entry::Sent { msg, .. } => {
                if let (Endpoint::Agent { id }, Some(corr)) = (&msg.from, msg.corr) {
                    self.corrs.insert(corr, *id);
                }
                self.next_corr = self.next_corr.saturating_add(1);
            }
            Entry::Replied { msg, to } => {
                if let Some(corr) = msg.corr {
                    self.corrs.remove(&corr);
                }
                let a = self.agents.get_mut(to).ok_or(Refusal::UnknownAgent(*to))?;
                if let Some(consumed) = &msg.consumed {
                    charge(a, consumed);
                }
                a.mailbox.push(msg.clone());
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
                if let Some(idx) = self.completed.iter().position(|c| c.agent == *agent) {
                    self.completed.remove(idx);
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
                self.expire()?;
            }
            Entry::Tick { now, .. } => {
                self.now = *now;
                self.expire()?;
            }
        }
        self.next_seq = self.next_seq.saturating_add(1);
        Ok(())
    }

    /// Hard-aborts every cancelled agent whose deadline `now` has reached.
    fn expire(&mut self) -> Result<(), Refusal> {
        for id in self.expiring(self.now) {
            self.finish(id, Outcome::Aborted)?;
        }
        Ok(())
    }

    /// Ends an agent, by exit or by abort: the record stays, the mailbox and
    /// open correlations go (HANDOFF §4.9), unspent budget returns to the
    /// parent, and the outcome is stored until claimed.
    ///
    /// The root has no parent; its remainder stays on its record, which is
    /// where the harness's grant is accounted for. A parent's record persists
    /// past its own end for exactly this reason: a child may outlive it.
    fn finish(&mut self, id: AgentId, outcome: Outcome) -> Result<(), Refusal> {
        let a = self.agents.get_mut(&id).ok_or(Refusal::UnknownAgent(id))?;
        a.status = match outcome {
            Outcome::Exited(_) => Status::Exited,
            Outcome::Aborted => Status::Aborted,
        };
        a.deadline = None;
        a.mailbox.clear();
        let parent = a.parent;
        let unspent = match parent {
            Some(_) => core::mem::replace(&mut a.budget, Budget::empty()),
            None => Budget::empty(),
        };
        self.corrs.retain(|_, owner| *owner != id);
        if let Some(parent) = parent {
            let p = self
                .agents
                .get_mut(&parent)
                .ok_or(Refusal::UnknownAgent(parent))?;
            p.budget.restore(&unspent)?;
        }
        self.completed.push(Completion {
            agent: id,
            parent,
            outcome,
        });
        Ok(())
    }
}

/// Records a driver's report against an agent and enforces `tokens`.
///
/// Enforcement is deliberately coarse: a report that overdraws the grant
/// drains it to zero and marks the agent exhausted, so its *next* send is
/// refused. Reservation before the call — refusing a send that could not be
/// paid for — is M1b, with the rest of the budget dimensions.
fn charge(agent: &mut Agent, consumed: &Consumption) {
    for (dim, amount) in consumed.iter() {
        let slot = agent.spent.entry(dim.clone()).or_insert(0);
        *slot = slot.saturating_add(amount);
    }
    let Some(tokens) = consumed.get(&DimKey::Tokens) else {
        return;
    };
    let ask = Budget::from_dims([(DimKey::Tokens, tokens)]);
    if agent.budget.carve(&ask).is_err() {
        let remaining = agent.budget.get(&DimKey::Tokens).unwrap_or(0);
        // Cannot fail: the amount is exactly what is there.
        let _ = agent
            .budget
            .carve(&Budget::from_dims([(DimKey::Tokens, remaining)]));
        agent.exhausted = true;
    }
    if agent.budget.get(&DimKey::Tokens).is_none_or(|t| t == 0) {
        agent.exhausted = true;
    }
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
