//! The syscall boundary, as an agent sees it.
//!
//! A [`Handle`] is an agent's whole world. It offers the seven syscalls and
//! nothing else — M0 ships four of them: `spawn`, `exit`, `send`, `recv`.
//! `wait` and `cancel` are M1, `attach` is M2 and harness-privileged.
//!
//! # `exit` is a type, not a convention
//!
//! A [`Program`] must return an [`Exit`], and the only way to make one is
//! [`Handle::exit`], which consumes the handle. "This is my last act" is
//! therefore checked by the compiler: no syscall can follow it, and no
//! program can finish without it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::abi::{AgentId, BlobRef, Budget, Capability, Corr, Endpoint, Msg, MsgKind, Namespace};
use crate::kernel::{BoxFuture, Kernel, KernelError};

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
    /// enough budget — and the parent's state unchanged.
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

    /// Syscall 5. Sends `payload` to the endpoint `cap` names. Never blocks.
    ///
    /// # Errors
    ///
    /// [`KernelError::WouldBlock`] if the target's inbox is full — handle it,
    /// the kernel will not queue on your behalf. [`KernelError::Refused`] if
    /// this agent does not hold `cap`, or has exhausted its tokens.
    pub fn send(&self, cap: Capability, payload: &[u8]) -> Result<Corr, KernelError> {
        self.kernel.send(self.id, cap, payload)
    }

    /// Syscall 6. The next message in this agent's mailbox that satisfies
    /// `filter`. Which message matched is itself a log entry.
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
