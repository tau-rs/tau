//! The workflow IR baked at build time (see `build.rs`). Empty when no
//! `TAU_IR_BYTES` was supplied — the guest `run` then returns its error arm.

/// Canonical IR bytes baked into the component.
pub static BAKED_IR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/baked_ir.bin"));
