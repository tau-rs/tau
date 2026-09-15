//! Drivers for the tau kernel: the border guards between envelopes and the
//! world (HANDOFF §6).
//!
//! A driver is the only thing that touches anything outside the kernel. This
//! crate holds the ones that need real dependencies — an HTTP client, a
//! runtime — which the kernel deliberately does not carry: the kernel is
//! executor-agnostic and never parses a payload, so a real model driver
//! cannot live there.
//!
//! One module per driver family, feature-gated:
//!
//! - [`model`] — model drivers speaking the bridge contract of ADR-0006.
//!   [`model::anthropic`] (feature `anthropic`) speaks the Messages API;
//!   [`model::openai`] (feature `openai`) speaks chat completions, which is
//!   OpenAI, vLLM, and everything else that copies the shape. Both are on
//!   by default.

pub mod model;
