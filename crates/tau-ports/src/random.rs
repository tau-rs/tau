//! RandomSource port — abstracts entropy.
//!
//! The kernel mints UUID/ULID bytes only through this port; host shells
//! supply the concrete impl (`OsRandom` on std hosts, `HwRandom` on MCU).
//! Routing entropy through a port is what makes `tau-runtime-core`
//! runnable on bare-metal targets with no `getrandom`.

#[cfg(any(test, feature = "test-fixtures"))]
use core::sync::atomic::{AtomicU64, Ordering};

/// Source of cryptographic-grade random bytes.
///
/// Implementations must produce uniformly distributed bytes. The MCU
/// host wraps a hardware TRNG; the tokio host wraps `getrandom`. The
/// deterministic test fixture is xorshift-seeded and is NOT suitable
/// for cryptographic use.
pub trait RandomSource: Send + Sync {
    /// Fill `dest` with random bytes.
    fn fill(&self, dest: &mut [u8]);
}

/// Seeded, deterministic PRNG for tests. xorshift64*; NOT cryptographic.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug)]
pub struct DeterministicRandom {
    state: AtomicU64,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl DeterministicRandom {
    /// Construct a `DeterministicRandom` from a 64-bit seed.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "test-fixtures")]
    /// # {
    /// use tau_ports::DeterministicRandom;
    /// let rng = DeterministicRandom::seeded(42);
    /// # }
    /// ```
    pub fn seeded(seed: u64) -> Self {
        // xorshift64* requires non-zero seed; substitute a canonical
        // value for the zero case rather than panic.
        let s = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed };
        Self {
            state: AtomicU64::new(s),
        }
    }

    fn next_u64(&self) -> u64 {
        // Compare-and-swap loop so concurrent fills (each treated as
        // its own draw) don't lose entropy. Practically the fixture is
        // single-task, but the atomic shape lets it be Sync without
        // unsafe.
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
                Ok(_) => return x.wrapping_mul(0x2545F4914F6CDD1D),
                Err(actual) => s = actual,
            }
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl RandomSource for DeterministicRandom {
    fn fill(&self, dest: &mut [u8]) {
        let mut i = 0;
        while i < dest.len() {
            let chunk = self.next_u64().to_le_bytes();
            let take = core::cmp::min(8, dest.len() - i);
            dest[i..i + take].copy_from_slice(&chunk[..take]);
            i += take;
        }
    }
}
