//! Clock port — abstracts wall-clock time.
//!
//! The kernel reads "now" only through this port; host shells supply
//! the concrete impl (`TokioClock` on tokio hosts, `EmbassyClock` on
//! MCU, etc.). Routing all `now()` calls through the port is what
//! makes `tau-runtime-core` portable to executors with no
//! `std::time::SystemTime`.

use core::sync::atomic::{AtomicI64, Ordering};

/// Wall-clock reading source.
///
/// Implementations return milliseconds since the Unix epoch. Negative
/// values are legal for pre-1970 timestamps. Resolution is
/// millisecond; sub-ms timing belongs in benchmarking, not in agent
/// semantics.
pub trait Clock: Send + Sync {
    /// Return the current instant as milliseconds since the Unix epoch.
    fn now(&self) -> i64;
}

/// Deterministic in-memory clock for tests. Each `now()` call returns
/// one millisecond after the previous one, starting from 1.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Default)]
pub struct MockClock {
    counter: AtomicI64,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl MockClock {
    /// Construct a `MockClock` with the cursor at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a `MockClock` with the cursor at the supplied instant.
    pub fn at(start_ms: i64) -> Self {
        Self {
            counter: AtomicI64::new(start_ms - 1),
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl Clock for MockClock {
    fn now(&self) -> i64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }
}
