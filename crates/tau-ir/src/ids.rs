//! Strongly-typed identifiers for IR entities.
//!
//! Each id is a newtype around `alloc::string::String`. The names are
//! ASCII (TOML key shape: letters, digits, `_`, `-`); validation is the
//! lowering pass's responsibility, not the type's.

use alloc::string::String;
use serde::{Deserialize, Serialize};

/// Identifier for an [`crate::Agent`] node within a [`crate::Workflow`].
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AgentId(pub String);

/// Identifier for a [`crate::Tool`] node within a [`crate::Workflow`].
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolId(pub String);

/// Identifier for a [`crate::Deterministic`] step within a [`crate::Workflow`].
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StepId(pub String);

/// Identifier for a [`crate::Subflow`] edge within a [`crate::Workflow`].
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SubflowId(pub String);

/// Identifier for a [`crate::pipeline::PipelineStep`] within a
/// [`crate::Workflow`]'s pipeline. Addressable as `steps.<id>.output`.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PipelineStepId(pub String);

/// Identifier for a postcondition [`Check`](crate::check::Check).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CheckId(pub String);
