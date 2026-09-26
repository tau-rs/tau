//! Clocks: time as a log entry, from a test's hand or from the wall.
//!
//! The reducer never reads a clock (ADR-0003). Time enters the system as
//! [`Tick`](crate::log::Entry::Tick) entries, and the two things here are the
//! only things that append them: [`VirtualClock`], when a test or a harness
//! driving a simulation says so, and [`WallClock`], on an interval, from real
//! time. They have the same shape — a reading, published through
//! [`Kernel::tick`] — with a timer where [`VirtualClock::advance`] is.
//!
//! Units are whatever the clock source says they are. By convention a reading
//! is `wall_ms` ([`DimKey::WallMs`](crate::abi::DimKey::WallMs)), which is
//! what [`WallClock`] publishes; a test may count in anything it likes.
//!
//! # Wall-exhaustion grace (ADR-0015)
//!
//! The tick that empties a timed agent's wall grant aborts it inside the
//! same apply, with no warning. A clock source knows the next reading before
//! it publishes it, so it can warn: with a [`Grace`] set, before each tick
//! it cancels — through the harness's ordinary
//! [`Kernel::cancel_from_harness`] — every live, timed agent whose remaining
//! wall would be at most the grace after that tick, with a grace equal to
//! the agent's remaining wall as of the last published reading. The reducer
//! sets the deadline as `now + grace`, so the deadline is exactly the reading
//! that would have aborted the agent anyway: **the notice moves earlier, the
//! deadline never moves.** The `Cancelled` entry precedes the `Tick` in the
//! log. A cancel is a subtree freeze, so a parent entering its window takes
//! its live children with it. This is policy over the existing surface, not
//! a kernel rule: the reducer, the ABI and every pinned hash are untouched.
//!
//! The default grace is zero, which is no policy at all: a harness that does
//! not opt in sees the hard path exactly.
//!
//! # The one place that reads a clock
//!
//! `clippy.toml` denies `Instant::now` and `SystemTime::now` workspace-wide so
//! that no clock reading can leak into the reducer. [`WallClock`] is the
//! exception, and it is exactly one function wide: [`read`], marked with the
//! one `allow` in the workspace. Everything it learns goes through the log
//! before anything acts on it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::abi::{AgentId, DimKey};
use crate::kernel::{BoxFuture, Kernel, KernelError};
use crate::reducer::{Agent, Status};
use crate::syscall::CancelMode;

/// A per-agent lead time: how many clock units before exhaustion an agent is
/// warned, or `None` to leave it on the hard path. See [`Grace::policy`].
pub type GracePolicy = Arc<dyn Fn(AgentId, &Agent) -> Option<u64> + Send + Sync>;

#[derive(Clone)]
enum Lead {
    Flat(u64),
    Policy(GracePolicy),
}

/// How far ahead of wall exhaustion a clock source warns an agent, and
/// what the warning says (ADR-0015 §2).
///
/// The lead time is in the clock's own units. Zero — the default, and what
/// a policy answering `None` or `Some(0)` means for one agent — is no
/// warning: the agent is aborted at exhaustion with no notice, exactly as
/// without a grace. The notice can arrive up to one clock period earlier
/// than the lead time, never later; a harness that wants a tighter window
/// ticks faster.
#[derive(Clone)]
pub struct Grace {
    lead: Lead,
    reason: Vec<u8>,
}

impl Default for Grace {
    fn default() -> Self {
        Self::flat(0)
    }
}

impl Grace {
    /// The payload of the notice unless [`reason`](Self::reason) says
    /// otherwise: the name of the dimension that is about to run out.
    pub const DEFAULT_REASON: &'static [u8] = b"wall_ms";

    /// The same lead time for every timed agent.
    #[must_use]
    pub fn flat(units: u64) -> Self {
        Self {
            lead: Lead::Flat(units),
            reason: Self::DEFAULT_REASON.to_vec(),
        }
    }

    /// A lead time chosen per agent from its record — its grant, its parent,
    /// its status. `None` leaves that agent on the hard path, which is how a
    /// harness that wants today's orphaning (ADR-0015 §4) says so.
    #[must_use]
    pub fn policy<F>(policy: F) -> Self
    where
        F: Fn(AgentId, &Agent) -> Option<u64> + Send + Sync + 'static,
    {
        Self {
            lead: Lead::Policy(Arc::new(policy)),
            reason: Self::DEFAULT_REASON.to_vec(),
        }
    }

    /// The payload every warning carries. The kernel does not read it; a
    /// program may use it to tell a wall warning from another cancel.
    #[must_use]
    pub fn reason(mut self, reason: &[u8]) -> Self {
        self.reason = reason.to_vec();
        self
    }

    fn is_off(&self) -> bool {
        matches!(self.lead, Lead::Flat(0))
    }

    fn lead(&self, id: AgentId, agent: &Agent) -> Option<u64> {
        match &self.lead {
            Lead::Flat(units) => Some(*units),
            Lead::Policy(policy) => policy(id, agent),
        }
    }

