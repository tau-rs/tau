//! Tokio-shell `Clock` impl: wall-clock UTC via [`chrono::Utc::now`].

use tau_ports::Clock;

/// Wall-clock implementation backed by `chrono::Utc::now`. Resolution is
/// milliseconds since the Unix epoch — matching the [`Clock`] trait
/// contract. Suitable for the tokio host shell; MCU shells supply their
/// own [`Clock`] impl.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_monotonic_ish() {
        let c = TokioClock;
        let a = c.now();
        let b = c.now();
        assert!(b >= a);
    }
}
