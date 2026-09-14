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
//! Two kinds ship: [`echo::EchoDriver`], because the loop it proves is the
//! point — request in, log entry, delivery, reply, log entry, consumption
//! settled — and the clocks in [`clock`], which are not request/reply drivers
//! at all but the only things allowed to tell the kernel that time passed.
//!
//! A driver is registered with a *ceiling*: the most one request to it may
//! cost, declared by the harness. The kernel reserves that much from the
//! sender before delivery and settles against the driver's report, so a
//! driver that reports honestly within its ceiling is all the accounting
//! needs. A report above it is charged in full and recorded as overdraft on
//! the agent; supervision (M3) is what will act on that.

pub mod clock;
pub mod echo;

use crate::abi::{Consumption, Corr};
use crate::kernel::{BoxFuture, Delivery};

/// What a driver offers to a model, for schema projection (ADR-0006 §5).
///
/// Opaque to the kernel: it forwards these bytes from the driver to the agent
/// that asked, and never reads them. The tool's *name* is not here — it is
/// the [`DriverId`](crate::abi::DriverId) the harness registered the driver
/// under, the way a filesystem is named by its mount point, so two instances
/// of one driver type cannot collide and the name in a model transcript is
/// the name in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSchema {
    /// One or two sentences the model reads to decide when to call this tool.
    pub description: String,
    /// A JSON Schema (draft 2020-12) for the tool's input, as bytes.
    ///
    /// Bytes rather than a parsed document, so that the kernel has nothing
    /// to interpret. The driver derives it from the same type it validates
    /// the input with, so there is one source of truth per driver.
    pub input_schema: Vec<u8>,
}

/// A driver's contract with the kernel.
///
/// `&self` throughout: the kernel calls [`Driver::abandon`] from a `cancel`
/// while the driver's loop may be inside [`Driver::handle`] for that very
/// request, so a driver that keeps state keeps it behind its own lock.
pub trait Driver: Send + Sync + 'static {
    /// Handles one request; returns the reply payload and what it cost.
    ///
    /// The kernel charges the report against the requester's budget without
    /// interpreting it. Report honestly, in your native units.
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)>;

    /// The tool this driver is, if it is one.
    ///
    /// A driver is at most one tool: a driver with several operations puts
    /// the discriminator in its own schema. The invocation payload it then
    /// receives is exactly the model's `input` bytes, and its reply bytes
    /// are exactly what the model reads back (ADR-0006 §5).
    ///
    /// Must be cheap and pure: the kernel calls it on every
    /// `Handle::describe`, and nothing is logged. The default is `None` —
    /// not a tool — which is right for a model driver, a clock, and the echo
    /// driver alike.
    fn describe(&self) -> Option<ToolSchema> {
        None
    }

    /// The requester of `corr` has been cancelled: stop working on it if you
    /// can. Phase two of `cancel` (ADR-0002).
    ///
    /// Advisory, not a contract to stay silent: a reply that arrives anyway is
    /// delivered while the owner is in its grace period and dead-lettered
    /// after. The default does nothing, which is correct for a driver whose
    /// requests cost nothing to finish.
    fn abandon(&self, corr: Corr) {
        let _ = corr;
    }
}
