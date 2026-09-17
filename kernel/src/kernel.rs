//! The kernel proper: the log, the reducer, the blob store, and the queues
//! between them, behind one lock.
//!
//! # Executor-agnostic by construction
//!
//! Nothing here names an async runtime. Syscalls that wait (`recv`, `wait`, a
//! driver's next delivery, the harness's drain) are plain futures over
//! `std::task` wakers, and new tasks are handed to a [`Spawner`] the harness
//! supplies at boot, which hands back the [`AbortHandle`] the kernel pulls at
//! a cancel deadline. ADR-0003 chooses tokio for the *body*; that choice lives
//! in the harness, and dependencies point inward.
//!
//! # Append-before-apply
//!
//! [`Inner::commit`] is the only path that mutates state, and it is check →
//! append → apply, in that order. A refused entry never reaches the log; an
//! entry that reached the log and then failed to apply is a kernel bug, and
//! the kernel records it as a [fault](KernelError::Faulted) rather than
//! carrying on with a log and a state that disagree.
//!
//! # Cancel, in two phases
//!
//! [`Kernel::cancel`] commits one `Cancelled` entry — the atomic freeze — and
//! then, outside the lock, tells each driver to abandon the subtree's open
//! requests. The deadline is enforced by the reducer as ticks are applied
//! ([`Kernel::tick`]); the kernel's only job at that point is to pull the abort
//! handles of the agents the reducer just declared dead. The reducer decides;
//! the executor obeys. Every syscall also fails closed on a finished agent, so
//! a task the executor has not yet reaped cannot act.
//!
//! # Hooks, under the lock
//!
//! [`Kernel::attach`] installs a hook program at boot. At each pinned point
//! (ADR-0008 §1) the syscall builds a [`HookEvent`], [`Inner::consult`]s
//! every hook attached there in install order under the same lock, and
//! commits the roll call as one `Verdicts` entry plus one `Emitted` entry
//! per note — before the governed entry at a pre point, after the causing
//! entry at an on point. The live closures live here beside the drivers; the
//! reducer holds only their records, and the fold never runs one.
//!
//! # Driver supervision (ADR-0014)
//!
//! A driver that *answers* is fully handled by its reply, whatever it says.
//! A driver that does not is the kernel's to close out, so no request ever
//! hangs: the loop that polls `handle` catches an unwind and writes
//! `DriverDown { crashed }` plus one `Unanswered` per request it had taken;
//! [`Kernel::tick`] writes `Unanswered { overdue }` for every request past
//! the bound its driver was registered with; and the harness's two verbs,
//! [`Kernel::replace_driver`] and [`Kernel::retire_driver`], write the
//! health transitions they cause. Each closed request bills its ceiling if
//! the driver had taken it and nothing if it was still queued. Health, the
//! bound and who holds what are cache here — [`DriverSlot`], `in_flight` —
//! never reducer state. What happens to the driver next is the supervisor's
//! call, told through [`Kernel::supervise`].

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use crate::abi::{
    AgentId, BlobRef, Budget, Capability, Consumption, Corr, DimKey, DownCause, DriverId, Endpoint,
    HookId, Msg, MsgKind, Namespace, Seq, UnansweredCause,
};
use crate::blob::{self, Blobs, Memory};
use crate::driver::{Driver, ToolSchema};
use crate::hook::{
    crossings, FailureMode, HookEvent, HookFailure, HookPoint, HookProgram, Roll, Ruling, Verdict,
};
use crate::log::{Entry, Log, LogError};
use crate::reducer::{as_consumption, Outcome, Refusal, State, StateHash, Status};
use crate::syscall::{CancelMode, Exit, ExitResult, Handle, Program, WaitFor};

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Stops a task the harness's executor is running. Called at most once, at a
/// cancel deadline; a no-op if the task has already finished.
pub type AbortHandle = Box<dyn FnOnce() + Send>;

/// How the kernel hands a task to the harness's executor, and gets back the
/// means to abort it.
pub type Spawner = Arc<dyn Fn(BoxFuture<()>) -> AbortHandle + Send + Sync>;

/// How many undelivered requests a driver's inbox holds before `send` reports
/// [`KernelError::WouldBlock`]. Backpressure is a visible error, never a hidden
/// await (ADR-0002).
pub const INBOX_CAPACITY: usize = 64;

/// Why a syscall or a harness call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KernelError {
    /// The reducer refused the entry; nothing was logged.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// The log could not be written.
    #[error(transparent)]
    Log(#[from] LogError),
    /// The driver's inbox is full. Retry later; nothing was logged.
    #[error("driver {0} would block")]
    WouldBlock(DriverId),
    /// A `wait(Any)` with no live and no unclaimed children: it would never
    /// resolve, so it resolves to this instead.
    #[error("agent {0} has no children to wait for")]
    NoChildren(AgentId),
    /// A hook denied it (ADR-0008 §3). The roll call is in the log; the
    /// governed entry is not.
    #[error("denied by {hook}: {reason}")]
    Denied {
        /// The hook that said no.
        hook: HookId,
        /// Its reason — or, for a hook that failed closed, its error.
        reason: String,
    },
    /// The kernel's log and state disagree. Nothing further will be accepted.
    #[error("kernel faulted: {reason}")]
    Faulted {
        /// What went wrong, for the operator.
        reason: String,
    },
    /// The harness shut the kernel down.
    #[error("kernel is shut down")]
    Closed,
}

/// Why [`Kernel::shred`] refused (ADR-0012 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ShredError {
    /// An agent in the subtree is live or cancelling; nothing was shredded.
    #[error("{0} is still live; cancel it and wait for the abort before shredding")]
    Live(AgentId),
}

/// A request routed to a driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// The correlation to reply on.
    pub corr: Corr,
    /// The requesting agent.
    pub from: AgentId,
    /// The payload bytes.
    pub payload: Vec<u8>,
}

struct Inbox {
    queue: VecDeque<Delivery>,
    waker: Option<Waker>,
}

/// What the supervisor is told (ADR-0014 §6). Every event is after the
/// fact: the kernel has already closed the request, recorded the transition
/// and billed it; the supervisor decides only what happens to the driver
/// next, through [`Kernel::replace_driver`] and [`Kernel::retire_driver`],
/// and doing nothing is a valid policy for every event.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DriverEvent {
    /// The loop unwound. `DriverDown { crashed }` and its `Unanswered`s are
    /// already logged; the inbox stays, and a replacement takes what is
    /// queued in it.
    Crashed {
        /// The driver.
        driver: DriverId,
    },
    /// A request passed the bound. Its `Unanswered { overdue }` is already
    /// logged. The driver is not declared down: slow and dead look the same
    /// from outside, and which it is is the supervisor's call.
    Overdue {
        /// The driver.
        driver: DriverId,
        /// The request.
        corr: Corr,
        /// Its owner.
        agent: AgentId,
    },
    /// A `Replied` settled above the ceiling. The overdraft is already on
    /// the agent, loud in the state hash (#18); nothing is logged beyond the
    /// `Replied` that carries it.
    Overdrew {
        /// The driver.
        driver: DriverId,
        /// The request.
        corr: Corr,
        /// Its owner, who carries the overdraft.
        agent: AgentId,
        /// How far above the ceiling, per dimension.
        excess: Consumption,
    },
}

/// Where a driver is in its life, as the kernel — not the fold — knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Health {
    /// Its loop is running.
    Up,
    /// Its loop unwound; the inbox stays for a replacement.
    Down,
    /// The harness retired it; the capability is unroutable for good.
    Retired,
}

