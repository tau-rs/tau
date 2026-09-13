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

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use crate::abi::{
    AgentId, BlobRef, Budget, Capability, Consumption, Corr, DriverId, Endpoint, Msg, MsgKind,
    Namespace,
};
use crate::blob::BlobStore;
use crate::driver::Driver;
use crate::log::{Entry, Log, LogError};
use crate::reducer::{Outcome, Refusal, State, StateHash, Status};
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

pub(crate) struct Inner {
    log: Log,
    state: State,
    blobs: BlobStore,
    spawner: Spawner,
    /// Registered drivers, so `cancel` can reach `abandon` while the driver's
    /// own loop is inside `handle`.
    drivers: BTreeMap<DriverId, Arc<dyn Driver>>,
    /// Which driver holds each open request. Cache, not state: derivable from
    /// the `Sent` entries' capabilities, kept warm for `cancel`.
    in_flight: BTreeMap<Corr, DriverId>,
    /// The executor's handle on each live agent's task.
    aborts: BTreeMap<AgentId, AbortHandle>,
    agent_wakers: BTreeMap<AgentId, Waker>,
    inboxes: BTreeMap<DriverId, Inbox>,
    drain_waker: Option<Waker>,
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
}

/// The kernel. One per run; shared by the harness, every agent handle, and
/// every driver loop.
pub struct Kernel {
    inner: Mutex<Inner>,
}

impl Kernel {
    /// Boots a kernel over `log`, with `spawner` as the way to run tasks.
    ///
    /// With tokio: `|fut| { let h = tokio::spawn(fut); Box::new(move || h.abort()) }`.
    pub fn boot<S>(log: Log, spawner: S) -> Arc<Self>
    where
        S: Fn(BoxFuture<()>) -> AbortHandle + Send + Sync + 'static,
    {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                log,
                state: State::initial(),
                blobs: BlobStore::new(),
                spawner: Arc::new(spawner),
                drivers: BTreeMap::new(),
                in_flight: BTreeMap::new(),
                aborts: BTreeMap::new(),
                agent_wakers: BTreeMap::new(),
                inboxes: BTreeMap::new(),
                drain_waker: None,
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
    /// # Errors
    ///
    /// [`Refusal::AfterBoot`] or [`Refusal::DriverExists`], via
    /// [`KernelError::Refused`]; or a log write failure.
    pub fn register_driver<D: Driver>(
        self: &Arc<Self>,
        id: DriverId,
        driver: D,
    ) -> Result<Capability, KernelError> {
        let driver: Arc<dyn Driver> = Arc::new(driver);
        let (cap, spawner) = {
            let mut inner = self.lock();
            inner.ensure_ok()?;
            let cap = Capability::alloc(inner.state.next_cap());
            let entry = Entry::DriverRegistered {
                seq: inner.state.next_seq(),
                driver: id.clone(),
                cap,
            };
            inner.commit(entry)?;
            inner.inboxes.insert(
                id.clone(),
                Inbox {
                    queue: VecDeque::new(),
                    waker: None,
                },
            );
            inner.drivers.insert(id.clone(), Arc::clone(&driver));
            (cap, Arc::clone(&inner.spawner))
        };
        let kernel = Arc::clone(self);
        // Driver loops are not agents: nothing cancels them but shutdown, so
        // the abort handle is not kept.
        let _ = spawner(Box::pin(async move {
            while let Some(delivery) = kernel.next_delivery(id.clone()).await {
                let corr = delivery.corr;
                let (payload, consumed) = driver.handle(delivery).await;
                // A reply nobody can receive — the owner exited — is dead
                // letter by design (HANDOFF §4.9). Driver supervision (M3)
                // will surface the error envelope; for now it drops on the
                // floor and the log shows exactly that: no `Replied` entry.
                let _ = kernel.reply(&id, corr, &payload, consumed);
            }
        }));
        Ok(cap)
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

    /// Stops accepting syscalls and ends every driver loop.
    pub fn shutdown(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        let wakers: Vec<Waker> = inner
            .inboxes
            .values_mut()
            .filter_map(|inbox| inbox.waker.take())
            .collect();
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

    /// Publishes a clock reading as a `Tick` entry. Cancel deadlines the
    /// reading reaches are enforced by the reducer as the tick applies, and
    /// the aborted agents' tasks are stopped before this returns.
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
            let expiring = inner.state.expiring(now);
            inner.commit(entry)?;
            inner.reap(&expiring)
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
        self.lock().blobs.get(&blob).map(<[u8]>::to_vec)
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
            let entry = Entry::Spawned {
                seq: inner.state.next_seq(),
                parent,
                agent: id,
                ns,
                budget,
            };
            inner.commit(entry)?;
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
        let result = inner.blobs.put(result);
        let entry = Entry::Exited {
            seq: inner.state.next_seq(),
            agent,
            result,
        };
        match inner.commit(entry) {
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
                reason: BlobStore::digest(&mode.reason),
            };
            inner.state.check(&entry)?;
            // Phase two's targets, read before the freeze: every open request
            // of every agent in the subtree, and the driver holding it.
            let subtree = inner.state.subtree(agent);
            let abandons: Vec<(Arc<dyn Driver>, Corr)> = subtree
                .iter()
                .flat_map(|id| inner.state.open_corrs(*id))
                .filter_map(|corr| {
                    let driver = inner.in_flight.get(&corr)?;
                    Some((Arc::clone(inner.drivers.get(driver)?), corr))
                })
                .collect();
            inner.blobs.put(&mode.reason);
            inner.commit(entry)?;
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

    pub(crate) fn send(
        &self,
        from: AgentId,
        via: Capability,
        payload: &[u8],
    ) -> Result<Corr, KernelError> {
        let mut inner = self.lock();
        inner.ensure_ok()?;
        let corr = inner.state.next_corr();
        let msg = Msg::new(
            inner.state.next_seq(),
            Endpoint::Agent { id: from },
            MsgKind::Request,
            BlobStore::digest(payload),
        )
        .with_corr(corr);
        let entry = Entry::Sent { msg, via };
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
        inner.blobs.put(payload);
        inner.commit(entry)?;
        inner.in_flight.insert(corr, driver.clone());
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
        let msg = Msg::new(
            inner.state.next_seq(),
            Endpoint::Driver { id: driver.clone() },
            MsgKind::Reply,
            BlobStore::digest(payload),
        )
        .with_corr(corr)
        .with_consumption(consumed);
        let entry = Entry::Replied { msg, to };
        inner.state.check(&entry)?;
        inner.blobs.put(payload);
        inner.commit(entry)?;
        inner.in_flight.remove(&corr);
        inner.wake_agent(to);
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
            Some(delivery) => Poll::Ready(Some(delivery)),
            None => {
                inbox.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
