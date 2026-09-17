//! The two KPIs of `kpi.rs`, counted instead of timed.
//!
//! Same harness (`support/`), same seeded log, same booted kernel; the
//! instrument is callgrind through iai-callgrind, and the number is `Ir`,
//! instructions retired in the measured region. Instruction counts do not
//! move with the runner's load, which is why they and not wall-clock are
//! what the Tier 3 nightly judges (HANDOFF §8 item 5, #151): the job runs
//! this target with `--output-format=json`, and `scripts/bench-regression.sh`
//! compares each benchmark's `Ir` with the committed baseline next to this
//! file, `kpi_ir.baseline.tsv`.
//!
//! 1. **`fold`**: `reducer::fold` over the seeded log, at two sizes, so a
//!    per-entry cost that grows with the state shows up as a gap between
//!    them. The log is generated in the setup, outside the measured region.
//! 2. **`send_deliver`**: a fixed number of `send`→deliver→`recv` round
//!    trips on a kernel booted in the setup. Per-trip cost is the count
//!    divided by the batch; the batch keeps the first-trip warm-up from
//!    dominating.
//!
//! Needs valgrind and `iai-callgrind-runner` at the library's exact version
//! to *run*; it compiles anywhere, which is all `cargo bench --no-run` (Tier
//! 2 item 7) asks. Run with `cargo bench -p tau-kernel --bench kpi_ir`.
//!
//! The harness is infallible by construction (a setup returns a `Result`
//! and the benchmark passes it through), so a setup that fails is reported
//! by the teardown and exits non-zero rather than being measured as a very
//! fast benchmark.

// `library_benchmark_group!` and `main!` expand to public modules, functions
// and constants of their own, none documented; `missing_docs` is a lint on
// this crate's surface, and this crate is a bench binary with none.
#![allow(missing_docs)]

mod support;

use std::hint::black_box;
use std::process::exit;

use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use tau_kernel::log::Entry;
use tau_kernel::reducer::{fold, StateHash};

use support::{recorded_log, RoundTrip};

/// Round trips per `send_deliver` measurement. The log grows by the round
/// trip's entries each time; a hundred is enough to average out the first
/// trip's allocations and small enough to run in a moment under callgrind.
const ROUND_TRIPS: u32 = 100;

/// Fails the run when a setup or the measured code refused, instead of
/// letting the refusal count as a suspiciously cheap benchmark.
fn refuse(what: &str, e: &str) -> ! {
    eprintln!("kpi_ir bench: {what}: {e}");
    exit(1)
}

fn check_hash(outcome: Result<StateHash, String>) {
    if let Err(e) = outcome {
        refuse("fold", &e);
    }
}

#[library_benchmark(teardown = check_hash)]
#[bench::events_1k(args = (1_000), setup = recorded_log)]
#[bench::events_10k(args = (10_000), setup = recorded_log)]
fn fold_entries(entries: Result<Vec<Entry>, String>) -> Result<StateHash, String> {
    let entries = entries?;
    black_box(
        fold(entries.iter())
            .map(|state| state.hash())
            .map_err(|e| format!("fold refused: {e}")),
    )
}

/// Boots the kernel outside the measured region; the count rides along.
fn booted(round_trips: u32) -> Result<(RoundTrip, u32), String> {
    RoundTrip::boot().map(|trip| (trip, round_trips))
}

/// Drains and shuts the kernel down outside the measured region.
fn finish(outcome: Result<RoundTrip, String>) {
    match outcome {
        Ok(trip) => {
            if let Err(e) = trip.finish() {
                refuse("send_deliver finish", &e);
            }
        }
        Err(e) => refuse("send_deliver", &e),
    }
}

#[library_benchmark(teardown = finish)]
#[bench::x100(args = (ROUND_TRIPS), setup = booted)]
fn send_deliver(booted: Result<(RoundTrip, u32), String>) -> Result<RoundTrip, String> {
    let (mut trip, round_trips) = booted?;
    for _ in 0..round_trips {
        trip.once()?;
    }
    Ok(black_box(trip))
}

library_benchmark_group!(name = kpi; benchmarks = fold_entries, send_deliver);
main!(library_benchmark_groups = kpi);