/// One registered driver: the instance, the bound, and the loop's handle.
/// Cache beside the fold's `drivers`/`ceilings`, never canonical state.
struct DriverSlot {
    /// The instance, so `cancel` can reach `abandon` while its loop is
    /// inside `handle`. Replaced whole by `replace_driver` and dropped by
    /// `retire_driver`; the old one goes when its aborted loop lets go of
    /// it too.
    driver: Option<Arc<dyn Driver>>,
    /// `reply_within` at registration: how long a request may wait for its
    /// answer, counted from its `Sent`, in `Tick.now` units. `None` is
    /// unbounded.
    reply_within: Option<u64>,
    health: Health,
    /// The loop's abort handle. "Nothing cancels a driver loop but shutdown"
    /// became "nothing but shutdown and replacement" (ADR-0014 §5).
    abort: Option<AbortHandle>,
}

/// An open request as the kernel tracks it: who holds it, when it was sent,
/// whether the driver has taken it from the inbox. Derivable from the log
/// but for the last bit, which is exactly the one that decides the bill of
/// an `Unanswered` (ADR-0014 §3).
struct InFlight {
    driver: DriverId,
    /// The clock reading at `Sent`.
    sent_at: u64,
    /// Whether `next_delivery` has handed it to the driver.
    taken: bool,
}

pub(crate) struct Inner {
    log: Log,
    state: State,
    blobs: Box<dyn Blobs>,
    spawner: Spawner,
    /// Registered drivers with their health, bound and loop.
    drivers: BTreeMap<DriverId, DriverSlot>,
    /// The live hook programs, by id. Cache, not state: the reducer holds
    /// their records, and the fold never consults them.
    hooks: BTreeMap<HookId, HookProgram>,
    /// Every open request, by correlation. Cache, not state: derivable from
    /// the `Sent` entries' capabilities and the ticks between, kept warm for
    /// `cancel`, the bound, and the bill of an `Unanswered`.
    in_flight: BTreeMap<Corr, InFlight>,
    /// The executor's handle on each live agent's task.
    aborts: BTreeMap<AgentId, AbortHandle>,
    agent_wakers: BTreeMap<AgentId, Waker>,
    inboxes: BTreeMap<DriverId, Inbox>,
    drain_waker: Option<Waker>,
    /// What the supervisor has not yet been told, in the order it happened.
    events: VecDeque<DriverEvent>,
    supervise_waker: Option<Waker>,
    fault: Option<String>,
    closed: bool,
}

impl Inner {
    fn ensure_ok(&self) -> Result<(), KernelError> {
        if let Some(reason) = &self.fault {
            return Err(KernelError::Faulted {
                reason: reason.clone(),
            });
        }
        if self.closed {
            return Err(KernelError::Closed);
        }
        Ok(())
    }

    /// Check, append, apply. The only writer of state.
    fn commit(&mut self, entry: Entry) -> Result<(), KernelError> {
        self.state.check(&entry)?;
        self.log.append(entry.clone())?;
        if let Err(refusal) = self.state.apply(&entry) {
            let reason = format!("entry {} was logged but refused: {refusal}", entry.seq());
            self.fault = Some(reason.clone());
            if let Some(w) = self.drain_waker.take() {
                w.wake();
            }
            return Err(KernelError::Faulted { reason });
        }
        Ok(())
    }

    fn wake_agent(&mut self, agent: AgentId) {
        if let Some(w) = self.agent_wakers.remove(&agent) {
            w.wake();
        }
    }

    fn wake_parent_of(&mut self, agent: AgentId) {
        if let Some(parent) = self.state.agent(agent).and_then(|a| a.parent) {
            self.wake_agent(parent);
        }
    }

    fn wake_drain_if_done(&mut self) {
        if self.state.is_drained() || self.fault.is_some() {
            if let Some(w) = self.drain_waker.take() {
                w.wake();
            }
        }
    }

    /// Bookkeeping after the reducer finished the agents in `ids`, by exit or
    /// abort: forget their in-flight requests, wake whoever waits on them, and
    /// hand back the abort handles of the ones the executor still runs.
    fn reap(&mut self, ids: &[AgentId]) -> Vec<AbortHandle> {
        let mut handles = Vec::new();
        for id in ids {
            let finished = self
                .state
                .agent(*id)
                .is_some_and(|a| matches!(a.status, Status::Exited | Status::Aborted));
            if !finished {
                continue;
            }
            if let Some(h) = self.aborts.remove(id) {
                handles.push(h);
            }
            self.wake_agent(*id);
            self.wake_parent_of(*id);
        }
        let state = &self.state;
        self.in_flight
            .retain(|corr, _| state.owner(*corr).is_some());
        self.wake_drain_if_done();
        handles
    }

    // ------------------------------------------------------------ supervision

    /// Tells the supervisor, if one is listening.
    fn raise(&mut self, event: DriverEvent) {
        self.events.push_back(event);
        if let Some(w) = self.supervise_waker.take() {
            w.wake();
        }
    }

    /// Closes one open request without its driver's answer (ADR-0014 §2–§3):
    /// the envelope is from the kernel, empty, on the request's correlation,
    /// billed the ceiling if the driver had taken it and nothing if not. Fires
    /// `OnBudget` as any entry that moves a grant; `PreDeliver` does not see
    /// it — there is nothing here a hook could refuse, the request must
    /// close. Forgets the request and wakes its owner.
    fn unanswered(&mut self, corr: Corr, cause: UnansweredCause) -> Result<(), KernelError> {
        let Some(open) = self.in_flight.remove(&corr) else {
            return Ok(());
        };
        if !open.taken {
            // Still in the inbox: it leaves with the correlation, or a
            // replacement would take a request that is already closed and
            // its answer would be dead letter.
            if let Some(inbox) = self.inboxes.get_mut(&open.driver) {
                inbox.queue.retain(|d| d.corr != corr);
            }
        }
        let Some(to) = self.state.owner(corr) else {
            // Finished between the driver's taking it and now; the request
            // went with the owner's record. Nothing to close.
            return Ok(());
        };
        let mut msg = Msg::new(
            self.state.next_seq(),
            Endpoint::Kernel,
            MsgKind::Reply,
            BlobRef::EMPTY,
        )
        .with_corr(corr);
        if open.taken {
            let ceiling = self
                .state
                .ceiling(&open.driver)
                .cloned()
                .unwrap_or_default();
            msg = msg.with_consumption(as_consumption(&ceiling));
        }
        let entry = Entry::Unanswered {
            msg,
            to,
            driver: open.driver,
            cause,
        };
        let before = self.before(&entry, &[]);
        self.commit(entry)?;
        self.after(before)?;
        self.wake_agent(to);
        Ok(())
    }

    /// Every open request routed to `driver`, in correlation order, with
    /// whether the driver has taken it.
    fn open_for(&self, driver: &DriverId) -> Vec<(Corr, bool)> {
        self.in_flight
            .iter()
            .filter(|(_, open)| open.driver == *driver)
            .map(|(corr, open)| (*corr, open.taken))
            .collect()
    }

    /// The loop for `id` observed `handle` unwinding: `DriverDown { crashed }`,
    /// then one `Unanswered { crashed }` per request the driver had taken,
    /// under this one lock. Queued requests stay queued for a replacement.
    fn crashed(&mut self, id: &DriverId) -> Result<(), KernelError> {
        self.ensure_ok()?;
        let Some(slot) = self.drivers.get_mut(id) else {
            return Ok(());
        };
        if slot.health != Health::Up {
            // Replaced or retired while the unwind was in flight: the
            // transition is already in the log.
            return Ok(());
        }
        slot.health = Health::Down;
        slot.abort = None;
        let entry = Entry::DriverDown {
            seq: self.state.next_seq(),
            driver: id.clone(),
            cause: DownCause::Crashed,
        };
        self.commit(entry)?;
        for (corr, taken) in self.open_for(id) {
            if taken {
                self.unanswered(corr, UnansweredCause::Crashed)?;
            }
        }
        self.raise(DriverEvent::Crashed { driver: id.clone() });
        Ok(())
    }

