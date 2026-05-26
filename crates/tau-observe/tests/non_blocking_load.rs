//! Producer-side latency assertion for the non_blocking install path.
//!
//! Emits 100,000 INFO events to a file-backed subscriber and asserts
//! no single emission took longer than 10 ms. The exact bound is
//! generous on purpose; the point is to catch a regression where the
//! writer goes back to blocking semantics. Producer-emit timing can
//! shift under CI noise (captured stdio, shared runners), so the
//! threshold is several orders of magnitude above the warm-machine
//! steady-state (typically <1 ms).

#![cfg(feature = "non_blocking")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tau_observe::filter::env_or_directive;
use tau_observe::install::{install, Format, InstallOptions, Rotation, Writer};

#[test]
fn producer_latency_stays_under_10ms_per_event() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path: PathBuf = tmp.path().join("load.log");

    let opts = InstallOptions {
        filter: env_or_directive("info"),
        format: Format::Human,
        writer: Writer::Stderr, // ignored when file_path is set
        non_blocking: true,
        file_path: Some(log_path.clone()),
        rotation: Rotation::Never,
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
        worst < Duration::from_millis(10),
        "worst-case producer latency {:?} exceeded 10ms",
        worst
    );
}
