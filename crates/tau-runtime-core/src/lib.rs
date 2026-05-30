#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Executor-agnostic kernel of tau. Drives the agent loop independently
//! of any async runtime. Host shells (`tau-runtime-tokio`,
//! `tau-runtime-embassy`, future smol/async-std/glommio/wasm shells)
//! link this crate and supply executor-specific adapters.
//!
//! See `docs/superpowers/specs/2026-05-30-tau-runtime-core-design.md`
//! for the design.

extern crate alloc;

#[cfg(any(test, feature = "host-fs"))]
extern crate std;

pub mod error;
pub use error::{
    BuildError, CapabilityDenial, HandshakeFailureReason, PluginKind, RuntimeError,
};