    /// The bound, enforced (ADR-0014 §5): every open request whose driver
    /// has a `reply_within` and whose `sent_at + reply_within <= now` closes
    /// as `Unanswered { overdue }`, billed per whether it was taken. Because
    /// the loop is sequential, a hung `handle` makes every request behind it
    /// overdue in turn, each at its own bound. The driver is not declared
    /// down.
    fn overdue(&mut self, now: u64) -> Result<(), KernelError> {
        let due: Vec<(Corr, DriverId)> = self
            .in_flight
            .iter()
            .filter(|(_, open)| {
                self.drivers
                    .get(&open.driver)
                    .and_then(|slot| slot.reply_within)
                    .is_some_and(|bound| open.sent_at.saturating_add(bound) <= now)
            })
            .map(|(corr, open)| (*corr, open.driver.clone()))
            .collect();
        for (corr, driver) in due {
            let Some(agent) = self.state.owner(corr) else {
                self.in_flight.remove(&corr);
                continue;
            };
            self.unanswered(corr, UnansweredCause::Overdue)?;
            self.raise(DriverEvent::Overdue {
                driver,
                corr,
                agent,
            });
        }
        Ok(())
    }

    /// The instance behind `driver`, if it is up: what `cancel` reaches
    /// with `abandon`. A driver that is down has no open requests to
    /// abandon, because they were unanswered when it went down.
    fn driver_up(&self, driver: &DriverId) -> Option<Arc<dyn Driver>> {
        let slot = self.drivers.get(driver)?;
        (slot.health == Health::Up)
            .then(|| slot.driver.as_ref().map(Arc::clone))
            .flatten()
    }

    /// The harness's half of a replacement or a retirement, under the lock:
    /// takes the loop's abort handle to fire outside it, writes
    /// `DriverDown { retired }` if the driver was up, and closes its open
    /// requests as `Unanswered { retired }` — the taken ones only when
    /// `keep_queue` (a replacement takes the queue), every one otherwise.
    /// Returns the handle. Does not change the slot's health; the caller
    /// sets what comes next.
    fn take_down(
        &mut self,
        id: &DriverId,
        keep_queue: bool,
    ) -> Result<Option<AbortHandle>, KernelError> {
        let slot = self
            .drivers
            .get_mut(id)
            .ok_or_else(|| Refusal::WrongSender(Endpoint::Driver { id: id.clone() }))?;
        match slot.health {
            Health::Retired => {
                let cap = self.state.driver_cap(id).unwrap_or(Capability::alloc(0));
                return Err(Refusal::Unroutable(cap).into());
            }
            Health::Down => {}
            Health::Up => {
                let entry = Entry::DriverDown {
                    seq: self.state.next_seq(),
                    driver: id.clone(),
                    cause: DownCause::Retired,
                };
                self.commit(entry)?;
            }
        }
        let abort = self.drivers.get_mut(id).and_then(|slot| slot.abort.take());
        for (corr, taken) in self.open_for(id) {
            if taken || !keep_queue {
                self.unanswered(corr, UnansweredCause::Retired)?;
            }
        }
        if !keep_queue {
            if let Some(inbox) = self.inboxes.get_mut(id) {
                inbox.queue.clear();
            }
        }
        Ok(abort)
    }

    // ------------------------------------------------------------------ hooks

    /// Whether any hook is attached at `point`; the cheap gate before an
    /// event is built.
    fn hooked(&self, point: &HookPoint) -> bool {
        self.state.hooks_at(point).next().is_some()
    }

    /// Whether any hook is attached at any `OnBudget` point.
    fn budget_hooked(&self) -> bool {
        self.state.budget_points().next().is_some()
    }

    /// The facts every event carries about its subject: parent and depth.
    fn lineage(&self, agent: AgentId) -> (Option<AgentId>, u64) {
        self.state.agent(agent).map_or((None, 0), |a| {
            (a.parent, a.budget.get(&DimKey::Depth).unwrap_or(0))
        })
    }

    /// Consults every hook at `point` about `event`, in install order, and
    /// commits what they said: one `Verdicts` entry, then one `Emitted` per
    /// note. Stops at the first ruling that stops it. Returns the hook that
    /// denied and its reason, if one did.
    ///
    /// Writes nothing when no hook is attached at `point`.
    fn consult(
        &mut self,
        point: &HookPoint,
        event: &HookEvent,
    ) -> Result<Option<(HookId, String)>, KernelError> {
        let ids: Vec<HookId> = self.state.hooks_at(point).collect();
        if ids.is_empty() {
            return Ok(None);
        }
        let mut roll: Roll = Vec::with_capacity(ids.len());
        let mut notes: Vec<(HookId, AgentId, Vec<u8>)> = Vec::new();
        let mut denied = None;
        let subject = event.subject();
        for id in ids {
            let (Some(program), Some(record)) = (self.hooks.get(&id), self.state.hook(id)) else {
                // Attached in the log but not live here: this kernel did not
                // install it, which a boot-only registry makes impossible.
                return Err(KernelError::Faulted {
                    reason: format!("{id} is attached but has no program"),
                });
            };
            let mode = record.failure;
            let answer = if point.admits_deny() {
                program.run(event)
            } else {
                // A `Deny` where nothing can be stopped is a program failure
                // (ADR-0008 §1), and at an on point the mode does not matter.
                match program.run(event) {
                    Ok(Verdict::Deny(reason)) => Err(HookFailure::new(format!(
                        "deny at {point}, which admits none: {reason}"
                    ))),
                    other => other,
                }
            };
            let ruling = match answer {
                Ok(Verdict::Allow) => Ruling::Allow,
                Ok(Verdict::Deny(reason)) => {
                    let blob = self.blobs.put(subject, reason.as_bytes());
                    denied = Some((id, reason));
                    Ruling::Deny(blob)
                }
                Ok(Verdict::Emit { to, payload }) => {
                    let blob = self.blobs.put(to, &payload);
                    notes.push((id, to, payload));
                    Ruling::Emit { to, payload: blob }
                }
                Err(HookFailure { message }) => {
                    let error = self.blobs.put(subject, message.as_bytes());
                    if mode == FailureMode::Closed && point.admits_deny() {
                        denied = Some((id, message));
                    }
                    Ruling::Failed { mode, error }
                }
            };
            let stop = ruling.stops(point);
            roll.push((id, ruling));
            if stop {
                break;
            }
        }
        let entry = Entry::Verdicts {
            seq: self.state.next_seq(),
            point: point.clone(),
            subject,
            roll,
        };
        self.commit(entry)?;
        for (hook, to, payload) in notes {
            let msg = Msg::new(
                self.state.next_seq(),
                Endpoint::Hook { id: hook },
                MsgKind::Notice,
                blob::digest(&payload),
            );
            self.commit(Entry::Emitted { hook, to, msg })?;
            self.wake_agent(to);
        }
        Ok(denied)
    }

    /// The `OnExit` facts of `agent`, captured before the entry that ends
    /// it: what it holds, budget and reservations, goes back up the tree in
    /// that apply and is gone from the record afterwards.
    fn before_exit(&self, agent: AgentId) -> Option<ExitFacts> {
        let a = self.state.agent(agent)?;
        let mut unspent = a.budget.clone();
        for held in a.reserved.values() {
            // Restoring what was carved from this very grant cannot
            // overflow; the fallback keeps the kernel free of `unwrap`.
            let _ = unspent.restore(held);
        }
        Some(ExitFacts {
            agent,
            parent: a.parent,
            depth: a.budget.get(&DimKey::Depth).unwrap_or(0),
            unspent,
        })
    }

