//! Re-export of `tau_runtime_core::stream` for legacy import paths.
//!
//! Pre-β.1.3.5c, the streaming pump lived in this module. It now lives
//! in `tau-runtime-core::stream` so the kernel agent loop is reachable
//! from `no_std + alloc` host shells. This module preserves the public
//! re-export surface so existing call sites compile unchanged.

pub use tau_runtime_core::stream::*;
