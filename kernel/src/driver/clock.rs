//! The virtual clock: time as a log entry, advanced on request.
//!
//! The reducer never reads a clock (ADR-0003). Time enters the system as
//! [`Tick`](crate::log::Entry::Tick) entries, and this is the thing that
//! appends them — when a test, or a harness driving a simulation, says so.
//! Nothing here consults `Instant` or `SystemTime`; the wall-clock source that
//! does, on a tokio interval, is M1b and will have exactly this shape with a
//! timer where [`VirtualClock::advance`] is.
//!
//! Units are whatever the clock source says they are. By convention a reading
//! is `wall_ms` ([`DimKey::WallMs`](crate::abi::DimKey::WallMs)), so a cancel
//! grace of `500` means half a second under a wall clock; a test may count in
//! anything it likes.

use std::sync::{Arc, Mutex, PoisonError};

use crate::kernel::{Kernel, KernelError};

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
    /// Cancel deadlines the reading reaches are enforced inside the kernel's
    /// apply of the tick: by the time this returns, the aborts have happened.
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