    /// Everything an on point needs to know before an entry is committed,
    /// gathered only when a hook could use it.
    fn before(&self, entry: &Entry, may_end: &[AgentId]) -> Aftermath {
        let exits = if self.hooked(&HookPoint::OnExit) {
            may_end
                .iter()
                .filter_map(|id| self.before_exit(*id))
                .collect()
        } else {
            Vec::new()
        };
        let remaining = if self.budget_hooked() {
            self.state.remaining(&self.state.budget_candidates(entry))
        } else {
            Vec::new()
        };
        Aftermath {
            seq: entry.seq(),
            mark: self.state.completion_mark(),
            exits,
            remaining,
            result: None,
        }
    }

    /// Fires the on points an entry caused, in this order: `OnExit` for
    /// every agent it finished, deepest first; then `OnBudget` for every
    /// crossing, in point order. A `Deny` here is a recorded failure, so
    /// nothing is returned.
    fn after(&mut self, before: Aftermath) -> Result<(), KernelError> {
        let Aftermath {
            seq,
            mark,
            exits,
            remaining,
            result,
        } = before;
        let finished: Vec<(AgentId, Outcome)> = self
            .state
            .completed_since(mark)
            .map(|c| (c.agent, c.outcome))
            .collect();
        for (agent, outcome) in finished {
            let Some(facts) = exits.iter().find(|f| f.agent == agent) else {
                continue;
            };
            let result = match outcome {
                Outcome::Exited(_) => result.clone(),
                Outcome::Aborted => None,
            };
            let event = HookEvent::OnExit {
                seq,
                subject: agent,
                parent: facts.parent,
                depth: facts.depth,
                outcome,
                result,
                unspent: facts.unspent.clone(),
            };
            self.consult(&HookPoint::OnExit, &event)?;
        }
        if remaining.is_empty() {
            return Ok(());
        }
        let points: Vec<HookPoint> = self.state.budget_points().cloned().collect();
        let fired = crossings(points.iter(), &remaining, |id| self.state.budget_of(id));
        for (point, subject, was) in fired {
            let HookPoint::OnBudget { dim, below } = &point else {
                continue;
            };
            let (parent, depth) = self.lineage(subject);
            let event = HookEvent::OnBudget {
                seq,
                subject,
                parent,
                depth,
                dim: dim.clone(),
                below: *below,
                remaining: was,
            };
            self.consult(&point, &event)?;
        }
        Ok(())
    }
}

/// Polls a driver's `handle` future inside `catch_unwind`, so a panic in a
/// driver is an outcome the loop sees rather than an unwind that ends the
/// task in silence (ADR-0014 §5). `AssertUnwindSafe` because the future is
/// dropped on `Err`, never polled again. Under `panic = "abort"` there is
/// nothing to catch, which is that profile's contract.
struct Guarded {
    inner: BoxFuture<(Vec<u8>, Consumption)>,
}

impl Future for Guarded {
    type Output = Result<(Vec<u8>, Consumption), ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.inner.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(out)) => Poll::Ready(Ok(out)),
            Err(_) => Poll::Ready(Err(())),
        }
    }
}

/// `handle` itself may unwind before it returns a future; that is a crash
/// too.
fn guarded(driver: &Arc<dyn Driver>, delivery: Delivery) -> Result<Guarded, ()> {
    catch_unwind(AssertUnwindSafe(|| driver.handle(delivery)))
        .map(|inner| Guarded { inner })
        .map_err(|_| ())
}

/// What an `OnExit` event needs from an agent's record before the entry
/// that ends it.
struct ExitFacts {
    agent: AgentId,
    parent: Option<AgentId>,
    depth: u64,
    unspent: Budget,
}

/// The pre-commit snapshot the on points are computed from.
struct Aftermath {
    /// The position of the entry being committed.
    seq: Seq,
    /// The completion mark before it.
    mark: u64,
    /// The exit facts of every agent it might end.
    exits: Vec<ExitFacts>,
    /// The remaining grant of every agent it might lower.
    remaining: Vec<(AgentId, Budget)>,
    /// The result bytes, for an `Exited`.
    result: Option<Vec<u8>>,
}

/// The kernel. One per run; shared by the harness, every agent handle, and
/// every driver loop.
pub struct Kernel {
    inner: Mutex<Inner>,
}

impl Kernel {
    /// Boots a kernel over `log`, with `spawner` as the way to run tasks and
    /// an in-memory blob store.
    ///
    /// With tokio: `|fut| { let h = tokio::spawn(fut); Box::new(move || h.abort()) }`.
    pub fn boot<S>(log: Log, spawner: S) -> Arc<Self>
    where
        S: Fn(BoxFuture<()>) -> AbortHandle + Send + Sync + 'static,
    {
        Self::boot_with(log, spawner, Box::new(Memory::new()))
    }

    /// Boots a kernel over `log` with the payload store the harness chose
    /// (ADR-0012 §1): [`Memory`], or a persistent store beside the log.
    pub fn boot_with<S>(log: Log, spawner: S, blobs: Box<dyn Blobs>) -> Arc<Self>
    where
        S: Fn(BoxFuture<()>) -> AbortHandle + Send + Sync + 'static,
    {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                log,
                state: State::initial(),
                blobs,
                spawner: Arc::new(spawner),
                drivers: BTreeMap::new(),
                hooks: BTreeMap::new(),
                in_flight: BTreeMap::new(),
                aborts: BTreeMap::new(),
                agent_wakers: BTreeMap::new(),
                inboxes: BTreeMap::new(),
                drain_waker: None,
                events: VecDeque::new(),
                supervise_waker: None,
                fault: None,
                closed: false,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned lock means a panic happened while it was held. The state
        // behind it is still a fold over the log, so it is still coherent;
        // refusing to proceed would turn one agent's panic into a hang for all.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---------------------------------------------------------------- harness

    /// Registers a driver under `id` and mints the capability that names it.
    ///
    /// Must happen before the root is spawned: a namespace can only be built
    /// from capabilities that exist.
    ///
    /// `ceiling` is the most one request to this driver may cost, and it is
    /// the harness's declaration, not the driver's: the kernel never parses a
    /// payload, so it cannot price a request itself. Every `send` through the
    /// returned capability reserves the ceiling plus one `calls` from the
    /// sender *before* delivery — refused if the sender cannot cover it — and
    /// the reply settles the reservation against what the driver reports.
    ///
    /// Unbounded: [`register_driver_with`](Self::register_driver_with) with
    /// `reply_within: None`, which is today's behaviour — a request to this
    /// driver waits for its answer for as long as its owner lives.
    ///
    /// # Errors
    ///
    /// [`Refusal::AfterBoot`] or [`Refusal::DriverExists`], via
    /// [`KernelError::Refused`]; or a log write failure.
    pub fn register_driver<D: Driver>(
        self: &Arc<Self>,
        id: DriverId,
        driver: D,
        ceiling: Budget,
    ) -> Result<Capability, KernelError> {
        self.register_driver_with(id, driver, ceiling, None)
    }

    /// [`register_driver`](Self::register_driver) with a bound (ADR-0014
    /// §2): `reply_within` is the most clock units a request to this driver
    /// may wait for its answer, counted from its `Sent`, in the units the
    /// clock publishes. A request past it is closed by the `tick` that
    /// passes the bound, as `Unanswered { overdue }`, billed the ceiling if
    /// the driver had taken it and nothing if it was still queued. The
    /// driver's own timeouts must be shorter, so a driver that *can* report
    /// a failure does so before the kernel stops waiting.
    ///
    /// # Errors
    ///
    /// As [`register_driver`](Self::register_driver).
    pub fn register_driver_with<D: Driver>(
        self: &Arc<Self>,
        id: DriverId,
        driver: D,
        ceiling: Budget,
        reply_within: Option<u64>,
    ) -> Result<Capability, KernelError> {
        let driver: Arc<dyn Driver> = Arc::new(driver);
        let cap = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            let cap = Capability::alloc(inner.state.next_cap());
            let entry = Entry::DriverRegistered {
                seq: inner.state.next_seq(),
                driver: id.clone(),
                cap,
                ceiling,
                reply_within,
            };
            inner.commit(entry)?;
            inner.inboxes.insert(
                id.clone(),
                Inbox {
                    queue: VecDeque::new(),
                    waker: None,
                },
            );
            inner.drivers.insert(
                id.clone(),
                DriverSlot {
                    driver: Some(Arc::clone(&driver)),
                    reply_within,
                    health: Health::Up,
                    abort: None,
                },
            );
            cap
        };
        self.spawn_loop(id, driver);
        Ok(cap)
    }

