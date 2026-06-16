//! Std-side lowering for the tau workflow IR.
//!
//! Holds the `lower` pass (`tau_pkg::ProjectConfig` → `tau_ir::IrModule`)
//! and the `LowerError` type. Split out of `tau-ir` (β.7.5) so the pure
//! IR crate and `tau-runtime-core` stay no_std and wasm-buildable.

extern crate alloc;

pub mod error;
pub mod lower;

pub use error::LowerError;
pub use lower::{lower_project, Caches, McpBuildError, ResolvedMcpContract, ResolvedServerTool};
