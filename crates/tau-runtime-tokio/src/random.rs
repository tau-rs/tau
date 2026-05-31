//! Tokio-shell `RandomSource` impl: OS entropy via [`getrandom`].

use tau_ports::RandomSource;

/// `RandomSource` backed by the operating-system entropy pool
/// (`getrandom`). Suitable for cryptographic use (UUID/ULID byte
/// minting in the core kernel). MCU shells supply their own
/// implementation backed by a hardware TRNG.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill(&self, dest: &mut [u8]) {
        getrandom::fill(dest).expect("OS entropy unavailable");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_with_distinct_bytes() {
        let r = OsRandom;
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        r.fill(&mut a);
        r.fill(&mut b);
        assert_ne!(a, b);
    }
}
