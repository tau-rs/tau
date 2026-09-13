//! Drivers: the border guards between envelopes and the world.
//!
//! A driver is the only thing that touches anything outside the kernel. It
//! receives a request the kernel routed to it, does whatever its protocol
//! requires, and reports back a payload and what it cost. The kernel never
//! looks inside either; that is what makes model-agnosticism structural.
//!
//! Drivers are named by the harness at registration, not by themselves — the
//! way a filesystem is named by its mount point. The name is what a log
//! resolves six months later, so it belongs to the operator, not the code.
//!
//! M0 ships one driver, [`echo::EchoDriver`], because the loop it proves is the
//! point: request in, log entry, delivery, reply, log entry, consumption
//! charged. Every real driver is that loop with a world on the far side.

pub mod echo;

use crate::abi::Consumption;
use crate::kernel::{BoxFuture, Delivery};

/// A driver's contract with the kernel.
pub trait Driver: Send + 'static {
    /// Handles one request; returns the reply payload and what it cost.
    ///
    /// The kernel charges the report against the requester's budget without
    /// interpreting it. Report honestly, in your native units.
    fn handle(&mut self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)>;
}
