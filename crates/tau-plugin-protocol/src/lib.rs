#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Wire-format types and framing primitives for the tau plugin protocol.
//!
//! Plugins talk to the tau runtime over MessagePack-RPC on stdio with
//! length-prefixed framing. This crate is shared by the host (in
//! `tau-runtime::plugin_host`) and the SDK (in `tau-plugin-sdk`); it
//! contains pure types and IO helpers, no tracing, no process management.
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md` §4
//! and ADR-0008 for the design rationale.

pub mod error;
pub mod frame;
pub mod framer;
pub mod handshake;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use error::{
    ProtocolError, RpcErrorEnvelope, CAPABILITY_DENIED, INTERNAL_ERROR, INVALID_PARAMS,
    INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, PLUGIN_CONTRACT_VIOLATION,
    PORT_SPECIFIC_ERROR_BASE,
};
pub use frame::{Frame, MAX_DECODE_DEPTH};
pub use framer::{FramedReader, FramedWriter, FramerOptions};
pub use handshake::{
    meta, HandshakeRequest, HandshakeResponse, MethodSchema, TraceContext, PROTOCOL_VERSION,
};
