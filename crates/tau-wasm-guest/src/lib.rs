//! β.7.5 wasm component guest.
//!
//! Built only for `wasm32-wasip2`, where it is a WASI 0.2 component that
//! imports the three host ports (`complete` / `now-millis` / `next-u64`)
//! and exports `run`. On every other target this crate is intentionally
//! empty, so `cargo check --workspace` and host test builds stay green
//! without the no_std component machinery (custom allocator, panic
//! handler, wasm import thunks) leaking into the host link.
#![cfg_attr(target_arch = "wasm32", no_std)]
// Opt out of the workspace `unsafe_code = "warn"` lint: the wasm32 guest needs
// `unsafe` for its custom allocator, panic handler, and WIT import thunks.
#![allow(unsafe_code)]

#[cfg(target_arch = "wasm32")]
mod baked;
#[cfg(target_arch = "wasm32")]
mod guest;
/// Re-export the WIT-generated host imports so sibling modules can reference
/// them at `crate::wit_host::complete` etc. without needing to know the
/// exact wit-bindgen generated path inside `guest.rs`.
#[cfg(target_arch = "wasm32")]
pub(crate) use guest::wit_host;
#[cfg(all(
    target_arch = "wasm32",
    any(tau_cap_net_http, tau_cap_fs_read, tau_cap_fs_write)
))]
pub(crate) use guest::wit_wasi;
#[cfg(target_arch = "wasm32")]
mod dispatcher;
#[cfg(target_arch = "wasm32")]
mod executor;
#[cfg(target_arch = "wasm32")]
// #689: absent entirely when the baked IR reaches no goal predicate, so
// `tau_native_tools::goal_predicates` (and therefore `regex-automata`) is
// unreferenced and wasm-ld collects it.
#[cfg(tau_goal_predicates)]
mod goal_registry;
#[cfg(target_arch = "wasm32")]
mod host_ports;
/// Pure preopen selection (longest-prefix, root-aware) shared by the fs
/// effect arms. Compiled whenever an fs cfg is on — and in host `cfg(test)`
/// builds, where its table-driven tests run natively (the rest of this crate
/// is wasm32-only, so this is the crate's only native test surface).
#[cfg(any(test, tau_cap_fs_read, tau_cap_fs_write))]
mod preopen;
