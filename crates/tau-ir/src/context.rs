//! Per-agent context-management configuration.
//!
//! v0 surface: the field exists on [`crate::Agent`] but is `None` for
//! every workflow — β.4 owns the actual pipeline shape. v0 reserves
//! the slot so adding β.4's struct later is a `MINOR` `ir_format`
//! bump (additive optional field), not a `MAJOR` one.

use serde::{Deserialize, Serialize};

/// Placeholder for β.4's context-manager configuration.
///
/// v0 keeps the struct empty and `#[non_exhaustive]` so β.4 can add
/// fields additively without forcing every existing IR module to
/// re-emit.
#[non_exhaustive]
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {}
