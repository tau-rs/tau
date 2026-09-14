//! Deterministic simulation harness for the tau kernel (HANDOFF §6, §8).
//!
//! A seeded generator drives the reducer through a random *legal* workload:
//! from the current [`State`] it proposes a candidate [`Entry`], keeps it if
//! [`State::check`] accepts, applies it, and repeats. The reducer draws no
//! randomness (ADR-0003); the generator may, and every draw comes from one
//! seeded PRNG so a run is a pure function of its seed.
//!
//! What the harness checks as it goes is the M1b conservation property: over
//! the whole tree, after every entry, budgets plus reservations plus spent
//! equal the root's grant plus overdraft, along every granted dimension but
//! `depth`. What it hands back is the log and the incrementally built state,
//! so a test can fold the log again — from memory, or through a serialized
//! round-trip — and compare [`State::hash`].
//!
//! Refusals are part of the workload, not a failure of it. The generator
//! proposes frozen senders, empty budgets, depth-zero parents, and clock
//! readings the reducer must accept or refuse on its own terms; a refusal is
//! counted and the next candidate is drawn. What *is* a failure is a candidate
//! the check accepted and the apply then refused: that would mean the reducer
//! can leave a partially applied entry behind, the one state the model has no
//! name for.

use std::collections::BTreeMap;

use tau_kernel::abi::{
    AgentId, BlobRef, Budget, Capability, Consumption, Corr, DimKey, DriverId, Endpoint, Msg,
    MsgKind, Name, NameError, Namespace, Seq,
};
use tau_kernel::log::{Entry, Log, LogError};
use tau_kernel::reducer::{Agent, Refusal, State, Status};

/// Why a run could not be completed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SimError {
    /// A fixed driver or dimension name failed validation.
    #[error("invalid name")]
    Name(#[from] NameError),
    /// The in-memory log rejected an append.
    #[error("log append failed")]
    Log(#[from] LogError),
    /// An entry the run cannot do without — boot, or the draining epilogue —
    /// was refused.
    #[error("a required entry was refused: {0}")]
    Required(Refusal),
    /// [`State::check`] accepted an entry and [`State::apply`] then refused it.
    /// The state may be partially applied; this is a reducer bug.
    #[error("check accepted an entry that apply refused at {at}: {refusal}")]
    Inconsistent {
        /// The position of the offending entry.
        at: Seq,
        /// What apply said.
        refusal: Refusal,
    },
    /// The conservation property broke.
    #[error("`{dim}` not conserved after entry {at}: sum {found}, expected {expected}")]
    NotConserved {
        /// The dimension.
        dim: DimKey,
        /// How many entries had been applied.
        at: u64,
        /// Budgets plus reservations plus spent over the tree.
        found: u64,
        /// The root's grant plus overdraft.
        expected: u64,
    },
    /// The generator could not find enough legal entries: too many proposals
    /// were refused or had no candidate.
    #[error("generator starved: {accepted} accepted after {proposals} proposals")]
    Starved {
        /// Entries accepted before giving up.
        accepted: u64,
        /// Proposals made.
        proposals: u64,
    },
}

/// A seeded PRNG: SplitMix64. Small, portable, and good enough for a workload
/// generator; the property under test is the reducer's, not the RNG's.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator whose whole future is fixed by `seed`.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; zero when `n` is zero.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64().rem_euclid(n)
    }

    /// Uniform in `0..=n`.
    pub fn up_to(&mut self, n: u64) -> u64 {
        match n.checked_add(1) {
            Some(bound) => self.below(bound),
            None => self.next_u64(),
        }
    }

    /// True with probability `1/n`.
    pub fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }

    /// A uniformly chosen element, or `None` if there is none.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        let len = u64::try_from(items.len()).ok()?;
        let idx = usize::try_from(self.below(len)).ok()?;
        items.get(idx)
    }

    /// A random 32-byte payload reference: content the kernel never reads.
    pub fn blob(&mut self) -> BlobRef {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            let word = self.next_u64().to_le_bytes();
            for (slot, byte) in chunk.iter_mut().zip(word) {
                *slot = byte;
            }
        }
        BlobRef::from_bytes(bytes)
    }
}

