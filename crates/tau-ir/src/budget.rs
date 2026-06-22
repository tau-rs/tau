//! Per-agent execution budget.
//!
//! The interpreter (β.2.4) and the v1 AOT lowering (β.7) both honor
//! this; exceeding any field surfaces as a `RuntimeError`. Fields are
//! optional so an agent can opt out (typical for development).

// schemars 0.8 derive generates code using bare `Box`/`String`/`vec!`
// from the std prelude — import it when the feature is active.
#[cfg(feature = "schema")]
#[allow(unused_imports)]
use std::prelude::rust_2021::*;

use serde::{Deserialize, Serialize};

/// Bounds on an agent's execution.
#[derive(Debug, Clone, Eq, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AgentBudget {
    /// Maximum number of turns the agent loop may take. `None` defers
    /// to the runtime default.
    pub max_turns: Option<u32>,
    /// Maximum tokens (input + output) the agent may consume across the
    /// entire run. `None` defers.
    pub max_tokens: Option<u64>,
}
