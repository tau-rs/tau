//! The model bridge contract: the bytes a model driver and the `libtau` tool
//! loop agree on (ADR-0006).
//!
//! # What this module is
//!
//! A vocabulary the kernel *carries* for its neighbours and never *uses*.
//! The kernel routes by capability and accounts by driver-reported
//! consumption; it does not parse payloads (ADR-0003, invariant 2). These
//! types exist here because both the driver crate and `libtau` depend on
//! `tau-kernel`, and a contract that lived in two places would drift. The
//! kernel-proper sources never import this module, and a test checks that.
//!
//! # What this module is not
//!
//! It is not ABI. Nothing here is frozen by the wire snapshots or by [`ABI`]
//! (`crate::abi::ABI`); the bridge carries its own version, [`VERSION`], on
//! every request and reply, and evolves under ADR-0006's rules: optional
//! fields are additive, anything a v1 reader cannot parse bumps the number.
//!
//! # Shape
//!
//! ```text
//! agent ──send(model_cap, ModelRequest bytes)──▶ model driver
//! agent ◀──reply(ModelReply bytes, Consumption)── model driver
//!
//! agent ──send(tool_cap, tool_call.input bytes)──▶ tool driver
//! agent ◀──reply(bytes = tool_result.content)───── tool driver
//! ```
//!
//! A [`ModelRequest`] carries the conversation, the projected tools, and the
//! output cap; a [`ModelReply`] carries what the model said, why it stopped,
//! and what it used. Tool calls and results are [`Content`] blocks inside
//! both. Which tools exist is answered by the driver, not this module: see
//! [`Driver::describe`](crate::driver::Driver::describe).
//!
//! [`ABI`]: crate::abi::ABI

mod reply;
mod request;

pub use reply::{ErrorKind, ModelError, ModelReply, StopReason, Usage};
pub use request::{Content, Message, ModelRequest, Role, Sampling, ToolDef, ToolErrorKind};

/// The bridge version stamped on every [`ModelRequest`] and [`ModelReply`].
///
/// A driver that receives a request stamped with a version it does not
/// implement replies [`ErrorKind::Unsupported`]; it never guesses.
pub const VERSION: u16 = 1;