    /// The sweep: before `next` is published, cancel every agent the
    /// policy says to, with its remaining wall as the grace, so the
    /// deadline lands on its exhaustion reading. Parents before children:
    /// a parent's cancel freezes its subtree, and a child whose ancestor is
    /// being cancelled on this sweep is left to that freeze rather than
    /// given a cancel of its own. Anything the kernel refuses — an agent
    /// that exited or was cancelled between the read and the write — is
    /// what the sweep would have skipped, and is skipped.
    ///
    /// # Errors
    ///
    /// A faulted kernel or a log failure from a cancel.
    fn warn(&self, kernel: &Kernel, next: u64) -> Result<(), KernelError> {
        if self.is_off() {
            return Ok(());
        }
        let state = kernel.state();
        let elapsed = next.saturating_sub(state.now());
        // Candidates in id order, which is spawn order: a parent's id is
        // always below its children's.
        let due: BTreeMap<AgentId, u64> = state
            .agents()
            .filter(|(_, a)| a.status == Status::Live)
            .filter_map(|(id, a)| {
                let remaining = a.budget.get(&DimKey::WallMs)?;
                let lead = self.lead(id, a)?;
                (lead > 0 && remaining.saturating_sub(elapsed) <= lead).then_some((id, remaining))
            })
            .collect();
        for (&id, &remaining) in &due {
            let mut ancestor = state.agent(id).and_then(|a| a.parent);
            let covered = std::iter::from_fn(|| {
                let up = ancestor?;
                ancestor = state.agent(up).and_then(|a| a.parent);
                Some(up)
            })
            .any(|up| due.contains_key(&up));
            if covered {
                continue;
            }
            let mode = CancelMode {
                grace: remaining,
                reason: self.reason.clone(),
            };
            match kernel.cancel_from_harness(id, &mode) {
                Ok(()) | Err(KernelError::Refused(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// A clock that advances only when told to.
pub struct VirtualClock {
    kernel: Arc<Kernel>,
    now: Mutex<u64>,
    grace: Grace,
}

impl VirtualClock {
    /// A clock reading zero, attached to `kernel`, with no grace.
    #[must_use]
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self {
            kernel,
            now: Mutex::new(0),
            grace: Grace::default(),
        }
    }

    /// The same clock with a wall-exhaustion grace (ADR-0015).
    #[must_use]
    pub fn with_grace(mut self, grace: Grace) -> Self {
        self.grace = grace;
        self
    }

    /// The last reading this clock published.
    #[must_use]
    pub fn now(&self) -> u64 {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Advances the clock by `by` and publishes the new reading as a tick.
    /// Returns the reading.
    ///
    /// Wall budgets and cancel deadlines the reading reaches are enforced
    /// inside the kernel's apply of the tick: by the time this returns, the
    /// aborts have happened. With a [`Grace`] set, the agents the reading
    /// would bring within their lead time are cancelled first, before the
    /// tick is published.
    ///
    /// # Errors
    ///
    /// Whatever [`Kernel::tick`] or a grace cancel returns; the reading is
    /// not advanced then.
    pub fn advance(&self, by: u64) -> Result<u64, KernelError> {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        let next = now.saturating_add(by);
        self.grace.warn(&self.kernel, next)?;
        self.kernel.tick(next)?;
        *now = next;
        Ok(next)
    }
}

/// How the wall clock waits between ticks: a function the harness supplies,
/// so the kernel names no executor. With tokio:
/// `Arc::new(|d| Box::pin(tokio::time::sleep(d)))`.
pub type Sleep = Arc<dyn Fn(Duration) -> BoxFuture<()> + Send + Sync>;

/// The one place in the workspace that reads a clock.
///
/// The lint that forbids this everywhere else exists so the reducer can never
/// depend on one (ADR-0003). This function is not the reducer: what it reads
/// becomes a `Tick` entry, and only the entry has any effect. Keeping the
/// call in a function of its own keeps the exception one line wide and
/// greppable.
#[allow(clippy::disallowed_methods)]
fn read() -> Instant {
    Instant::now()
}

/// A clock that publishes real elapsed time, in milliseconds, on an interval.
///
/// Readings are milliseconds since the clock was created — monotonic, so
/// [`Refusal::ClockRewound`](crate::reducer::Refusal::ClockRewound) cannot
/// happen — and every reading becomes a `Tick` through [`Kernel::tick`],
/// which takes and releases the kernel lock inside the call. The loop holds
/// nothing across its await.
pub struct WallClock {
    kernel: Arc<Kernel>,
    period: Duration,
    epoch: Instant,
    grace: Grace,
}

impl WallClock {
    /// A clock attached to `kernel` that will tick every `period`, with no
    /// grace. Reading zero is now.
    #[must_use]
    pub fn new(kernel: Arc<Kernel>, period: Duration) -> Self {
        Self {
            kernel,
            period,
            epoch: read(),
            grace: Grace::default(),
        }
    }

    /// The same clock with a wall-exhaustion grace (ADR-0015), in
    /// milliseconds.
    #[must_use]
    pub fn with_grace(mut self, grace: Grace) -> Self {
        self.grace = grace;
        self
    }

    /// Milliseconds since this clock was created.
    #[must_use]
    pub fn now(&self) -> u64 {
        u64::try_from(read().duration_since(self.epoch).as_millis()).unwrap_or(u64::MAX)
    }

    /// Publishes the current reading as one tick. Returns the reading. With
    /// a [`Grace`] set, the agents the reading would bring within their
    /// lead time are cancelled first.
    ///
    /// # Errors
    ///
    /// Whatever [`Kernel::tick`] or a grace cancel returns.
    pub fn tick(&self) -> Result<u64, KernelError> {
        let now = self.now();
        self.grace.warn(&self.kernel, now)?;
        self.kernel.tick(now)?;
        Ok(now)
    }

    /// Ticks every period until the kernel stops accepting ticks — shut down,
    /// faulted, or a log write failed. Hand the future to the harness's
    /// executor; it is not an agent, and nothing cancels it but the kernel.
    pub fn run(self: Arc<Self>, sleep: Sleep) -> BoxFuture<()> {
        Box::pin(async move {
            loop {
                sleep(self.period).await;
                if self.tick().is_err() {
                    break;
                }
            }
        })
    }
}
