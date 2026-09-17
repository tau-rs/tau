//! The harness the two KPI targets share, so `kpi.rs` (criterion, wall-clock)
//! and `kpi_ir.rs` (iai-callgrind, instruction count) measure the same
//! thing: the same seeded log for the fold, the same booted kernel and echo
//! driver for the round trip. A drift between the two numbers is then a
//! property of the instrument, not of the workload.
//!
//! Not a bench target: cargo only discovers `benches/*.rs` and
//! `benches/*/main.rs`.

use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel, KernelError};
use tau_kernel::log::{Entry, Log};
use tau_kernel::syscall::{program, Match};
use tau_sim::{run_unchecked, Options};
use tokio::runtime::{Builder, Handle, Runtime};
use tokio::sync::mpsc;

/// The seed behind the fold workload. Any value works; one value means the
/// numbers on two machines describe the same log.
pub(crate) const SEED: u64 = 0x5eed_0003;

/// The latency payload: one byte, so the echo bills one token per round trip
/// and the root's budget lasts however many iterations the harness wants.
const PAYLOAD: &[u8] = b"x";

/// What one echo may cost at most; reserved before every delivery.
const CEILING: u64 = 64;

/// The root's grant: enough for every iteration either harness will ever draw.
const GRANT: u64 = 1 << 40;

/// The shape the soak runs: no conservation checks along the way, and the
/// live population capped so the per-event cost is the reducer's own rather
/// than the generator's.
fn workload(events: u64) -> Options {
    Options {
        events,
        check_every: None,
        max_live: Some(64),
        snapshot_every: None,
    }
}

/// A seeded log of about `events` entries, generated outside the timed loop.
pub(crate) fn recorded_log(events: u64) -> Result<Vec<Entry>, String> {
    let report = run_unchecked(SEED, &workload(events))
        .map_err(|e| format!("sim run for {events} events failed: {e}"))?;
    Ok(report.log.entries().to_vec())
}

/// tokio as the body (ADR-0003): spawn on the harness's runtime, hand back
/// the abort. Taken by handle rather than `tokio::spawn`, because the kernel
/// is booted outside `block_on`.
fn spawner(handle: Handle) -> impl Fn(BoxFuture<()>) -> AbortHandle + Send + Sync + 'static {
    move |fut| {
        let task = handle.spawn(fut);
        Box::new(move || task.abort())
    }
}

/// One kernel, one root, alive across round trips. Each [`RoundTrip::once`]
/// nudges the root over `go`; the root sends one byte to the echo driver,
/// awaits the matching reply, and answers over `done`. The measured region
/// is that round trip: `send`, delivery, the driver's reply, `recv`
/// resolving. The kernel is booted once so its boot is not in the number;
/// the log grows by the round trip's entries each iteration, which is the
/// price of an event-sourced kernel and belongs in the number.
pub(crate) struct RoundTrip {
    rt: Runtime,
    kernel: std::sync::Arc<Kernel>,
    go_tx: mpsc::Sender<()>,
    done_rx: mpsc::Receiver<Result<(), KernelError>>,
}

impl RoundTrip {
    /// Boots the kernel on a current-thread runtime, registers the echo
    /// driver, and spawns the root that will answer every nudge.
    pub(crate) fn boot() -> Result<Self, String> {
        let rt = Builder::new_current_thread()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        let kernel = Kernel::boot(Log::in_memory(), spawner(rt.handle().clone()));
        let echo_id = DriverId::new(Name::new("echo").map_err(|e| format!("driver name: {e}"))?);
        let echo = kernel
            .register_driver(
                echo_id,
                EchoDriver::new(),
                Budget::from_dims([(DimKey::Tokens, CEILING)]),
            )
            .map_err(|e| format!("register echo: {e}"))?;

        let (go_tx, mut go_rx) = mpsc::channel::<()>(1);
        let (done_tx, done_rx) = mpsc::channel::<Result<(), KernelError>>(1);
        kernel
            .spawn_root(
                program(move |root| async move {
                    while go_rx.recv().await.is_some() {
                        let outcome = match root.send(echo, PAYLOAD) {
                            Ok(corr) => root.recv(Match::Corr(corr)).await.map(|_msg| ()),
                            Err(e) => Err(e),
                        };
                        if done_tx.send(outcome).await.is_err() {
                            break;
                        }
                    }
                    root.exit(&[])
                }),
                Namespace::from_caps([echo]),
                Budget::from_dims([(DimKey::Tokens, GRANT), (DimKey::Calls, GRANT)]),
            )
            .map_err(|e| format!("spawn root: {e}"))?;

        Ok(Self {
            rt,
            kernel,
            go_tx,
            done_rx,
        })
    }

    /// One `send`→deliver→`recv` round trip, driven to completion.
    pub(crate) fn once(&mut self) -> Result<(), String> {
        let go_tx = &self.go_tx;
        let done_rx = &mut self.done_rx;
        self.rt.block_on(async {
            if go_tx.send(()).await.is_err() {
                return Err("the root is gone".to_owned());
            }
            match done_rx.recv().await {
                Some(Ok(())) => Ok(()),
                Some(Err(e)) => Err(format!("round trip refused: {e}")),
                None => Err("the root is gone".to_owned()),
            }
        })
    }

    /// Lets the root exit, waits for the kernel to drain, and shuts it down.
    pub(crate) fn finish(self) -> Result<(), String> {
        let Self {
            rt,
            kernel,
            go_tx,
            done_rx,
        } = self;
        drop(go_tx);
        drop(done_rx);
        rt.block_on(kernel.drained())
            .map_err(|e| format!("drain: {e}"))?;
        kernel.shutdown();
        Ok(())
    }
}