/// What a run produced.
#[derive(Debug)]
pub struct Report {
    /// Every accepted entry, in order.
    pub log: Log,
    /// The state built incrementally alongside the log.
    pub state: State,
    /// How many proposals the reducer accepted, boot and epilogue included.
    pub accepted: u64,
    /// How many proposals the reducer refused.
    pub refused: u64,
}

/// A fake driver: a name and the ceiling it was registered with.
#[derive(Clone, Debug)]
struct Driver {
    id: DriverId,
    cap: Capability,
    ceiling: Budget,
}

/// The fixed driver set every run boots with. Three ceilings of different
/// shapes so `send` reserves, and `Replied` settles, along more than one
/// dimension: a `calls` ceiling exercises the "ceiling plus one call" rule.
fn drivers() -> Result<Vec<(DriverId, Budget)>, NameError> {
    Ok(vec![
        (
            DriverId::new(Name::new("echo")?),
            Budget::from_dims([(DimKey::Tokens, 10)]),
        ),
        (
            DriverId::new(Name::new("model")?),
            Budget::from_dims([(DimKey::Tokens, 40), (DimKey::CostMicroUsd, 500)]),
        ),
        (
            DriverId::new(Name::new("tool")?),
            Budget::from_dims([(DimKey::ComputeMs, 20), (DimKey::Calls, 1)]),
        ),
    ])
}

/// The root's grant: every ceiling dimension, in amounts no run exhausts, a
/// wall grant that never runs out — the root must be timed so its children
/// can be — and four levels of depth.
fn root_grant() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 1_000_000),
        (DimKey::Calls, 1_000_000),
        (DimKey::CostMicroUsd, 1_000_000),
        (DimKey::ComputeMs, 1_000_000),
        (DimKey::WallMs, 1_000_000_000_000),
        (DimKey::Depth, 4),
    ])
}

/// The most a child is granted along each dimension: small, so children run
/// dry and the reducer's refusals get exercised.
fn child_cap(dim: &DimKey) -> u64 {
    match dim {
        DimKey::Tokens => 200,
        DimKey::Calls => 8,
        DimKey::WallMs => 500,
        DimKey::CostMicroUsd => 2_000,
        DimKey::ComputeMs => 100,
        _ => 50,
    }
}

fn is_live(a: &Agent) -> bool {
    matches!(a.status, Status::Live | Status::Cancelling)
}

/// The conservation property (Tier 2 property #1, checked here at every
/// step): over the whole tree, budgets plus reservations plus spent equal the
/// root's grant plus overdraft, along every granted dimension but `depth`.
///
/// # Errors
///
/// [`SimError::NotConserved`] naming the first dimension that fails.
pub fn conserved(state: &State, grant: &Budget) -> Result<(), SimError> {
    let mut sum: BTreeMap<DimKey, u64> = BTreeMap::new();
    let mut overdraft: BTreeMap<DimKey, u64> = BTreeMap::new();
    let add = |into: &mut BTreeMap<DimKey, u64>, dim: &DimKey, v: u64| {
        let slot = into.entry(dim.clone()).or_insert(0);
        *slot = slot.saturating_add(v);
    };
    for (_, a) in state.agents() {
        for (dim, v) in a.budget.iter() {
            add(&mut sum, dim, v);
        }
        for held in a.reserved.values() {
            for (dim, v) in held.iter() {
                add(&mut sum, dim, v);
            }
        }
        for (dim, v) in &a.spent {
            add(&mut sum, dim, *v);
        }
        for (dim, v) in &a.overdraft {
            add(&mut overdraft, dim, *v);
        }
    }
    for (dim, want) in grant.iter().filter(|(d, _)| **d != DimKey::Depth) {
        let over = overdraft.get(dim).copied().unwrap_or(0);
        let expected = want.saturating_add(over);
        let found = sum.get(dim).copied().unwrap_or(0);
        if found != expected {
            return Err(SimError::NotConserved {
                dim: dim.clone(),
                at: state.len(),
                found,
                expected,
            });
        }
    }
    Ok(())
}

/// One run in progress.
struct Sim {
    rng: Rng,
    state: State,
    log: Log,
    root: AgentId,
    grant: Budget,
    drivers: Vec<Driver>,
    /// A dimension no ceiling reserves, for replies that report along it.
    stray: DimKey,
    /// Which driver each open request went to. The reducer does not record
    /// this — any registered driver may answer — so the fake world does.
    routed: BTreeMap<Corr, usize>,
    accepted: u64,
    refused: u64,
}

