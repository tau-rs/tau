//! Tool-args schema validation at the kernel boundary.
//!
//! The implementation lives in `tau_runtime_core::tool_args`; this
//! module re-exports the public items for use within tau-runtime.

#[allow(unused_imports)]
pub(crate) use tau_runtime_core::tool_args::{validate_tool_args, ToolArgsValidator};
