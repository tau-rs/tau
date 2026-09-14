//! Input estimation and output clamping: the driver's side of the ceiling
//! (ADR-0006 §7).

/// A conservative token estimate for `bytes` of prompt: bytes ÷ 3, then
/// × 1.2, rounded up — that is, ⌈bytes × 2 ⁄ 5⌉.
///
/// A byte count is not a token count; the margin is what absorbs tokenizer
/// drift, and it errs high on purpose. The exact path is the provider's
/// token-counting endpoint (#32), which costs a request but no tokens.
#[must_use]
pub fn tokens_for_bytes(bytes: usize) -> u64 {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    bytes.saturating_mul(2).div_ceil(5)
}

/// Clamps a request's `max_tokens` to the configured maximum.
///
/// Clamp rather than refuse: the ceiling is what the harness reserved, and a
/// request that asked for more room than that gets exactly the room there is.
/// The reply's `stop` says `max_tokens` if the clamp bit.
#[must_use]
pub fn clamp_max_tokens(requested: u32, maximum: u32) -> u32 {
    requested.min(maximum)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn estimate_rounds_up_and_is_conservative() {
        assert_eq!(tokens_for_bytes(0), 0);
        assert_eq!(tokens_for_bytes(1), 1);
        assert_eq!(tokens_for_bytes(5), 2);
        assert_eq!(tokens_for_bytes(3), 2);
        // 3 bytes per token, plus 20%: 3000 bytes → 1000 → 1200.
        assert_eq!(tokens_for_bytes(3_000), 1_200);
        // The reply-error fixture's number: 9210 tokens from 23025 bytes.
        assert_eq!(tokens_for_bytes(23_025), 9_210);
    }

    #[test]
    fn estimate_saturates_instead_of_overflowing() {
        assert_eq!(tokens_for_bytes(usize::MAX), u64::MAX.div_ceil(5));
    }

    #[test]
    fn clamp_only_ever_lowers() {
        assert_eq!(clamp_max_tokens(4_096, 1_024), 1_024);
        assert_eq!(clamp_max_tokens(512, 1_024), 512);
        assert_eq!(clamp_max_tokens(1_024, 1_024), 1_024);
    }
}