    /// Spawns the loop for `id` over `driver`: `next_delivery` → `handle` →
    /// `reply`, one request at a time, until the kernel shuts down or the
    /// loop is aborted by a replacement. Keeps the abort handle on the slot.
    fn spawn_loop(self: &Arc<Self>, id: DriverId, driver: Arc<dyn Driver>) {
        let kernel = Arc::clone(self);
        let loop_id = id.clone();
        let spawner = Arc::clone(&self.lock().spawner);
        let abort = spawner(Box::pin(async move {
            while let Some(delivery) = kernel.next_delivery(loop_id.clone()).await {
                let corr = delivery.corr;
                let answer = match guarded(&driver, delivery) {
                    Ok(fut) => fut.await,
                    Err(()) => Err(()),
                };
                match answer {
                    Ok((payload, consumed)) => {
                        // A reply nobody can receive — the owner exited, or
                        // the request was already closed as unanswered — is
                        // dead letter by design (HANDOFF §4.9, ADR-0014 §5):
                        // it drops on the floor and the log shows exactly
                        // that, no `Replied` entry and no second bill.
                        let _ = kernel.reply(&loop_id, corr, &payload, consumed);
                    }
                    Err(()) => {
                        // The driver raised instead of reporting. The
                        // request it had taken is closed here, under one
                        // lock, and the loop ends; the inbox stays for a
                        // replacement (ADR-0014 §5). A `DriverDown` the log
                        // cannot take has already faulted the kernel.
                        let _ = kernel.lock().crashed(&loop_id);
                        return;
                    }
                }
            }
        }));
        let mut inner = self.lock();
        if let Some(slot) = inner.drivers.get_mut(&id) {
            if slot.health == Health::Up && slot.abort.is_none() {
                slot.abort = Some(abort);
                return;
            }
        }
        // The loop already ended — it crashed before this lock, or the
        // driver was retired — or a replacement raced in; nothing to keep.
        drop(inner);
        abort();
    }

