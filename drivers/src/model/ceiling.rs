//! The provider-neutral side of a model driver's ceiling (ADR-0006 §7):
//! deriving it from four configured numbers, estimating input, clamping
//! output, and pricing what the provider counted.
//!
//! Every model driver reserves the same shape — `tokens` and
//! `cost_microusd`, derived from an input bound, a maximum `max_tokens`, and
//! two prices — and every one bills the same way. Only the wire format is
//! the provider's.

use tau_kernel::abi::{Budget, Consumption, DimKey};
use tau_kernel::bridge::Usage;

use super::ConfigError;

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

/// The registration ceiling four numbers imply (ADR-0006 §7): the most one
/// call can cost when the driver enforces the bound and the clamp.
///
/// ```text
/// tokens        = input_bound + max_max_tokens
/// cost_microusd = input_bound × input_price + max_max_tokens × output_price
/// ```
///
/// At Claude Opus 5 list prices (5 and 25 µUSD per token), an input bound of
/// 8,000 and a maximum `max_tokens` of 1,024 give 9,024 `tokens` and 65,600
/// `cost_microusd`. `calls` is not included; the kernel adds it.
///
/// # Errors
///
/// [`ConfigError::CeilingOverflow`] if a sum or product exceeds `u64`.
pub fn derive_ceiling(
    input_bound: u64,
    max_max_tokens: u32,
    input_price_microusd: u64,
    output_price_microusd: u64,
) -> Result<Budget, ConfigError> {
    let max_out = u64::from(max_max_tokens);
    let tokens = input_bound
        .checked_add(max_out)
        .ok_or(ConfigError::CeilingOverflow {
            dim: DimKey::Tokens,
        })?;
    let cost = input_bound
        .checked_mul(input_price_microusd)
        .and_then(|input| {
            max_out
                .checked_mul(output_price_microusd)
                .and_then(|output| input.checked_add(output))
        })
        .ok_or(ConfigError::CeilingOverflow {
            dim: DimKey::CostMicroUsd,
        })?;
    Ok(Budget::from_dims([
        (DimKey::Tokens, tokens),
        (DimKey::CostMicroUsd, cost),
    ]))
}

/// What `usage` costs at the given prices: `tokens` = in + out,
/// `cost_microusd` = in × input price + out × output price. Saturates at
/// `u64::MAX`, which no real call reaches.
#[must_use]
pub fn price(usage: Usage, input_price_microusd: u64, output_price_microusd: u64) -> Consumption {
    let tokens = usage.input_tokens.saturating_add(usage.output_tokens);
    let cost = usage
        .input_tokens
        .saturating_mul(input_price_microusd)
        .saturating_add(usage.output_tokens.saturating_mul(output_price_microusd));
    Consumption::from_dims([(DimKey::Tokens, tokens), (DimKey::CostMicroUsd, cost)])
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

    #[test]
    fn the_worked_example_ceiling() {
        let ceiling = derive_ceiling(8_000, 1_024, 5, 25).unwrap();
        assert_eq!(ceiling.get(&DimKey::Tokens), Some(9_024));
        assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(65_600));
        assert_eq!(ceiling.get(&DimKey::Calls), None, "the kernel adds calls");
    }

    #[test]
    fn the_ceiling_refuses_to_overflow() {
        assert!(matches!(
            derive_ceiling(u64::MAX, 1_024, 5, 25),
            Err(ConfigError::CeilingOverflow {
                dim: DimKey::Tokens
            })
        ));
        assert!(matches!(
            derive_ceiling(u64::MAX >> 1, 1, 5, 25),
            Err(ConfigError::CeilingOverflow {
                dim: DimKey::CostMicroUsd
            })
        ));
    }

    #[test]
    fn pricing_is_in_plus_out_at_the_given_rates() {
        let consumed = price(
            Usage {
                input_tokens: 120,
                output_tokens: 34,
            },
            5,
            25,
        );
        assert_eq!(consumed.get(&DimKey::Tokens), Some(154));
        assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(600 + 850));
        let huge = price(
            Usage {
                input_tokens: u64::MAX,
                output_tokens: 1,
            },
            5,
            25,
        );
        assert_eq!(huge.get(&DimKey::Tokens), Some(u64::MAX));
        assert_eq!(huge.get(&DimKey::CostMicroUsd), Some(u64::MAX));
    }
}
