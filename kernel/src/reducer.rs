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

use core::fmt;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::abi::{
    AgentId, BlobRef, Budget, BudgetError, Capability, Consumption, Corr, DimKey, DriverId,
    Endpoint, Msg, MsgKind, Namespace, Seq, ABI,
};
use crate::blob::sha256;
use crate::log::Entry;

/// Whether an agent is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The agent has not exited.
    Live,
    /// The agent has exited; its record persists for accounting.
    Exited,
}

/// Everything the kernel knows about one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// The spawning agent; `None` for the root.
    pub parent: Option<AgentId>,
    /// The birth namespace. Authority is this set — M0 has no transfer.
    pub ns: Namespace,
    /// What remains of the grant.
    pub budget: Budget,
    /// Everything drivers have reported against this agent, every dimension.
    /// Recorded in full; only `tokens` is *enforced* in M0.
    pub spent: BTreeMap<DimKey, u64>,
    /// Whether the `tokens` grant is gone. An exhausted agent cannot send.
    pub exhausted: bool,
    /// Live or exited.
    pub status: Status,
    /// Delivered, unresolved messages, in delivery order.
    pub mailbox: Vec<Msg>,
}

impl Agent {
    fn is_live(&self) -> bool {
        matches!(self.status, Status::Live)
    }
}

/// The kernel's state: a pure fold over the log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    next_seq: u64,
    next_agent: u64,
    next_corr: u64,
    next_cap: u64,
    drivers: BTreeMap<DriverId, Capability>,
    caps: BTreeMap<Capability, Endpoint>,
    agents: BTreeMap<AgentId, Agent>,
    corrs: BTreeMap<Corr, AgentId>,
    results: BTreeMap<AgentId, BlobRef>,
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
    /// The agent has already exited.
    #[error("agent {0} has exited")]
    AgentExited(AgentId),
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
    /// A claim for an agent with no stored result.
    #[error("agent {0} has no unclaimed result")]
    NoResult(AgentId),
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

    /// One agent's record, live or exited.
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

    /// An exit result that has not been claimed.
    #[must_use]
    pub fn result(&self, agent: AgentId) -> Option<BlobRef> {
        self.results.get(&agent).copied()
    }

    /// How many agents have not exited.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.agents.values().filter(|a| a.is_live()).count()
    }

    /// Whether a root was spawned and every agent has since exited.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        !self.agents.is_empty() && self.live_count() == 0
    }

    /// A digest of this state.
    ///
    /// Two folds of the same log must produce the same hash on every platform;
    /// that is the property the determinism jobs test. Serialization is
    /// order-stable because every collection here is a B-tree.
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
                        let p = self.live(*parent)?;
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
                let sender = self.live(id)?;
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
            Entry::Claimed { agent, .. } => {
                if !self.results.contains_key(agent) {
                    return Err(Refusal::NoResult(*agent));
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
                let a = self
                    .agents
                    .get_mut(agent)
                    .ok_or(Refusal::UnknownAgent(*agent))?;
                a.status = Status::Exited;
                // Mailbox hygiene (HANDOFF §4.9): unclaimed correlations and
                // undelivered mail dead-letter at exit. A reply that arrives
                // later finds no owner and is refused at the driver boundary.
                a.mailbox.clear();
                self.corrs.retain(|_, owner| owner != agent);
                // Unspent budget returns to the parent. The root has none; its
                // remainder stays on its record, which is where the harness's
                // grant is accounted for. A parent's record persists past its
                // own exit for exactly this reason: a child may outlive it.
                if let Some(parent) = a.parent {
                    let unspent = core::mem::replace(&mut a.budget, Budget::empty());
                    let p = self
                        .agents
                        .get_mut(&parent)
                        .ok_or(Refusal::UnknownAgent(parent))?;
                    p.budget.restore(&unspent)?;
                }
                self.results.insert(*agent, *result);
            }
            Entry::Claimed { agent, .. } => {
                self.results.remove(agent);
            }
        }
        self.next_seq = self.next_seq.saturating_add(1);
        Ok(())
    }
}

/// Records a driver's report against an agent and enforces `tokens`.
///
/// M0 enforcement is deliberately coarse: a report that overdraws the grant
/// drains it to zero and marks the agent exhausted, so its *next* send is
/// refused. Reservation before the call — refusing a send that could not be
/// paid for — is M1, with the rest of the budget dimensions.
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
