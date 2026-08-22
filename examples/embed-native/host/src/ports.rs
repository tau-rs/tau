//! Real host ports: wall-clock via std, entropy via a time-seeded PRNG.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tau_ports::{Clock, RandomSource};

/// Wall-clock port backed by `std::time`.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Non-cryptographic entropy port (time-seeded xorshift64*). A real
/// product wraps `getrandom`/OS entropy here; this example stays
/// dependency-free and its asserted outcome does not depend on entropy
/// quality.
pub struct HostRandom {
    state: AtomicU64,
}

impl HostRandom {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1; // xorshift needs a non-zero seed
        Self {
            state: AtomicU64::new(seed),
        }
    }
}

impl Default for HostRandom {
    fn default() -> Self {
        Self::new()
    }
}

impl HostRandom {
    /// Draw the next xorshift64* value via a compare-exchange retry loop
    /// so concurrent callers (this crate pulls tokio `rt-multi-thread`,
    /// and `RandomSource` is `Send + Sync`) each observe a distinct state
    /// transition instead of racing to load/store the same value.
    fn next_u64(&self) -> u64 {
        let mut s = self.state.load(Ordering::Relaxed);
        loop {
            let mut x = s;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            match self
                .state
                .compare_exchange_weak(s, x, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return x,
                Err(actual) => s = actual,
            }
        }
    }
}

impl RandomSource for HostRandom {
    fn fill(&self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let x = self.next_u64();
            for (d, b) in chunk.iter_mut().zip(x.to_le_bytes()) {
                *d = b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::{Clock, RandomSource};

    #[test]
    fn system_clock_is_positive() {
        assert!(SystemClock.now() > 0);
    }

    #[test]
    fn host_random_fills_bytes() {
        let r = HostRandom::new();
        let mut buf = [0u8; 16];
        r.fill(&mut buf);
        assert!(buf.iter().any(|&b| b != 0), "should write entropy");
    }
}