/// What happened to a proposal.
enum Verdict {
    Accepted,
    Refused(Refusal),
}

impl Sim {
    /// Registers the drivers and spawns the root. Every boot entry is
    /// required.
    fn boot(seed: u64) -> Result<Self, SimError> {
        let mut sim = Self {
            rng: Rng::seeded(seed),
            state: State::initial(),
            log: Log::in_memory(),
            root: AgentId::new(0),
            grant: root_grant(),
            drivers: Vec::new(),
            stray: DimKey::Custom(Name::new("pixels")?),
            routed: BTreeMap::new(),
            accepted: 0,
            refused: 0,
        };
        for (id, ceiling) in drivers()? {
            let cap = Capability::mint(sim.state.next_cap());
            sim.require(Entry::DriverRegistered {
                seq: sim.state.next_seq(),
                driver: id.clone(),
                cap,
                ceiling: ceiling.clone(),
            })?;
            sim.drivers.push(Driver { id, cap, ceiling });
        }
        let root = sim.state.next_agent();
        sim.require(Entry::Spawned {
            seq: sim.state.next_seq(),
            parent: None,
            agent: root,
            ns: Namespace::from_caps(sim.drivers.iter().map(|d| d.cap)),
            budget: sim.grant.clone(),
        })?;
        sim.root = root;
        Ok(sim)
    }

    /// Offers `entry` to the reducer: check, then apply, then the
    /// conservation property.
    fn offer(&mut self, entry: Entry) -> Result<Verdict, SimError> {
        if let Err(refusal) = self.state.check(&entry) {
            self.refused = self.refused.saturating_add(1);
            return Ok(Verdict::Refused(refusal));
        }
        let at = entry.seq();
        if let Err(refusal) = self.state.apply(&entry) {
            return Err(SimError::Inconsistent { at, refusal });
        }
        if let Entry::Sent { msg, via } = &entry {
            if let (Some(corr), Some(idx)) =
                (msg.corr, self.drivers.iter().position(|d| d.cap == *via))
            {
                self.routed.insert(corr, idx);
            }
        }
        if let Entry::Replied { msg, .. } = &entry {
            if let Some(corr) = msg.corr {
                self.routed.remove(&corr);
            }
        }
        self.log.append(entry)?;
        self.accepted = self.accepted.saturating_add(1);
        // The grant exists once the root does; before that there is nothing
        // to conserve.
        if self.state.agent(self.root).is_some() {
            conserved(&self.state, &self.grant)?;
        }
        Ok(Verdict::Accepted)
    }

    /// Offers an entry the run cannot do without.
    fn require(&mut self, entry: Entry) -> Result<(), SimError> {
        match self.offer(entry)? {
            Verdict::Accepted => Ok(()),
            Verdict::Refused(refusal) => Err(SimError::Required(refusal)),
        }
    }

    // ------------------------------------------------------------ choosers

    fn live(&self) -> Vec<AgentId> {
        self.state
            .agents()
            .filter(|(_, a)| is_live(a))
            .map(|(id, _)| id)
            .collect()
    }

    fn live_non_root(&self) -> Vec<AgentId> {
        self.live()
            .into_iter()
            .filter(|id| *id != self.root)
            .collect()
    }

    fn pick_live(&mut self) -> Option<AgentId> {
        let live = self.live();
        self.rng.pick(&live).copied()
    }

    fn pick_live_non_root(&mut self) -> Option<AgentId> {
        let live = self.live_non_root();
        self.rng.pick(&live).copied()
    }

    fn pick_driver(&mut self) -> Option<&Driver> {
        let len = u64::try_from(self.drivers.len()).ok()?;
        let idx = usize::try_from(self.rng.below(len)).ok()?;
        self.drivers.get(idx)
    }

    // ------------------------------------------------------------ proposals

    /// A candidate entry for the current state, or `None` if the kind drawn
    /// has nothing to act on (no open request to answer, no mailbox to
    /// resolve, ...).
    fn propose(&mut self) -> Option<Entry> {
        match self.rng.below(100) {
            0..=14 => self.spawn(),
            15..=34 => self.send(),
            35..=54 => self.reply(),
            55..=69 => self.resolve(),
            70..=77 => self.exit(),
            78..=85 => self.claim(),
            86..=89 => self.cancel(),
            _ => Some(self.tick()),
        }
    }

