//! Model drivers: a `send` to a model capability carries a
//! [`ModelRequest`](tau_kernel::bridge::ModelRequest) and the reply is a
//! [`ModelReply`](tau_kernel::bridge::ModelReply), in JSON, per ADR-0006.
//!
//! Every driver here is registered behind a *ceiling* derived from its own
//! configuration (ADR-0006 §7), refuses what it cannot honour instead of
//! degrading silently, and reports its consumption honestly, error or not.

#[cfg(feature = "anthropic")]
pub mod anthropic;
