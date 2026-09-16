//! The two benchmark KPIs ADR-0003 names, and nothing else:
//!
//! 1. **Reducer throughput**, in entries per second: `reducer::fold` over a
//!    log the sim crate generated from a fixed seed, so the input is the
//!    same on every machine and every run and exercises every entry kind.
//! 2. **`send`→deliver latency**: one `send` through the kernel to the echo
//!    driver and back to the matching `recv`, on a current-thread tokio
//!    runtime, measured per round trip.
//!
//! These are measurements, not judgements. Thresholds and baselines are Tier
//! 3 (HANDOFF §8 item 5); this file only has to keep compiling (Tier 2 item
//! 7) and keep reporting.
//!
//! Run with `cargo bench -p tau-kernel`; `--no-run` is the build-only gate.

use std::process::ExitCode;

use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel, KernelError};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::fold;
use tau_kernel::syscall::{program, Match};
use tau_sim::{run_unchecked, Options};
use tokio::runtime::{Builder, Handle};
use tokio::sync::mpsc;

/// The seed behind the fold workload. Any value works; one value means the
/// numbers on two machines describe the same log.
const SEED: u64 = 0x5eed_0003;

/// Event counts for the fold workload. Two sizes, so a per-entry cost that
/// grows with the state shows up as a throughput drop between them.
const FOLD_EVENTS: [u64; 2] = [1_000, 10_000];

/// The latency payload: one byte, so the echo bills one token per round trip
/// and the root's budget lasts however many iterations criterion wants.
const PAYLOAD: &[u8] = b"x";

/// What one echo may cost at most; reserved before every delivery.
const CEILING: u64 = 64;

/// The root's grant: enough for every iteration criterion will ever draw.
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
fn recorded_log(events: u64) -> Result<Vec<Entry>, String> {
    let report = run_unchecked(SEED, &workload(events))
        .map_err(|e| format!("sim run for {events} events failed: {e}"))?;
    Ok(report.log.entries().to_vec())
}

fn fold_throughput(c: &mut Criterion) -> Result<(), String> {
    let mut group = c.benchmark_group("fold");
    for events in FOLD_EVENTS {
        let entries = recorded_log(events)?;
        let count = u64::try_from(entries.len()).map_err(|e| e.to_string())?;
        group.throughput(Throughput::Elements(count));
        group.bench_with_input(
            BenchmarkId::new("entries", count),
            &entries,
            |b, entries| {
                b.iter(|| fold(entries.iter()).map(|state| state.hash()));
            },
        );
    }
    group.finish();
    Ok(())
}

/// tokio as the body (ADR-0003): spawn on the bench's runtime, hand back
/// the abort. Taken by handle rather than `tokio::spawn`, because the kernel
/// is booted outside `block_on`.
fn spawner(handle: Handle) -> impl Fn(BoxFuture<()>) -> AbortHandle + Send + Sync + 'static {
    move |fut| {
        let task = handle.spawn(fut);
        Box::new(move || task.abort())
    }
}

/// One kernel, one root, alive for the whole bench. Each iteration nudges
/// the root over `go`; the root sends one byte to the echo driver, awaits
/// the matching reply, and answers over `done`. The timed region is that
/// round trip: `send`, delivery, the driver's reply, `recv` resolving. The
/// kernel is booted once so its boot is not in the number; the log grows by
/// the round trip's entries each iteration, which is the price of an
/// event-sourced kernel and belongs in the number.
///
/// That growth is also why this bench runs shorter than criterion's default:
/// about a kilobyte of log and state per round trip, at a few microseconds
/// each, is a gigabyte and more over the default eight seconds. Three
/// seconds keeps it to a few hundred megabytes on a fast machine, and fifty
/// samples is still a tight interval at this variance.
fn send_deliver_latency(c: &mut Criterion) -> Result<(), String> {
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
    let (done_tx, mut done_rx) = mpsc::channel::<Result<(), KernelError>>(1);
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

    let mut failure: Option<String> = None;
    let mut group = c.benchmark_group("latency");
    group
        .sample_size(50)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2));
    group.bench_function("send_deliver", |b| {
        b.iter_batched(
            || (),
            |()| {
                let outcome = rt.block_on(async {
                    if go_tx.send(()).await.is_err() {
                        return Err("the root is gone".to_owned());
                    }
                    match done_rx.recv().await {
                        Some(Ok(())) => Ok(()),
                        Some(Err(e)) => Err(format!("round trip refused: {e}")),
                        None => Err("the root is gone".to_owned()),
                    }
                });
                if let Err(e) = outcome {
                    failure.get_or_insert(e);
                }
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();

    drop(go_tx);
    rt.block_on(kernel.drained())
        .map_err(|e| format!("drain: {e}"))?;
    kernel.shutdown();
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn main() -> ExitCode {
    let mut c = Criterion::default().configure_from_args();
    let outcome = fold_throughput(&mut c).and_then(|()| send_deliver_latency(&mut c));
    c.final_summary();
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("kpi bench: {message}");
            ExitCode::FAILURE
        }
    }
}
