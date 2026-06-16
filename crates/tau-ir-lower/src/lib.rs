//! Std-side lowering for the tau workflow IR.
//!
//! Holds the `lower` pass (`tau_pkg::ProjectConfig` → `tau_ir::IrModule`)
//! and the `LowerError` type. Split out of `tau-ir` (β.7.5) so the pure
//! IR crate and `tau-runtime-core` stay no_std and wasm-buildable.
