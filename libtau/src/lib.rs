//! `libtau`: the userspace layer over the tau kernel.
//!
//! Everything here is an ordinary program over the seven syscalls
//! (ADR-0002). It holds no special power: a model is a driver behind a
//! capability, a tool is another, and this crate only ever calls `send`,
//! `recv`, `read`, and `describe` on a [`Handle`](tau_kernel::syscall::Handle)
//! it was given. If something in here cannot be written that way, that is
//! evidence about the kernel, not a reason for a back door.
//!
//! # Shape
//!
//! ```text
//!             ┌────────── libtau ───────────┐
//!  caps ──▶ Toolbox::project ── describe() ─┼──▶ tools[] in the request
//!                                           │
//!  request ─▶ infer ── send/recv/read ───────┼──▶ model driver ──▶ ModelReply
//!                                           │
//!  reply.stop == tool_call ─▶ tool_loop ─────┼──▶ resolve name → cap
//!                                           │    validate input → schema
//!                                           │    send(cap, input) / recv
//!                                           │    tool_result → next turn
//!             └─────────────────────────────┘
//! ```
//!
//! The bytes on every arrow are the bridge contract in
//! [`tau_kernel::bridge`] (ADR-0006): a [`ModelRequest`] goes to the model,
//! a [`ModelReply`] comes back, and a tool is invoked with exactly its
//! `tool_call.input` bytes and answered with exactly its reply bytes.
//!
//! # What is here
//!
//! - [`infer`]: one model call. Encode, `send`, `recv` on the correlation
//!   (or the cancel notice that pre-empts it), `read`, decode.
//! - [`Toolbox`]: the projected namespace. One tool per capability, named
//!   by the `DriverId` the harness registered the driver under, with the
//!   driver's schema compiled for validation.
//! - [`tool_loop`]: the loop of HANDOFF §3.2. Every failure short of a
//!   budget refusal is fed back to the model as a `tool_result`, so it can
//!   self-correct.
//!
//! # What is deliberately not here
//!
//! Retries, fan-out, and a bound on the number of rounds. A budget bounds
//! the loop already: every `send` reserves the driver's ceiling plus one
//! call, and the loop ends the moment that reservation is refused.
//!
//! [`ModelRequest`]: tau_kernel::bridge::ModelRequest
//! [`ModelReply`]: tau_kernel::bridge::ModelReply

mod infer;
mod tool_loop;
mod toolbox;

pub use infer::{decode_reply, encode_request, infer, InferError};
pub use tool_loop::{prompt, render_result, tool_loop, ToolLoopError};
pub use toolbox::{ProjectError, Toolbox};
