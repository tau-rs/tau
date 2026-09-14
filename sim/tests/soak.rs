//! The Tier 2 soak's shape at Tier 0 size (HANDOFF §8, item 2).
//!
//! The soak runs the generator in its linear-per-event mode: conservation
//! sampled instead of checked at every entry, and the live population kept
//! bounded so no chooser sweeps more than a few dozen records. This suite
//! proves the mode at a size that fits the quick ceiling; `tier2.yml` runs
//! the same mode at 10^6 events and refolds the log on a second platform.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::log::Log;
use tau_kernel::reducer::fold;
use tau_sim::{conserved, run_with, Options};

const SEED: u64 = 0x7a75_0000_0000_0002;

fn soak_options(events: u64) -> Options {
    Options {
        events,
        check_every: Some(1_000),
        max_live: Some(64),
    }
}

#[test]
fn the_linear_mode_refolds_to_the_same_hash() {
    let report = run_with(SEED, &soak_options(8_000)).unwrap();
    let incremental = report.state.hash();
    let refold = fold(report.log.entries()).unwrap().hash();
    let mut bytes = Vec::new();
    report.log.write_to(&mut bytes).unwrap();
    let read = Log::read_from(bytes.as_slice()).unwrap();
    let round_trip = fold(read.entries()).unwrap().hash();
    assert_eq!(incremental, refold, "refold diverged");
    assert_eq!(
        incremental, round_trip,
        "fold of the serialized log diverged"
    );
    assert!(report.state.is_drained());
    assert!(report.accepted >= 8_000);
}

#[test]
fn the_linear_mode_is_a_pure_function_of_the_seed() {
    let first = run_with(SEED, &soak_options(2_000)).unwrap();
    let again = run_with(SEED, &soak_options(2_000)).unwrap();
    assert_eq!(first.log.entries(), again.log.entries());
    assert_eq!(first.state.hash(), again.state.hash());
}

#[test]
fn the_population_bound_is_enforced() {
    // The mix keeps the population small on its own — a dozen live agents
    // at once is typical — so the bound is proven with one tight enough to
    // bite: without it the run gets past four, with it the run never does.
    let unbounded = run_with(SEED, &Options::exhaustive(2_000)).unwrap();
    assert!(
        unbounded.peak_live > 4,
        "peak live {} never reached the tight bound",
        unbounded.peak_live
    );
    let bounded = run_with(
        SEED,
        &Options {
            events: 2_000,
            check_every: Some(1),
            max_live: Some(4),
        },
    )
    .unwrap();
    assert!(
        bounded.peak_live <= 4,
        "peak live {} exceeds the bound",
        bounded.peak_live
    );
    assert!(bounded.state.is_drained());
}

#[test]
fn an_unsampled_run_still_conserves_at_the_end() {
    // `check_every: None` samples nothing along the way; the end check is
    // unconditional, and this is the same property measured from outside.
    let options = Options {
        events: 1_000,
        check_every: None,
        max_live: Some(16),
    };
    let report = run_with(SEED, &options).unwrap();
    assert!(report.state.is_drained());
    assert!(conserved(&report.state, &report.grant).is_ok());
    assert!(report.peak_live <= 16);
}
