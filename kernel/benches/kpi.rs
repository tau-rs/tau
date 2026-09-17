//! The two benchmark KPIs ADR-0003 names, and nothing else:
//!
//! 1. **Reducer throughput**, in entries per second: `reducer::fold` over a
//!    log the sim crate generated from a fixed seed, so the input is the
//!    same on every machine and every run and exercises every entry kind.
//! 2. **`send`→deliver latency**: one `send` through the kernel to the echo
//!    driver and back to the matching `recv`, on a current-thread tokio
//!    runtime, measured per round trip.
//!
//! These are measurements, not judgements: wall-clock on a shared runner is
//! noise. The judged number is the instruction count of the same two KPIs,
//! over the same harness (`support/`), in `kpi_ir.rs` (Tier 3 item 5);
//! this file only has to keep compiling (Tier 2 item 7) and keep reporting.
//!
//! Run with `cargo bench -p tau-kernel --bench kpi`; `--no-run` is the
//! build-only gate.

mod support;

use std::process::ExitCode;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use tau_kernel::reducer::fold;

use support::{recorded_log, RoundTrip};

/// Event counts for the fold workload. Two sizes, so a per-entry cost that
/// grows with the state shows up as a throughput drop between them.
const FOLD_EVENTS: [u64; 2] = [1_000, 10_000];

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

/// One kernel, one root, alive for the whole bench ([`RoundTrip`]); the
/// timed region is one round trip.
///
/// The log grows by the round trip's entries each iteration, which is why
/// this bench runs shorter than criterion's default: about a kilobyte of log
/// and state per round trip, at a few microseconds each, is a gigabyte and
/// more over the default eight seconds. Three seconds keeps it to a few
/// hundred megabytes on a fast machine, and fifty samples is still a tight
/// interval at this variance.
fn send_deliver_latency(c: &mut Criterion) -> Result<(), String> {
    let mut trip = RoundTrip::boot()?;

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
                if let Err(e) = trip.once() {
                    failure.get_or_insert(e);
                }
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();

    trip.finish()?;
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