    /// Restarts `id` with a fresh instance (ADR-0014 §5): same `DriverId`,
    /// same `Capability` — the address every namespace holds survives — and
    /// no new `DriverRegistered`, so it is allowed after boot, unlike
    /// registration. If the old loop is still running it is aborted, the
    /// old instance dropped, `DriverDown { retired }` written and every
    /// request it had taken closed as `Unanswered { retired }` at the
    /// ceiling; a driver that had crashed already has its `DriverDown`.
    /// Then `DriverUp`, and a new loop over the same inbox, which takes
    /// whatever is still queued. The bound is the registration's.
    ///
    /// The kernel never re-uses an instance that raised: supply a fresh one.
    ///
    /// # Errors
    ///
    /// [`Refusal::WrongSender`] naming the driver if `id` was never
    /// registered, [`Refusal::Unroutable`] if it was retired, via
    /// [`KernelError::Refused`]; or a log write failure.
    pub fn replace_driver<D: Driver>(
        self: &Arc<Self>,
        id: &DriverId,
        driver: D,
    ) -> Result<(), KernelError> {
        let fresh: Arc<dyn Driver> = Arc::new(driver);
        let old_loop = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            let old_loop = inner.take_down(id, true)?;
            let entry = Entry::DriverUp {
                seq: inner.state.next_seq(),
                driver: id.clone(),
            };
            inner.commit(entry)?;
            if let Some(slot) = inner.drivers.get_mut(id) {
                slot.driver = Some(Arc::clone(&fresh));
                slot.health = Health::Up;
            }
            old_loop
        };
        if let Some(abort) = old_loop {
            abort();
        }
        self.spawn_loop(id.clone(), fresh);
        Ok(())
    }

    /// Retires `id` for good (ADR-0014 §5): `DriverDown { retired }` if it
    /// was up, every open request — taken or queued — closed as
    /// `Unanswered { retired }` (the ceiling for a taken one, nothing for a
    /// queued one), the inbox removed, so every later `send` through its
    /// capability is [`Refusal::Unroutable`] and `describe` through it is
    /// `None`. A second retire is a no-op.
    ///
    /// # Errors
    ///
    /// [`Refusal::WrongSender`] naming the driver if `id` was never
    /// registered, via [`KernelError::Refused`]; or a log write failure.
    pub fn retire_driver(&self, id: &DriverId) -> Result<(), KernelError> {
        let old_loop = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            if inner
                .drivers
                .get(id)
                .is_some_and(|slot| slot.health == Health::Retired)
            {
                return Ok(());
            }
            let old_loop = inner.take_down(id, false)?;
            if let Some(slot) = inner.drivers.get_mut(id) {
                slot.health = Health::Retired;
                slot.driver = None;
            }
            // Removing the inbox is what makes the capability unroutable;
            // its waker, if any, is the old loop's, which is being aborted.
            inner.inboxes.remove(id);
            old_loop
        };
        if let Some(abort) = old_loop {
            abort();
        }
        Ok(())
    }

    /// The next thing a supervisor would want to know (ADR-0014 §6). A
    /// future, like [`drained`](Self::drained): resolves to the oldest
    /// event not yet taken, or to [`KernelError::Closed`] once the kernel
    /// has shut down with none left, or [`KernelError::Faulted`]. The
    /// supervisor is optional — a harness that never polls this still
    /// drains, given a clock and a bound.
    pub fn supervise(self: &Arc<Self>) -> Supervise {
        Supervise {
            kernel: Arc::clone(self),
        }
    }

    /// Syscall 7. Installs a hook program at `point` (ADR-0008 §4).
    ///
    /// On `Kernel`, not `Handle`: the harness's privilege is structural, not
    /// a permission bit — an agent cannot express the call. Boot-only, like
    /// [`register_driver`](Self::register_driver): refused once the root
    /// exists, and there is no `detach`. The returned id is the hook's
    /// position in the roll call at its point; hooks are consulted in the
    /// order they were attached.
    ///
    /// `failure` says what a verdict the program fails to produce counts
    /// as. At a point that admits `Deny` — `PreSend`, `PreDeliver`,
    /// `OnSpawn` — it must be [`FailureMode::Closed`].
    ///
    /// A native program runs under the kernel lock, synchronously: no I/O,
    /// no `await`, no lock of its own, no call back into the kernel, no
    /// panic. That is enforced by review, not by the compiler.
    ///
    /// # Errors
    ///
    /// [`Refusal::AfterBoot`], [`Refusal::OpenAtVetoPoint`], or
    /// [`Refusal::RulePoint`], via [`KernelError::Refused`]; or a log write
    /// failure.
    pub fn attach(
        self: &Arc<Self>,
        point: HookPoint,
        program: HookProgram,
        failure: FailureMode,
    ) -> Result<HookId, KernelError> {
        if let HookProgram::Rule(rule) = &program {
            if *rule.point() != point {
                return Err(Refusal::RulePoint {
                    rule: rule.point().clone(),
                    attached: point,
                }
                .into());
            }
        }
        let mut inner = self.lock();
        inner.ensure_ok()?;
        let id = inner.state.next_hook();
        let entry = Entry::Attached {
            seq: inner.state.next_seq(),
            hook: id,
            point,
            failure,
            program: program.source(),
        };
        inner.commit(entry)?;
        inner.hooks.insert(id, program);
        Ok(id)
    }

    /// Spawns the root agent. The harness's grant is `budget`, whole.
    ///
    /// # Errors
    ///
    /// [`Refusal::RootExists`] or [`Refusal::UnknownCapability`], via
    /// [`KernelError::Refused`]; or a log write failure.
    pub fn spawn_root(
        self: &Arc<Self>,
        program: Program,
        ns: Namespace,
        budget: Budget,
    ) -> Result<AgentId, KernelError> {
        self.spawn(None, program, ns, budget)
    }

    /// Resolves when every agent has finished, or the kernel has faulted.
    pub fn drained(self: &Arc<Self>) -> Drained {
        Drained {
            kernel: Arc::clone(self),
        }
    }

    /// Stops accepting syscalls and ends every driver loop. Not a health
    /// event: the run is over, and nothing is written (ADR-0014 §5).
    pub fn shutdown(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        let mut wakers: Vec<Waker> = inner
            .inboxes
            .values_mut()
            .filter_map(|inbox| inbox.waker.take())
            .collect();
        wakers.extend(inner.supervise_waker.take());
        drop(inner);
        for w in wakers {
            w.wake();
        }
    }

    /// Claims a stored outcome. The claim is a log entry.
    ///
    /// This is the harness's claim, for outcomes the tree left behind — the
    /// root's, or an orphan's. An agent claiming its child's is `wait`.
    ///
    /// # Errors
    ///
    /// [`Refusal::NoResult`] via [`KernelError::Refused`], or a log failure.
    pub fn claim(&self, agent: AgentId) -> Result<Outcome, KernelError> {
        let mut inner = self.lock();
        if let Some(reason) = &inner.fault {
            return Err(KernelError::Faulted {
                reason: reason.clone(),
            });
        }
        let outcome = inner.state.result(agent).ok_or(Refusal::NoResult(agent))?;
        let entry = Entry::Claimed {
            seq: inner.state.next_seq(),
            agent,
            by: None,
        };
        inner.commit(entry)?;
        Ok(outcome)
    }

    /// The harness's `cancel`: freezes `agent` and its subtree, with the
    /// notice sent from [`Endpoint::Harness`]. See [`Handle::cancel`] for the
    /// phases.
    ///
    /// # Errors
    ///
    /// [`KernelError::Refused`] if `agent` is not live or is already
    /// cancelled; or a log failure.
    pub fn cancel_from_harness(
        &self,
        agent: AgentId,
        mode: &CancelMode,
    ) -> Result<(), KernelError> {
        self.cancel(None, agent, mode)
    }

    /// Publishes a clock reading as a `Tick` entry. Wall budgets are charged
    /// and cancel deadlines enforced by the reducer as the tick applies, and
    /// the tasks of the agents it ended are stopped before this returns.
    ///
    /// This is the clock source's entry point, not an agent's: time enters the
    /// system here and nowhere else (ADR-0003).
    ///
    /// # Errors
    ///
    /// [`Refusal::ClockRewound`] via [`KernelError::Refused`] if `now` reads
    /// earlier than the last tick; or a log failure.
    pub fn tick(&self, now: u64) -> Result<(), KernelError> {
        let handles = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            let entry = Entry::Tick {
                seq: inner.state.next_seq(),
                now,
            };
            inner.state.check(&entry)?;
            let ending = inner.state.expiring(now);
            let before = inner.before(&entry, &ending);
            inner.commit(entry)?;
            inner.after(before)?;
            let handles = inner.reap(&ending);
            // The bound (ADR-0014 §5): after the tick committed and the
            // agents it ended are gone, every request past its driver's
            // `reply_within` closes here, before the lock is released.
            inner.overdue(now)?;
            handles
        };
        for abort in handles {
            abort();
        }
        Ok(())
    }

    /// The bytes behind a payload reference, if the store holds them.
    ///
    /// Not a syscall: a blob is immutable, content-addressed data, and reading
    /// it has no effect to log.
    #[must_use]
    pub fn read(&self, blob: BlobRef) -> Option<Vec<u8>> {
        self.lock().blobs.get(&blob)
    }

    /// Erases the payloads of `root` and every agent below it (ADR-0012 §3):
    /// the store drops each agent's key, and every reference their entries
    /// carry reads as `None` from now on, in every copy of the store.
    ///
    /// Harness-level, like [`attach`](Self::attach): not a syscall, and not
    /// a log entry — the log records the run, and a shred has no effect on
    /// it, by construction. The subtree must be finished: a live agent would
    /// keep sealing payloads under a key that no longer exists. Cancel it,
    /// wait for the abort, then shred. An agent the kernel does not know, or
    /// one that put nothing, is a no-op; so is a second shred.
    ///
    /// # Errors
    ///
    /// [`ShredError::Live`] naming the first agent in the subtree that is
    /// still live or cancelling. Nothing is shredded in that case.
    pub fn shred(&self, root: AgentId) -> Result<(), ShredError> {
        let mut inner = self.lock();
        let subtree = inner.state.subtree(root);
        if let Some(live) = subtree.iter().copied().find(|id| {
            inner
                .state
                .agent(*id)
                .is_some_and(|a| matches!(a.status, Status::Live | Status::Cancelling))
        }) {
            return Err(ShredError::Live(live));
        }
        for owner in subtree {
            inner.blobs.shred(owner);
        }
        Ok(())
    }

    /// A copy of every log entry so far.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        self.lock().log.entries().to_vec()
    }

    /// A copy of the current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.lock().state.clone()
    }

    /// The digest of the current state.
    #[must_use]
    pub fn state_hash(&self) -> StateHash {
        self.lock().state.hash()
    }

    // --------------------------------------------------------------- syscalls

    pub(crate) fn spawn(
        self: &Arc<Self>,
        parent: Option<AgentId>,
        program: Program,
        ns: Namespace,
        budget: Budget,
    ) -> Result<AgentId, KernelError> {
        let (id, spawner) = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            let id = inner.state.next_agent();
            let mut entry = Entry::Spawned {
                seq: inner.state.next_seq(),
                parent,
                agent: id,
                ns,
                budget,
            };
            inner.state.check(&entry)?;
            if inner.hooked(&HookPoint::OnSpawn) {
                let Entry::Spawned { ns, budget, .. } = &entry else {
                    return Err(KernelError::Faulted {
                        reason: "spawn built a non-spawn entry".into(),
                    });
                };
                let event = HookEvent::OnSpawn {
                    seq: inner.state.next_seq(),
                    subject: id,
                    parent,
                    depth: inner.state.birth_depth(parent, budget),
                    ns: ns.clone(),
                    remaining: budget.clone(),
                };
                if let Some((hook, reason)) = inner.consult(&HookPoint::OnSpawn, &event)? {
                    return Err(KernelError::Denied { hook, reason });
                }
                if let Entry::Spawned { seq, .. } = &mut entry {
                    *seq = inner.state.next_seq();
                }
            }
            let before = inner.before(&entry, &[]);
            inner.commit(entry)?;
            inner.after(before)?;
            (id, Arc::clone(&inner.spawner))
        };
        let handle = Handle::new(Arc::clone(self), id);
        let abort = spawner(Box::pin(async move {
            let Exit { .. } = program(handle).await;
        }));
        // The task may already have finished on another worker — by its own
        // exit, or by a zero-grace cancel that ran before this lock. Keep the
        // handle only while there is something to abort; otherwise fire it
        // now, so a reducer-aborted task is stopped rather than merely failed
        // closed.
        let mut inner = self.lock();
        let live = inner
            .state
            .agent(id)
            .is_some_and(|a| matches!(a.status, Status::Live | Status::Cancelling));
        if live {
            inner.aborts.insert(id, abort);
        } else {
            drop(inner);
            abort();
        }
        Ok(id)
    }

    pub(crate) fn exit(&self, agent: AgentId, result: &[u8]) {
        let mut inner = self.lock();
        let blob = inner.blobs.put(agent, result);
        let entry = Entry::Exited {
            seq: inner.state.next_seq(),
            agent,
            result: blob,
        };
        let mut before = inner.before(&entry, &[agent]);
        before.result = Some(result.to_vec());
        match inner.commit(entry).and_then(|()| inner.after(before)) {
            Ok(()) => {}
            // The reducer aborted this agent between its last poll and its
            // exit. The abort is the outcome of record; the exit is a no-op.
            Err(KernelError::Refused(Refusal::AgentExited(_))) => {}
            Err(err) => {
                // `exit` cannot fail from the agent's side — the handle is
                // consumed and there is no one to return to. Anything else
                // that stops the exit from being logged is a kernel fault,
                // and the harness hears it.
                if inner.fault.is_none() {
                    inner.fault = Some(format!("exit of {agent} could not be logged: {err}"));
                }
            }
        }
        // The exiting task is the one running this; its handle is spent.
        let _ = inner.reap(&[agent]);
    }

    pub(crate) fn cancel(
        &self,
        by: Option<AgentId>,
        agent: AgentId,
        mode: &CancelMode,
    ) -> Result<(), KernelError> {
        let (abandons, handles) = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            let entry = Entry::Cancelled {
                seq: inner.state.next_seq(),
                by,
                agent,
                grace: mode.grace,
                reason: blob::digest(&mode.reason),
            };
            inner.state.check(&entry)?;
            // Phase two's targets, read before the freeze: every open request
            // of every agent in the subtree, and the driver holding it.
            let subtree = inner.state.subtree(agent);
            let abandons: Vec<(Arc<dyn Driver>, Corr)> = subtree
                .iter()
                .flat_map(|id| inner.state.open_corrs(*id))
                .filter_map(|corr| {
                    let open = inner.in_flight.get(&corr)?;
                    Some((inner.driver_up(&open.driver)?, corr))
                })
                .collect();
            inner.blobs.put(agent, &mode.reason);
            let before = inner.before(&entry, &subtree);
            inner.commit(entry)?;
            inner.after(before)?;
            // Everyone frozen sees the notice; a grace of zero has already
            // aborted them, and `reap` sorts one from the other.
            for id in &subtree {
                inner.wake_agent(*id);
            }
            let handles = inner.reap(&subtree);
            (abandons, handles)
        };
        for (driver, corr) in abandons {
            driver.abandon(corr);
        }
        for abort in handles {
            abort();
        }
        Ok(())
    }

    /// The tool schema of the driver behind `via`, for `from`. A read, not
    /// a syscall: see `Handle::describe`.
    ///
    /// The driver is called outside the kernel lock. Its `describe` is
    /// documented as cheap and pure, but a lock held across foreign code is
    /// a deadlock waiting for a driver that reaches back in.
    pub(crate) fn describe(
        &self,
        from: AgentId,
        via: Capability,
    ) -> Result<Option<(DriverId, ToolSchema)>, KernelError> {
        let (id, driver) = {
            let inner = self.lock();
            inner.ensure_ok()?;
            let agent = inner.state.agent(from).ok_or(Refusal::UnknownAgent(from))?;
            if !agent.ns.holds(via) {
                return Err(Refusal::NotHeld {
                    agent: from,
                    cap: via,
                }
                .into());
            }
            let id = match inner.state.resolve(via) {
                Some(Endpoint::Driver { id }) => id.clone(),
                Some(_) => return Err(Refusal::Unroutable(via).into()),
                None => return Err(Refusal::UnknownCapability(via).into()),
            };
            let slot = inner.drivers.get(&id).ok_or(Refusal::Unroutable(via))?;
            // A retired driver is no tool (ADR-0014 §5): the capability
            // resolves, and nothing answers behind it.
            let Some(driver) = slot
                .driver
                .as_ref()
                .filter(|_| slot.health != Health::Retired)
            else {
                return Ok(None);
            };
            (id, Arc::clone(driver))
        };
        Ok(driver.describe().map(|schema| (id, schema)))
    }

    pub(crate) fn send(
        &self,
        from: AgentId,
        via: Capability,
        payload: &[u8],
    ) -> Result<Corr, KernelError> {
        let mut inner = self.lock();
        inner.ensure_ok()?;
        let corr = inner.state.next_corr();
        let envelope = |seq| {
            Msg::new(
                seq,
                Endpoint::Agent { id: from },
                MsgKind::Request,
                blob::digest(payload),
            )
            .with_corr(corr)
        };
        let mut entry = Entry::Sent {
            msg: envelope(inner.state.next_seq()),
            via,
        };
        inner.state.check(&entry)?;
        let driver = match inner.state.resolve(via) {
            Some(Endpoint::Driver { id }) => id.clone(),
            _ => return Err(Refusal::Unroutable(via).into()),
        };
        let Some(inbox) = inner.inboxes.get(&driver) else {
            return Err(Refusal::Unroutable(via).into());
        };
        if inbox.queue.len() >= INBOX_CAPACITY {
            return Err(KernelError::WouldBlock(driver));
        }
        if inner.hooked(&HookPoint::PreSend) {
            let (parent, depth) = inner.lineage(from);
            let event = HookEvent::PreSend {
                seq: inner.state.next_seq(),
                subject: from,
                parent,
                depth,
                driver: driver.clone(),
                corr,
                payload: payload.to_vec(),
                remaining: inner.state.budget_of(from).cloned().unwrap_or_default(),
            };
            if let Some((hook, reason)) = inner.consult(&HookPoint::PreSend, &event)? {
                return Err(KernelError::Denied { hook, reason });
            }
            entry = Entry::Sent {
                msg: envelope(inner.state.next_seq()),
                via,
            };
        }
        inner.blobs.put(from, payload);
        let before = inner.before(&entry, &[]);
        inner.commit(entry)?;
        inner.after(before)?;
        let sent_at = inner.state.now();
        inner.in_flight.insert(
            corr,
            InFlight {
                driver: driver.clone(),
                sent_at,
                taken: false,
            },
        );
        if let Some(inbox) = inner.inboxes.get_mut(&driver) {
            inbox.queue.push_back(Delivery {
                corr,
                from,
                payload: payload.to_vec(),
            });
            if let Some(w) = inbox.waker.take() {
                w.wake();
            }
        }
        Ok(corr)
    }

    /// Resolves a `recv` if a message in `agent`'s mailbox satisfies `filter`.
    pub(crate) fn poll_recv(
        &self,
        agent: AgentId,
        filter: &crate::syscall::Match,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Msg, KernelError>> {
        let mut inner = self.lock();
        if let Err(err) = inner.ensure_ok() {
            return Poll::Ready(Err(err));
        }
        let Some(record) = inner.state.agent(agent) else {
            return Poll::Ready(Err(Refusal::UnknownAgent(agent).into()));
        };
        if matches!(record.status, Status::Exited | Status::Aborted) {
            return Poll::Ready(Err(Refusal::AgentExited(agent).into()));
        }
        let Some(msg) = record.mailbox.iter().find(|m| filter.matches(m)).cloned() else {
            inner.agent_wakers.insert(agent, cx.waker().clone());
            return Poll::Pending;
        };
        let entry = Entry::Resolved {
            seq: inner.state.next_seq(),
            agent,
            matched: msg.seq,
        };
        Poll::Ready(inner.commit(entry).map(|()| msg))
    }

    /// Resolves a `wait` if a matching child has finished; the claim is the
    /// `Claimed` entry.
    pub(crate) fn poll_wait(
        &self,
        agent: AgentId,
        what: WaitFor,
        cx: &mut Context<'_>,
    ) -> Poll<Result<ExitResult, KernelError>> {
        let mut inner = self.lock();
        if let Err(err) = inner.ensure_ok() {
            return Poll::Ready(Err(err));
        }
        let Some(record) = inner.state.agent(agent) else {
            return Poll::Ready(Err(Refusal::UnknownAgent(agent).into()));
        };
        if matches!(record.status, Status::Exited | Status::Aborted) {
            return Poll::Ready(Err(Refusal::AgentExited(agent).into()));
        }
        let completion = match what {
            WaitFor::Child(child) => {
                let Some(rec) = inner.state.agent(child) else {
                    return Poll::Ready(Err(Refusal::UnknownAgent(child).into()));
                };
                if rec.parent != Some(agent) {
                    return Poll::Ready(Err(Refusal::NotChild { agent, child }.into()));
                }
                match (inner.state.result(child), rec.status) {
                    (Some(outcome), _) => (child, outcome),
                    (None, Status::Live | Status::Cancelling) => {
                        inner.agent_wakers.insert(agent, cx.waker().clone());
                        return Poll::Pending;
                    }
                    // Finished, and already claimed — by the harness, since
                    // only the parent and the harness may claim.
                    (None, Status::Exited | Status::Aborted) => {
                        return Poll::Ready(Err(Refusal::NoResult(child).into()));
                    }
                }
            }
            WaitFor::Any => match inner.state.next_completed_child(agent) {
                Some(c) => (c.agent, c.outcome),
                None if inner.state.has_live_children(agent) => {
                    inner.agent_wakers.insert(agent, cx.waker().clone());
                    return Poll::Pending;
                }
                None => return Poll::Ready(Err(KernelError::NoChildren(agent))),
            },
        };
        let (child, outcome) = completion;
        let entry = Entry::Claimed {
            seq: inner.state.next_seq(),
            agent: child,
            by: Some(agent),
        };
        Poll::Ready(inner.commit(entry).map(|()| ExitResult {
            agent: child,
            outcome,
        }))
    }

    // ---------------------------------------------------------------- drivers

    /// A driver's reply to a request it was delivered.
    ///
    /// # Errors
    ///
    /// [`Refusal::UnknownCorr`] if the owner has finished (dead letter), via
    /// [`KernelError::Refused`]; or a log failure.
    pub fn reply(
        &self,
        driver: &DriverId,
        corr: Corr,
        payload: &[u8],
        consumed: Consumption,
    ) -> Result<(), KernelError> {
        let mut inner = self.lock();
        inner.ensure_ok()?;
        let to = inner
            .state
            .owner(corr)
            .ok_or(Refusal::UnknownCorr(Some(corr)))?;
        let envelope = |seq| {
            Msg::new(
                seq,
                Endpoint::Driver { id: driver.clone() },
                MsgKind::Reply,
                blob::digest(payload),
            )
            .with_corr(corr)
            .with_consumption(consumed.clone())
        };
        let mut entry = Entry::Replied {
            msg: envelope(inner.state.next_seq()),
            to,
        };
        inner.state.check(&entry)?;
        if inner.hooked(&HookPoint::PreDeliver) {
            let (parent, depth) = inner.lineage(to);
            let event = HookEvent::PreDeliver {
                seq: inner.state.next_seq(),
                subject: to,
                parent,
                depth,
                driver: driver.clone(),
                corr,
                kind: MsgKind::Reply,
                payload: payload.to_vec(),
                remaining: inner.state.budget_of(to).cloned().unwrap_or_default(),
            };
            if let Some((hook, reason)) = inner.consult(&HookPoint::PreDeliver, &event)? {
                return Err(KernelError::Denied { hook, reason });
            }
            entry = Entry::Replied {
                msg: envelope(inner.state.next_seq()),
                to,
            };
        }
        inner.blobs.put(to, payload);
        let before = inner.before(&entry, &[]);
        inner.commit(entry)?;
        inner.after(before)?;
        inner.in_flight.remove(&corr);
        inner.wake_agent(to);
        // A report above the ceiling settled as overdraft; the supervisor
        // hears it as an event and nothing more is logged (ADR-0014 §6).
        let ceiling = inner.state.ceiling(driver).cloned().unwrap_or_default();
        let excess = Consumption::from_dims(consumed.iter().filter_map(|(dim, used)| {
            let held = ceiling.get(dim).unwrap_or(0);
            (used > held).then(|| (dim.clone(), used.saturating_sub(held)))
        }));
        if !excess.is_empty() {
            inner.raise(DriverEvent::Overdrew {
                driver: driver.clone(),
                corr,
                agent: to,
                excess,
            });
        }
        Ok(())
    }

    /// The next request routed to `driver`; `None` once the kernel shuts down.
    pub fn next_delivery(self: &Arc<Self>, driver: DriverId) -> NextDelivery {
        NextDelivery {
            kernel: Arc::clone(self),
            driver,
        }
    }
}

