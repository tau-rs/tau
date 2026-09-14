//! The syscall boundary, as an agent sees it.
//!
//! A [`Handle`] is an agent's whole world. It offers the seven syscalls and
//! nothing else. Six are here: `spawn`, `exit`, `wait`, `cancel`, `send`,
//! `recv`. `attach` is M2 and harness-privileged.
//!
//! # `exit` is a type, not a convention
//!
//! A [`Program`] must return an [`Exit`], and the only way to make one is
//! [`Handle::exit`], which consumes the handle. "This is my last act" is
//! therefore checked by the compiler: no syscall can follow it, and no
//! program can finish without it.
//!
//! # Selectors are not ABI
//!
//! [`Match`] and [`WaitFor`] are the arguments to `recv` and `wait`. They never
//! reach the wire: the log records which message a `recv` resolved to and
//! which outcome a `wait` claimed, not the filter that chose it. They are Rust
//! API, versioned with the crate (ADR-0002, "Selectors").

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::abi::{
    AgentId, BlobRef, Budget, Capability, Corr, DriverId, Endpoint, Msg, MsgKind, Namespace,
};
use crate::driver::ToolSchema;
use crate::kernel::{BoxFuture, Kernel, KernelError};
pub use crate::reducer::Outcome;

/// Proof that an agent exited. Only [`Handle::exit`] produces one.
#[must_use = "an agent's program must return the Exit it was given"]
pub struct Exit {
    pub(crate) _sealed: (),
}

/// An agent's code: a function of its handle, ending in an [`Exit`].
pub type Program = Box<dyn FnOnce(Handle) -> BoxFuture<Exit> + Send>;

/// Boxes an async closure into a [`Program`].
pub fn program<F, Fut>(f: F) -> Program
where
    F: FnOnce(Handle) -> Fut + Send + 'static,
    Fut: Future<Output = Exit> + Send + 'static,
{
    Box::new(move |handle| Box::pin(f(handle)))
}

/// The `recv` filter language. Closed, on purpose (ADR-0002).
///
/// An open predicate would let userspace push arbitrary computation into the
/// kernel's delivery path and make "which message matched" depend on user code
/// at replay time. These five forms are all there is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match {
    /// Any message at all.
    Any,
    /// A message on this correlation.
    Corr(Corr),
    /// A message from this endpoint.
    Sender(Endpoint),
    /// A message of this kind.
    Kind(MsgKind),
    /// A message satisfying any of these.
    Or(Vec<Match>),
}

impl Match {
    /// Whether `msg` satisfies this filter.
    #[must_use]
    pub fn matches(&self, msg: &Msg) -> bool {
        match self {
            Self::Any => true,
            Self::Corr(corr) => msg.corr == Some(*corr),
            Self::Sender(from) => msg.from == *from,
            Self::Kind(kind) => msg.kind == *kind,
            Self::Or(alternatives) => alternatives.iter().any(|m| m.matches(msg)),
        }
    }
}

/// What a `wait` is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitFor {
    /// One particular child.
    Child(AgentId),
    /// Whichever child finishes first — in completion order, so a child that
    /// finished before the parent asked is returned first, not lost.
    Any,
}

/// How a `cancel` proceeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelMode {
    /// How many clock units the subtree has to exit on its own before the
    /// kernel hard-aborts it. Zero aborts immediately. Nothing in userspace
    /// can extend this once set.
    pub grace: u64,
    /// The payload of the `Notice` each cancelled agent receives. The kernel
    /// does not read it.
    pub reason: Vec<u8>,
}

impl CancelMode {
    /// A grace period with no stated reason.
    #[must_use]
    pub fn grace(grace: u64) -> Self {
        Self {
            grace,
            reason: Vec::new(),
        }
    }

    /// No grace: a hard abort, now.
    #[must_use]
    pub fn immediate() -> Self {
        Self::grace(0)
    }

    /// Attaches a reason.
    #[must_use]
    pub fn with_reason(mut self, reason: &[u8]) -> Self {
        self.reason = reason.to_vec();
        self
    }
}

/// What `wait` returns: which child finished, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitResult {
    /// The child.
    pub agent: AgentId,
    /// How it finished.
    pub outcome: Outcome,
}

impl ExitResult {
    /// The result blob, if the child exited rather than being aborted.
    #[must_use]
    pub fn result(&self) -> Option<BlobRef> {
        match self.outcome {
            Outcome::Exited(blob) => Some(blob),
            Outcome::Aborted => None,
        }
    }
}

/// An agent's kernel handle: the seven syscalls, and no other power.
pub struct Handle {
    kernel: Arc<Kernel>,
    id: AgentId,
}

impl Handle {
    pub(crate) fn new(kernel: Arc<Kernel>, id: AgentId) -> Self {
        Self { kernel, id }
    }

    /// This agent's id.
    #[must_use]
    pub fn id(&self) -> AgentId {
        self.id
    }

