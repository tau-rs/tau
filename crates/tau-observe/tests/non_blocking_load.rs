//! Producer-side latency assertion for the non_blocking install path.
//!
//! Emits 100,000 INFO events to a file-backed subscriber and asserts no
//! single emission exceeds the latency budget. The point is to catch a
//! regression where the writer goes back to blocking semantics — if
//! that happens, worst-case latency jumps to seconds, not tens of
//! milliseconds.
//!
//! Threshold is set well above platform scheduler granularity:
//! - Linux/macOS typical: <1 ms (steady-state observed ~160µs).
//! - Windows: ~15.6 ms (default scheduler tick) is the floor; CI
//!   runners can spike above that under contention.
//!
//! 100 ms is two orders of magnitude above the warm-machine steady
//! state, comfortably above Windows' tick budget, and still
//! 50–500x below the seconds-range failure mode the assertion exists
//! to catch.

#![cfg(feature = "non_blocking")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tau_observe::filter::env_or_directive;
use tau_observe::install::{install, Format, InstallOptions, Rotation, Writer};

const LATENCY_BUDGET: Duration = Duration::from_millis(100);

#[test]
fn producer_latency_stays_within_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path: PathBuf = tmp.path().join("load.log");

    let opts = InstallOptions {
        filter: env_or_directive("info"),
        format: Format::Human,
        writer: Writer::Stderr, // ignored when file_path is set
        non_blocking: true,
        file_path: Some(log_path.clone()),
        rotation: Rotation::Never,
        extra_layers: Vec::new(),
        #[cfg(feature = "otlp")]
        otlp: None,
    };
    let _guard = install(opts).expect("install");

    let mut worst = Duration::ZERO;
    for i in 0..100_000 {
        let start = Instant::now();
        tracing::info!(idx = i, "load.test_event");
        let elapsed = start.elapsed();
        if elapsed > worst {
            worst = elapsed;
        }
    }
    assert!(
        worst < LATENCY_BUDGET,
        "worst-case producer latency {worst:?} exceeded {LATENCY_BUDGET:?}",
    );
}
