//! β.7.5 wasm component guest.
//!
//! Built only for `wasm32-wasip2`, where it is a WASI 0.2 component that
//! imports the three host ports (`complete` / `now-millis` / `next-u64`)
//! and exports `run`. On every other target this crate is intentionally
//! empty, so `cargo check --workspace` and host test builds stay green
//! without the no_std component machinery (custom allocator, panic
//! handler, wasm import thunks) leaking into the host link.
#![cfg_attr(target_arch = "wasm32", no_std)]

#[cfg(target_arch = "wasm32")]
mod baked;
#[cfg(target_arch = "wasm32")]
mod guest;