/// Resolves when every agent has finished. See [`Kernel::drained`].
pub struct Drained {
    kernel: Arc<Kernel>,
}

impl Future for Drained {
    type Output = Result<(), KernelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.kernel.lock();
        if let Some(reason) = &inner.fault {
            return Poll::Ready(Err(KernelError::Faulted {
                reason: reason.clone(),
            }));
        }
        if inner.state.is_drained() {
            return Poll::Ready(Ok(()));
        }
        inner.drain_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Resolves to a driver's next delivery. See [`Kernel::next_delivery`].
pub struct NextDelivery {
    kernel: Arc<Kernel>,
    driver: DriverId,
}

impl Future for NextDelivery {
    type Output = Option<Delivery>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.kernel.lock();
        if inner.closed || inner.fault.is_some() {
            return Poll::Ready(None);
        }
        let Some(inbox) = inner.inboxes.get_mut(&self.driver) else {
            return Poll::Ready(None);
        };
        match inbox.queue.pop_front() {
            Some(delivery) => {
                // Taken: from here an unanswered request bills its ceiling
                // (ADR-0014 §3), because the driver may have done the work.
                if let Some(open) = inner.in_flight.get_mut(&delivery.corr) {
                    open.taken = true;
                }
                Poll::Ready(Some(delivery))
            }
            None => {
                inbox.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Resolves to the next [`DriverEvent`]. See [`Kernel::supervise`].
pub struct Supervise {
    kernel: Arc<Kernel>,
}

impl Future for Supervise {
    type Output = Result<DriverEvent, KernelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.kernel.lock();
        if let Some(event) = inner.events.pop_front() {
            return Poll::Ready(Ok(event));
        }
        if let Err(err) = inner.ensure_ok() {
            return Poll::Ready(Err(err));
        }
        inner.supervise_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
