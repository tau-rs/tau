//! tau-mcp — MCP (Model Context Protocol) facilitator types.
//!
//! Pure types + canonical-hash + cassette format. No I/O, no tokio.
//! Transports + lifecycle live in `tau-mcp-tokio`.
//!
//! # Modules
//!
//! - [`protocol`] — MCP wire types: JSON-RPC envelopes, the five v0 method
//!   payloads (`initialize`, `tools/list`, `tools/call`,
//!   `sampling/createMessage`, `roots/list`), notifications, cancellation.
//! - [`contract`] — `ServerContract` (the schema + capability declaration
//!   tau-build pins) + canonical hash (`Hash256` = SHA-256 of canonical
//!   JSON) + pinned-contract file (de)serializer.
//! - [`host`] — `HostHandlers` trait with default-deny baseline impl.
//! - [`cassette`] — transport-agnostic message-level recorder + replayer.
//! - [`transport`] — `Transport` trait shared by `tau-mcp-tokio` impls.
//!
//! # Spec
//!
//! [β.3 MCP facilitator design](https://github.com/LEBOCQTitouan/tau/blob/main/docs/superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md).

#![no_std]
#![cfg_attr(test, allow(unused_extern_crates))]

extern crate alloc;

#[cfg(any(test, feature = "with-std-adapters"))]
extern crate std;

pub mod cassette;
pub mod contract;
pub mod error;
pub mod host;
pub mod protocol;
pub mod transport;

pub use error::McpError;
