//! How this driver sizes its input (ADR-0006 §7). The arithmetic itself —
//! the bytes estimate, the clamp, the ceiling — is provider-neutral and
//! lives in [`crate::model::ceiling`]; what is Anthropic's is the choice of
//! asking `count_tokens` first.

/// How the driver sizes a prompt against `input_bound` (ADR-0006 §7): with
/// a margin, locally, or exactly, by asking the provider first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputEstimate {
    /// [`tokens_for_bytes`](crate::model::ceiling::tokens_for_bytes) over the
    /// serialized provider body: ⌈bytes × 2 ⁄ 5⌉, conservative, no extra
    /// request. The default.
    #[default]
    Bytes,
    /// `POST /v1/messages/count_tokens` before the call, and the bound is
    /// checked against the number the provider answers. One more request
    /// per call, on its own rate limit, at no token cost; it is not
    /// reported in the call's consumption.
    CountTokens,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_estimate_is_bytes() {
        assert_eq!(InputEstimate::default(), InputEstimate::Bytes);
    }
}
