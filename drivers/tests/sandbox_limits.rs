//! The two stops that take time by construction: `RLIMIT_CPU` needs a
//! whole second of CPU, and the wall bound needs the run to outlive it.
//! This binary runs under the `ci` nextest profile; `quick` skips it
//! (`.config/nextest.toml`).

#![cfg(all(feature = "sandbox", unix))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::sandbox::{config, driver, request, run_one};
use tau_drivers::sandbox::wire::Stop;
use tau_kernel::abi::DimKey;

#[tokio::test]
async fn cpu_limit_is_the_host_kernel_killing_the_run_billed_at_the_ceiling() {
    let (reply, root) = run_one(
        driver(config(1, Duration::from_secs(20))),
        request("echo spinning; while :; do :; done"),
    )
    .await;
    assert_eq!(reply.stop, Stop::CpuLimit, "{reply:?}");
    assert_eq!(reply.stdout, "spinning\n", "what it printed before");
    let ms = reply.usage.compute_ms();
    // Linux enforces against tick-sampled time and reports scheduler
    // runtime; on a virtualised runner they drift.
    assert!(ms >= 700, "about a second of CPU: {ms} ms");
    assert_eq!(root.spent.get(&DimKey::ComputeMs), Some(&ms));
    // At the ceiling to within the host's accounting granularity; a hair
    // over is overdraft the kernel records, not a refusal.
    assert!(ms <= 1_300, "{ms} ms");
}

#[tokio::test]
async fn wall_limit_is_the_shim_killing_the_run_with_real_usage() {
    let mut c = config(5, Duration::from_millis(500));
    // The driver's last resort fires at `wall + abandon_grace` counted from
    // the shim's spawn; the shim's wall bound only starts once it has set
    // its limits, spawned the interpreter, and written the partial report
    // (ADR-0009 §4). The grace therefore has to absorb the shim's whole
    // startup plus its kill-reap-report tail, and under full-workspace load
    // the default second was not enough: the shim was SIGKILLed before its
    // report and the reply was `lost` (#211). This test is about the wall
    // bound, not the last resort — `a_shim_that_never_reports_...` below
    // is — so the grace is wide. It costs nothing on the green path: the
    // shim reports at the wall and the driver returns at once.
    c.abandon_grace = Duration::from_secs(10);
    let (reply, root) = run_one(driver(c), request("echo waiting; sleep 20; echo never")).await;
    assert_eq!(reply.stop, Stop::WallLimit, "{reply:?}");
    assert_eq!(reply.stdout, "waiting\n");
    let ms = reply.usage.compute_ms();
    assert!(ms < 500, "sleeping costs no CPU: {ms} ms");
    assert_eq!(root.spent.get(&DimKey::ComputeMs), Some(&ms));
}

#[tokio::test]
async fn a_shim_that_never_reports_is_killed_at_the_last_resort() {
    // A fake shim that ignores its flags and sleeps: no report, no exit.
    // The driver's deadline is wall + abandon_grace, after which it kills
    // the group and replies lost, billed at the ceiling.
    let dir = common::sandbox::outside_dir("fake-shim");
    let fake = dir.join("shim.sh");
    std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut c = config(2, Duration::from_millis(300));
    c.abandon_grace = Duration::from_millis(300);
    c.shim = Some(fake);
    let (reply, root) = run_one(driver(c), request("echo hi")).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, tau_drivers::sandbox::wire::ErrorKind::Lost);
    assert!(e.message.contains("no report"), "{}", e.message);
    assert_eq!(
        root.spent.get(&DimKey::ComputeMs),
        Some(&2_000),
        "the ceiling"
    );
    let _ = std::fs::remove_dir_all(dir);
}
