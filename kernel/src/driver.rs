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
//! Two drivers ship: [`echo::EchoDriver`], because the loop it proves is the
//! point — request in, log entry, delivery, reply, log entry, consumption
//! charged — and [`clock::VirtualClock`], which is not a request/reply driver
//! at all but the one thing allowed to tell the kernel that time passed.

pub mod clock;
pub mod echo;

use crate::abi::{Consumption, Corr};
use crate::kernel::{BoxFuture, Delivery};

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
