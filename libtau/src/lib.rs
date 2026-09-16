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
//!   (or the cancel notice that pre-empts it), `read`, decode. Nothing here
//!   reads `abi` off a delivered envelope: that stamp is the handing-over
//!   build's and a replay from a snapshot does not preserve it (ADR-0011
//!   §3, documented on `Handle::recv`).
//! - [`RetryPolicy`] and [`infer_with`]: the same call again when the
//!   driver could not get an answer. Every attempt is its own `send`, and
//!   the wait comes from a sleep the caller supplies.
//! - [`Note`]: where a hook's `Emit` lands. Every call takes a
//!   `&mut Vec<Note>`; a hook notice the call's `recv` resolved while
//!   waiting is pushed there instead of dropped, and the program reads it
//!   after the call. A cancel notice still ends the call.
//! - [`Toolbox`]: the projected namespace. One tool per capability, named
//!   by the `DriverId` the harness registered the driver under, with the
//!   driver's schema compiled for validation.
//! - [`tool_loop`] and [`tool_loop_with`]: the loop of HANDOFF §3.2. Every
//!   failure short of a budget refusal is fed back to the model as a
//!   `tool_result`, so it can self-correct.
//!
//! # What is deliberately not here
//!
//! Fan-out, a bound on the number of rounds, and a retry that the budget
//! does not see. A budget bounds the loop already: every `send` — a retry
//! included — reserves the driver's ceiling plus one call, and the loop
//! ends the moment that reservation is refused. No driver retries on its
//! own, and this crate reads no clock: the wait between attempts is a
//! future the caller hands in.
//!
//! [`ModelRequest`]: tau_kernel::bridge::ModelRequest
//! [`ModelReply`]: tau_kernel::bridge::ModelReply

mod infer;
mod tool_loop;
mod toolbox;

pub use infer::{
    decode_reply, encode_request, infer, infer_with, should_retry, InferError, Note, RetryPolicy,
};
pub use tool_loop::{prompt, render_result, tool_loop, tool_loop_with, ToolLoopError};
pub use toolbox::{ProjectError, Toolbox};
