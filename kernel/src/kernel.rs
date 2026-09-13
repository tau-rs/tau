//! The kernel proper: the log, the reducer, the blob store, and the queues
//! between them, behind one lock.
//!
//! # Executor-agnostic by construction
//!
//! Nothing here names an async runtime. Syscalls that wait (`recv`, a driver's
//! next delivery, the harness's drain) are plain futures over `std::task`
//! wakers, and new tasks are handed to a [`Spawner`] the harness supplies at
//! boot. ADR-0003 chooses tokio for the *body*; that choice lives in the
//! harness, and dependencies point inward.
//!
//! # Append-before-apply
//!
//! [`Inner::commit`] is the only path that mutates state, and it is check →
//! append → apply, in that order. A refused entry never reaches the log; an
//! entry that reached the log and then failed to apply is a kernel bug, and
//! the kernel records it as a [fault](KernelError::Faulted) rather than
//! carrying on with a log and a state that disagree.

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
use crate::reducer::{Refusal, State, StateHash};
use crate::syscall::{Exit, Handle, Program};

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// How the kernel hands a task to the harness's executor.
pub type Spawner = Arc<dyn Fn(BoxFuture<()>) + Send + Sync>;

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
}

/// The kernel. One per run; shared by the harness, every agent handle, and
/// every driver loop.
pub struct Kernel {
    inner: Mutex<Inner>,
}

impl Kernel {
    /// Boots a kernel over `log`, with `spawner` as the way to run tasks.
    pub fn boot<S>(log: Log, spawner: S) -> Arc<Self>
    where
        S: Fn(BoxFuture<()>) + Send + Sync + 'static,
    {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                log,
                state: State::initial(),
                blobs: BlobStore::new(),
                spawner: Arc::new(spawner),
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
        mut driver: D,
    ) -> Result<Capability, KernelError> {
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
            (cap, Arc::clone(&inner.spawner))
        };
        let kernel = Arc::clone(self);
        spawner(Box::pin(async move {
            while let Some(delivery) = kernel.next_delivery(id.clone()).await {
                let corr = delivery.corr;
                let (payload, consumed) = driver.handle(delivery).await;
                // A reply nobody can receive — the owner exited — is dead
                // letter by design (HANDOFF §4.9). Driver supervision (M3)
                // will surface the error envelope; M0 drops it on the floor
                // and the log shows exactly that: no `Replied` entry.
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

    /// Resolves when every agent has exited, or the kernel has faulted.
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

    /// Claims a stored exit result. The claim is a log entry.
    ///
    /// This is the harness's claim, for results the tree left behind. An
    /// agent claiming its child's result is `wait` (M1).
    ///
    /// # Errors
    ///
    /// [`Refusal::NoResult`] via [`KernelError::Refused`], or a log failure.
    pub fn claim(&self, agent: AgentId) -> Result<Vec<u8>, KernelError> {
        let mut inner = self.lock();
        if let Some(reason) = &inner.fault {
            return Err(KernelError::Faulted {
                reason: reason.clone(),
            });
        }
        let result = inner.state.result(agent).ok_or(Refusal::NoResult(agent))?;
        let entry = Entry::Claimed {
            seq: inner.state.next_seq(),
            agent,
        };
        inner.commit(entry)?;
        Ok(inner
            .blobs
            .get(&result)
            .map(<[u8]>::to_vec)
            .unwrap_or_default())
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
        spawner(Box::pin(async move {
            let Exit { .. } = program(handle).await;
        }));
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
        if let Err(err) = inner.commit(entry) {
            // `exit` cannot fail from the agent's side — the handle is consumed
            // and there is no one to return to. Anything that stops the exit
            // from being logged is a kernel fault, and the harness hears it.
            if inner.fault.is_none() {
                inner.fault = Some(format!("exit of {agent} could not be logged: {err}"));
            }
        }
        if inner.state.is_drained() || inner.fault.is_some() {
            if let Some(w) = inner.drain_waker.take() {
                w.wake();
            }
        }
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

    // ---------------------------------------------------------------- drivers

    /// A driver's reply to a request it was delivered.
    ///
    /// # Errors
    ///
    /// [`Refusal::UnknownCorr`] if the owner has exited (dead letter), via
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

/// Resolves when every agent has exited. See [`Kernel::drained`].
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