    fn spawn(&mut self) -> Option<Entry> {
        let parent_id = self.pick_live()?;
        let parent = self.state.agent(parent_id)?;
        let (parent_ns, parent_budget) = (parent.ns.clone(), parent.budget.clone());
        let mut caps = Vec::new();
        for cap in parent_ns.iter() {
            if !self.rng.one_in(4) {
                caps.push(cap);
            }
        }
        let mut dims = Vec::new();
        for (dim, have) in parent_budget.iter() {
            if *dim == DimKey::Depth {
                continue;
            }
            // Leave a dimension out now and then: for `wall_ms` under a
            // timed parent that is the `Unbounded` refusal; for the rest it
            // is a child that cannot pay for some driver.
            if self.rng.one_in(10) {
                continue;
            }
            let amount = if self.rng.one_in(20) {
                have.saturating_add(1)
            } else {
                self.rng.up_to(have.min(child_cap(dim)))
            };
            dims.push((dim.clone(), amount));
        }
        // Depth: usually asked for — sometimes the parent's own level, which
        // is one too many — and sometimes left to the reducer to derive.
        //
        // Both paths are capped at 3 until #26 lands: today a finished agent
        // hands its `depth` back to its heir, so a child born at the
        // parent's level minus one grows the tree's depth exponentially and
        // overflows `u64` inside a tick-driven abort. The derived path is
        // only taken under shallow parents for the same reason. Lift both
        // caps when that closes.
        let level = parent_budget.get(&DimKey::Depth).unwrap_or(0);
        if level > 4 || !self.rng.one_in(3) {
            dims.push((DimKey::Depth, self.rng.up_to(level.min(3))));
        }
        Some(Entry::Spawned {
            seq: self.state.next_seq(),
            parent: Some(parent_id),
            agent: self.state.next_agent(),
            ns: Namespace::from_caps(caps),
            budget: Budget::from_dims(dims),
        })
    }

    fn send(&mut self) -> Option<Entry> {
        let from = self.pick_live()?;
        let held: Vec<Capability> = self.state.agent(from)?.ns.iter().collect();
        let via = if held.is_empty() || self.rng.one_in(10) {
            // A capability the sender may not hold: `NotHeld`, usually.
            self.pick_driver()?.cap
        } else {
            *self.rng.pick(&held)?
        };
        let payload = self.rng.blob();
        Some(Entry::Sent {
            msg: Msg::new(
                self.state.next_seq(),
                Endpoint::Agent { id: from },
                MsgKind::Request,
                payload,
            )
            .with_corr(self.state.next_corr()),
            via,
        })
    }

    fn reply(&mut self) -> Option<Entry> {
        let open: Vec<(AgentId, Corr)> = self
            .state
            .agents()
            .filter(|(_, a)| is_live(a))
            .flat_map(|(id, a)| a.reserved.keys().map(move |corr| (id, *corr)))
            .collect();
        let (to, corr) = *self.rng.pick(&open)?;
        let idx = self.routed.get(&corr).copied().unwrap_or(0);
        let driver = self.drivers.get(idx)?.clone();
        let mut dims = Vec::new();
        for (dim, held) in driver.ceiling.iter() {
            let used = if self.rng.one_in(8) {
                // Above the ceiling: charged in full, recorded as overdraft.
                held.saturating_add(self.rng.up_to(held)).saturating_add(1)
            } else {
                self.rng.up_to(held)
            };
            dims.push((dim.clone(), used));
        }
        if self.rng.one_in(16) {
            // Along a dimension nobody reserved: overdraft entirely.
            dims.push((self.stray.clone(), self.rng.up_to(50).saturating_add(1)));
        }
        let payload = self.rng.blob();
        let mut msg = Msg::new(
            self.state.next_seq(),
            Endpoint::Driver { id: driver.id },
            MsgKind::Reply,
            payload,
        )
        .with_corr(corr);
        if !self.rng.one_in(10) {
            msg = msg.with_consumption(Consumption::from_dims(dims));
        }
        Some(Entry::Replied { msg, to })
    }