    /// Syscall 1. Creates a child with a namespace ⊆ this one's and a budget
    /// carved atomically from this one's. Returns once the spawn is logged;
    /// the child runs concurrently.
    ///
    /// # Errors
    ///
    /// [`KernelError::Refused`] with the reducer's reason — not a subset, not
    /// enough budget, this agent is cancelled — and the parent's state
    /// unchanged.
    pub fn spawn(
        &self,
        program: Program,
        ns: Namespace,
        budget: Budget,
    ) -> Result<AgentId, KernelError> {
        self.kernel.spawn(Some(self.id), program, ns, budget)
    }

    /// Syscall 2. The last act: stores `result` until claimed, returns unspent
    /// budget to the parent, and consumes the handle.
    pub fn exit(self, result: &[u8]) -> Exit {
        self.kernel.exit(self.id, result);
        Exit { _sealed: () }
    }

    /// Syscall 3. The outcome of a child. Level-triggered: a child that
    /// finished before this was called is returned at once, and an outcome
    /// persists until it is claimed, which is what this does — the claim is a
    /// log entry.
    ///
    /// Resolves to an error rather than pending forever when there is nothing
    /// to wait for: [`WaitFor::Child`] of an agent that is not this one's
    /// child, or [`WaitFor::Any`] with no live and no unclaimed children.
    pub fn wait(&self, what: WaitFor) -> Wait {
        Wait {
            kernel: Arc::clone(&self.kernel),
            agent: self.id,
            what,
        }
    }

    /// Syscall 4. Cancels a descendant and everything below it, in two
    /// phases (ADR-0002): this call is the atomic freeze — every live agent in
    /// the subtree may no longer `spawn`, `send`, or `cancel`, receives a
    /// `Notice` carrying the reason, and has its in-flight requests abandoned
    /// at the drivers. Then the kernel enforces the grace deadline as clock
    /// ticks are applied, and hard-aborts whatever is still running.
    ///
    /// Returns once the freeze is logged. Follow it with a `wait` for the
    /// outcome.
    ///
    /// # Errors
    ///
    /// [`KernelError::Refused`] if `id` is not a live descendant of this
    /// agent, is already cancelled, or this agent is itself cancelled.
    pub fn cancel(&self, id: AgentId, mode: CancelMode) -> Result<(), KernelError> {
        self.kernel.cancel(Some(self.id), id, &mode)
    }

    /// Syscall 5. Sends `payload` to the endpoint `cap` names. Never blocks.
    ///
    /// # Errors
    ///
    /// [`KernelError::WouldBlock`] if the target's inbox is full — handle it,
    /// the kernel will not queue on your behalf. [`KernelError::Refused`] if
    /// this agent does not hold `cap`, cannot reserve the driver's declared
    /// ceiling plus one `calls` from its budget, or is cancelled. The
    /// reservation happens *before* delivery and is settled by the reply.
    pub fn send(&self, cap: Capability, payload: &[u8]) -> Result<Corr, KernelError> {
        self.kernel.send(self.id, cap, payload)
    }

    /// Syscall 6. The next message in this agent's mailbox that satisfies
    /// `filter`. Which message matched is itself a log entry.
    ///
    /// A cancelled agent's notice is a [`MsgKind::Notice`] from the canceller;
    /// a program that wants to see one while waiting on a reply asks for
    /// `Match::Or(vec![Match::Corr(corr), Match::Kind(MsgKind::Notice)])`.
    pub fn recv(&self, filter: Match) -> Recv {
        Recv {
            kernel: Arc::clone(&self.kernel),
            agent: self.id,
            filter,
        }
    }

    /// The bytes behind a payload reference. Not a syscall: a blob is
    /// immutable content-addressed data, and reading it has no effect to log.
    #[must_use]
    pub fn read(&self, blob: BlobRef) -> Option<Vec<u8>> {
        self.kernel.read(blob)
    }

    /// What the driver behind `cap` offers as a tool, and the name the
    /// harness gave it. Not a syscall: like [`Handle::read`], a query with
    /// no effect to log — the projected tool list ends up inside a request
    /// payload the log already references by hash (ADR-0006 §5).
    ///
    /// `None` if the driver is not a tool (a model driver, say).
    ///
    /// # Errors
    ///
    /// [`KernelError::Refused`] with `NotHeld` if this agent does not hold
    /// `cap`, or `Unroutable` if `cap` does not name a driver. Authority
    /// stays in the kernel: a child projects its own namespace, never a copy
    /// of its parent's table.
    pub fn describe(&self, cap: Capability) -> Result<Option<(DriverId, ToolSchema)>, KernelError> {
        self.kernel.describe(self.id, cap)
    }
}

/// A pending `recv`. See [`Handle::recv`].
pub struct Recv {
    kernel: Arc<Kernel>,
    agent: AgentId,
    filter: Match,
}

impl Future for Recv {
    type Output = Result<Msg, KernelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.kernel.poll_recv(self.agent, &self.filter, cx)
    }
}

/// A pending `wait`. See [`Handle::wait`].
pub struct Wait {
    kernel: Arc<Kernel>,
    agent: AgentId,
    what: WaitFor,
}

impl Future for Wait {
    type Output = Result<ExitResult, KernelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.kernel.poll_wait(self.agent, self.what, cx)
    }
}
