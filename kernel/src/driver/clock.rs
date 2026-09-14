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
//! # The one place that reads a clock
//!
//! `clippy.toml` denies `Instant::now` and `SystemTime::now` workspace-wide so
//! that no clock reading can leak into the reducer. [`WallClock`] is the
//! exception, and it is exactly one function wide: [`read`], marked with the
//! one `allow` in the workspace. Everything it learns goes through the log
//! before anything acts on it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::kernel::{BoxFuture, Kernel, KernelError};

/// A clock that advances only when told to.
pub struct VirtualClock {
    kernel: Arc<Kernel>,
    now: Mutex<u64>,
}

impl VirtualClock {
    /// A clock reading zero, attached to `kernel`.
    #[must_use]
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self {
            kernel,
            now: Mutex::new(0),
        }
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
    /// aborts have happened.
    ///
    /// # Errors
    ///
    /// Whatever [`Kernel::tick`] returns; the reading is not advanced then.
    pub fn advance(&self, by: u64) -> Result<u64, KernelError> {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        let next = now.saturating_add(by);
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
}

impl WallClock {
    /// A clock attached to `kernel` that will tick every `period`. Reading
    /// zero is now.
    #[must_use]
    pub fn new(kernel: Arc<Kernel>, period: Duration) -> Self {
        Self {
            kernel,
            period,
            epoch: read(),
        }
    }

    /// Milliseconds since this clock was created.
    #[must_use]
    pub fn now(&self) -> u64 {
        u64::try_from(read().duration_since(self.epoch).as_millis()).unwrap_or(u64::MAX)
    }

    /// Publishes the current reading as one tick. Returns the reading.
    ///
    /// # Errors
    ///
    /// Whatever [`Kernel::tick`] returns.
    pub fn tick(&self) -> Result<u64, KernelError> {
        let now = self.now();
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