    fn resolve(&mut self) -> Option<Entry> {
        let full: Vec<AgentId> = self
            .state
            .agents()
            .filter(|(_, a)| is_live(a) && !a.mailbox.is_empty())
            .map(|(id, _)| id)
            .collect();
        let agent = *self.rng.pick(&full)?;
        let seqs: Vec<Seq> = self
            .state
            .agent(agent)?
            .mailbox
            .iter()
            .map(|m| m.seq)
            .collect();
        let matched = *self.rng.pick(&seqs)?;
        Some(Entry::Resolved {
            seq: self.state.next_seq(),
            agent,
            matched,
        })
    }

    fn exit(&mut self) -> Option<Entry> {
        let agent = self.pick_live_non_root()?;
        let result = self.rng.blob();
        Some(Entry::Exited {
            seq: self.state.next_seq(),
            agent,
            result,
        })
    }

    fn claim(&mut self) -> Option<Entry> {
        let completed: Vec<(AgentId, Option<AgentId>)> = self
            .state
            .completed()
            .iter()
            .map(|c| (c.agent, c.parent))
            .collect();
        let (agent, parent) = *self.rng.pick(&completed)?;
        let parent_live = parent
            .and_then(|p| self.state.agent(p))
            .is_some_and(is_live);
        let by = if parent_live && !self.rng.one_in(4) {
            parent
        } else {
            None
        };
        Some(Entry::Claimed {
            seq: self.state.next_seq(),
            agent,
            by,
        })
    }

    fn cancel(&mut self) -> Option<Entry> {
        let (by, agent) = if self.rng.one_in(3) {
            (None, self.pick_live_non_root()?)
        } else {
            let by = self.pick_live()?;
            let below: Vec<AgentId> = self
                .live()
                .into_iter()
                .filter(|id| self.state.is_descendant(*id, by))
                .collect();
            (Some(by), *self.rng.pick(&below)?)
        };
        let grace = self.rng.up_to(100);
        let reason = self.rng.blob();
        Some(Entry::Cancelled {
            seq: self.state.next_seq(),
            by,
            agent,
            grace,
            reason,
        })
    }

    fn tick(&mut self) -> Entry {
        let step = if self.rng.one_in(8) {
            0
        } else {
            self.rng.up_to(50)
        };
        Entry::Tick {
            seq: self.state.next_seq(),
            now: self.state.now().saturating_add(step),
        }
    }

    // ---------------------------------------------------------------- run

    /// Proposes until `events` entries have been accepted.
    fn drive(&mut self, events: u64) -> Result<(), SimError> {
        let target = self.accepted.saturating_add(events);
        let mut proposals: u64 = 0;
        let limit = events.saturating_mul(20).max(64);
        while self.accepted < target {
            proposals = proposals.saturating_add(1);
            if proposals > limit {
                return Err(SimError::Starved {
                    accepted: self.accepted,
                    proposals,
                });
            }
            if let Some(entry) = self.propose() {
                self.offer(entry)?;
            }
        }
        Ok(())
    }

    /// Ends the run the way a harness would: cancel the root with no grace,
    /// which aborts every live agent at once, then claim what the tree left
    /// behind. Every entry here is required.
    fn drain(&mut self) -> Result<(), SimError> {
        let reason = self.rng.blob();
        self.require(Entry::Cancelled {
            seq: self.state.next_seq(),
            by: None,
            agent: self.root,
            grace: 0,
            reason,
        })?;
        let left: Vec<AgentId> = self.state.completed().iter().map(|c| c.agent).collect();
        for agent in left {
            self.require(Entry::Claimed {
                seq: self.state.next_seq(),
                agent,
                by: None,
            })?;
        }
        Ok(())
    }
}

/// Runs one simulation: boot, `events` accepted entries, then the draining
/// epilogue. The result is a pure function of `seed` and `events`.
///
/// # Errors
///
/// [`SimError`] if the run could not be completed — including the two that
/// would be reducer bugs, [`SimError::Inconsistent`] and
/// [`SimError::NotConserved`].
pub fn run(seed: u64, events: u64) -> Result<Report, SimError> {
    let mut sim = Sim::boot(seed)?;
    sim.drive(events)?;
    sim.drain()?;
    Ok(Report {
        log: sim.log,
        state: sim.state,
        accepted: sim.accepted,
        refused: sim.refused,
    })
}
