//! The sandbox driver through a real kernel with a real `/bin/sh`: every
//! `stop` and `error.kind` that does not need a second of CPU or wall,
//! each produced by a real process; the output bound; the scrubbed
//! environment and the fresh directory; and cancel reaching the
//! interpreter.
//!
//! `cpu_limit` and `wall_limit` take at least a second by construction and
//! live in `sandbox_limits.rs`, which the quick profile skips.

#![cfg(all(feature = "sandbox", unix))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::sandbox::{
    alive, config, driver, eventually, outside_dir, request, run_one, sh_id, tokio_spawner,
    Recorded,
};
use serde_json::json;
use tau_drivers::sandbox::wire::{ErrorKind, Stop};
use tau_drivers::sandbox::SandboxDriver;
use tau_kernel::abi::{AgentId, Budget, Corr, DimKey, Namespace};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::reducer::{Outcome, Status};
use tau_kernel::syscall::{program, CancelMode, Match, WaitFor};
use tokio::sync::Notify;

fn sh() -> SandboxDriver {
    driver(config(5, Duration::from_secs(10)))
}

#[tokio::test]
async fn exit_reports_the_status_both_streams_and_real_cpu() {
    let payload = serde_json::to_vec(&json!({
        "code": "read x; echo \"got $x\"; echo oops >&2; exit 3",
        "stdin": "hello\n",
    }))
    .unwrap();
    let (reply, root) = run_one(sh(), payload).await;
    assert_eq!(reply.stop, Stop::Exit(3));
    assert_eq!(reply.stdout, "got hello\n");
    assert_eq!(reply.stderr, "oops\n");
    assert!(!reply.truncated.stdout && !reply.truncated.stderr);
    assert!(reply.usage.max_rss_bytes > 0, "{:?}", reply.usage);
    let ms = reply.usage.compute_ms();
    assert_eq!(
        root.spent.get(&DimKey::ComputeMs),
        Some(&ms),
        "billed what the shim measured"
    );
    assert!(ms < 5_000, "within the ceiling: {ms} ms");
    assert!(root.overdraft.is_empty());
    assert_eq!(root.spent.get(&DimKey::Calls), Some(&1));
    assert_eq!(root.budget.get(&DimKey::ComputeMs), Some(60_000 - ms));
}

#[tokio::test]
async fn exit_zero_is_not_special() {
    let (reply, _) = run_one(sh(), request("printf 6")).await;
    assert_eq!(reply.stop, Stop::Exit(0));
    assert_eq!(reply.stdout, "6");
}

#[tokio::test]
async fn a_signal_is_named() {
    let (reply, root) = run_one(sh(), request("echo before; kill -SEGV $$")).await;
    assert_eq!(reply.stop, Stop::Signal("SIGSEGV".into()));
    assert_eq!(reply.stdout, "before\n", "what it printed before it died");
    assert!(
        root.spent.contains_key(&DimKey::ComputeMs),
        "a crash is billed"
    );
}

#[tokio::test]
async fn an_unsupported_version_runs_nothing_and_bills_nothing() {
    let payload = serde_json::to_vec(&json!({ "v": 2, "code": "echo hi" })).unwrap();
    let (reply, root) = run_one(sh(), payload).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("version 2"), "{}", e.message);
    assert_eq!(reply.stdout, "");
    assert!(
        !root.spent.contains_key(&DimKey::ComputeMs),
        "nothing billed"
    );
    assert_eq!(root.budget.get(&DimKey::ComputeMs), Some(60_000));
}

#[tokio::test]
async fn code_over_the_bound_is_unsupported_with_the_adr_message() {
    let mut c = config(5, Duration::from_secs(10));
    c.code_bytes = 65_536;
    let payload = request(&"x".repeat(131_072));
    let (reply, root) = run_one(driver(c), payload).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert_eq!(e.message, "code is 131072 bytes; the bound is 65536");
    assert!(!root.spent.contains_key(&DimKey::ComputeMs));
}

#[tokio::test]
async fn a_request_with_an_unknown_field_is_unsupported() {
    let payload = serde_json::to_vec(&json!({ "code": "echo hi", "argv": ["-x"] })).unwrap();
    let (reply, _) = run_one(sh(), payload).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("argv"), "{}", e.message);
}

#[tokio::test]
async fn a_missing_shim_is_a_host_error_billed_at_nothing() {
    let mut c = config(5, Duration::from_secs(10));
    c.shim = Some("/nonexistent/tau-sandbox-shim".into());
    let (reply, root) = run_one(driver(c), request("echo hi")).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Host);
    assert!(
        e.message.contains("/nonexistent/tau-sandbox-shim"),
        "{}",
        e.message
    );
    assert!(!root.spent.contains_key(&DimKey::ComputeMs));
}

#[tokio::test]
async fn a_scratch_root_that_cannot_be_used_is_a_host_error() {
    let mut c = config(5, Duration::from_secs(10));
    c.scratch_root = Some("/nonexistent/tau-scratch".into());
    let (reply, root) = run_one(driver(c), request("echo hi")).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Host);
    assert!(e.message.contains("scratch directory"), "{}", e.message);
    assert!(!root.spent.contains_key(&DimKey::ComputeMs));
}

/// Not run under AddressSanitizer: `asan+lsan (sandbox)` in tier2.yml
/// excludes this test by name, because the ASan runtime's shadow mapping
/// needs terabytes of address space and dies at startup under any
/// `RLIMIT_AS` a test would set.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_memory_bound_reaches_the_shim_and_a_small_run_fits_under_it() {
    let mut c = config(5, Duration::from_secs(10));
    c.memory_bytes = Some(512 * 1024 * 1024);
    let (reply, _) = run_one(driver(c), request("echo fits")).await;
    assert_eq!(reply.stop, Stop::Exit(0), "{}", reply.stderr);
    assert_eq!(reply.stdout, "fits\n");
}

#[test]
fn the_shim_refuses_bad_arguments_and_reports_a_report_it_cannot_write() {
    use std::process::Command;
    let status = |args: &[&str]| {
        Command::new(common::sandbox::SHIM)
            .args(args)
            .status()
            .unwrap()
    };
    assert_eq!(status(&["--bogus", "1"]).code(), Some(2), "unknown flag");
    assert_eq!(status(&["--cpu-s"]).code(), Some(2), "flag without a value");
    assert_eq!(
        status(&[
            "--report",
            "r.json",
            "--cpu-s",
            "1",
            "--wall-ms",
            "1000",
            "--file-bytes",
            "1",
            "--open-files",
            "8"
        ])
        .code(),
        Some(2),
        "no interpreter after --"
    );
    // Arguments in order, but the report cannot be written: exit 1, which
    // the driver reads as lost.
    let unwritable = [
        "--report",
        "/nonexistent/dir/r.json",
        "--cpu-s",
        "1",
        "--wall-ms",
        "1000",
        "--file-bytes",
        "1000000",
        "--open-files",
        "8",
        "--",
        "/bin/sh",
        "-c",
        "true",
    ];
    assert_eq!(status(&unwritable).code(), Some(1));
}

#[tokio::test]
async fn a_missing_interpreter_is_a_host_error_from_the_shim() {
    let mut c = config(5, Duration::from_secs(10));
    c.interpreter = vec!["/nonexistent/python3".into()];
    let (reply, root) = run_one(driver(c), request("echo hi")).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Host);
    assert!(e.message.contains("/nonexistent/python3"), "{}", e.message);
    assert!(!root.spent.contains_key(&DimKey::ComputeMs));
}

#[tokio::test]
async fn a_shim_that_leaves_no_report_is_lost_and_bills_the_ceiling() {
    // A real process that is not the shim: `sh` rejects the shim's flags
    // and exits without a report. Something ran; the driver cannot say how
    // much, so it bills the ceiling.
    let mut c = config(3, Duration::from_secs(10));
    c.shim = Some("/bin/sh".into());
    let (reply, root) = run_one(driver(c), request("echo hi")).await;
    let Stop::Error(e) = reply.stop else {
        panic!("{:?}", reply.stop)
    };
    assert_eq!(e.kind, ErrorKind::Lost);
    assert!(e.message.contains("no report"), "{}", e.message);
    assert_eq!(
        root.spent.get(&DimKey::ComputeMs),
        Some(&3_000),
        "the ceiling"
    );
    assert!(root.overdraft.is_empty());
}

#[tokio::test]
async fn output_past_the_bound_is_dropped_flagged_and_never_stalls() {
    let mut c = config(5, Duration::from_secs(10));
    c.output_bytes = 1_024;
    // 4 MiB on stdout, far past any pipe buffer, then a clean exit.
    let code = "i=0; while [ $i -lt 65536 ]; do echo 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; i=$((i+1)); done; echo done >&2";
    let (reply, _) = run_one(driver(c), request(code)).await;
    assert_eq!(reply.stop, Stop::Exit(0), "the run finished");
    assert_eq!(reply.stdout.len(), 1_024);
    assert!(reply.truncated.stdout);
    assert_eq!(reply.stderr, "done\n");
    assert!(!reply.truncated.stderr);
}

#[tokio::test]
async fn the_run_sees_only_the_configured_environment_in_a_directory_that_is_gone_after() {
    std::env::set_var("TAU_SANDBOX_TEST_SECRET", "leaked");
    let mut c = config(5, Duration::from_secs(10));
    c.env = vec![("GREETING".into(), "hi".into())];
    let code = "echo \"secret=[$TAU_SANDBOX_TEST_SECRET] greeting=[$GREETING] home=[$HOME]\"; pwd; ls -A | grep -v main.sh | grep -v '^\\.tau-sandbox-report' ; env | wc -l";
    let (reply, _) = run_one(driver(c), request(code)).await;
    assert_eq!(reply.stop, Stop::Exit(0), "{}", reply.stderr);
    let mut lines = reply.stdout.lines();
    let first = lines.next().unwrap();
    assert!(
        first.starts_with("secret=[] greeting=[hi] home=["),
        "{first}"
    );
    let cwd = lines.next().unwrap().to_owned();
    // `pwd` may print the resolved path (`/private/var/...` on macOS) where
    // HOME carries the alias; the unique directory name is what matters.
    let home = first.rsplit("home=[").next().unwrap().trim_end_matches(']');
    assert_eq!(
        std::path::Path::new(home).file_name(),
        std::path::Path::new(&cwd).file_name(),
        "HOME is the scratch dir: {first}"
    );
    assert_ne!(
        std::path::Path::new(&cwd),
        std::env::current_dir().unwrap(),
        "not the harness's directory"
    );
    assert!(
        !std::path::Path::new(&cwd).exists(),
        "the scratch directory is gone after the reply"
    );
    let count: usize = lines.last().unwrap().trim().parse().unwrap();
    // HOME, GREETING, whatever `sh` itself sets (PWD, SHLVL, `_`...), and
    // the profile path under `cargo llvm-cov`.
    assert!(
        count <= 6,
        "the environment is tiny: {count}\n{}",
        reply.stdout
    );
}

#[tokio::test]
async fn a_cancel_abandons_the_run_and_the_interpreter_and_its_children_are_gone() {
    let dir = outside_dir("cancel");
    let pid_file = dir.join("pid");
    let gpid_file = dir.join("gpid");
    let code = format!(
        "sleep 30 & echo $! > {}; echo $$ > {}; wait",
        gpid_file.display(),
        pid_file.display()
    );
    let recorded = Recorded::new(sh());
    let probe = recorded.clone();
    let ceiling = recorded.inner.ceiling();

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let cap = kernel.register_driver(sh_id(), recorded, ceiling).unwrap();
    let ns = Namespace::from_caps([cap]);
    let payload = request(&code);
    let cancel_now = Arc::new(Notify::new());
    let cancel_signal = Arc::clone(&cancel_now);
    let child_ns = ns.clone();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(cap, &payload).unwrap();
                            let _ = child.recv(Match::Corr(corr)).await;
                            child.exit(b"unreachable")
                        }),
                        child_ns,
                        Budget::from_dims([(DimKey::ComputeMs, 10_000), (DimKey::Calls, 1)]),
                    )
                    .unwrap();
                cancel_signal.notified().await;
                root.cancel(child, CancelMode::immediate()).unwrap();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.outcome, Outcome::Aborted);
                root.exit(b"")
            }),
            ns,
            Budget::from_dims([
                (DimKey::ComputeMs, 20_000),
                (DimKey::Calls, 2),
                (DimKey::Depth, 1),
            ]),
        )
        .unwrap();

    // The interpreter is running: it wrote its pid and its child's.
    eventually("the run to start", || {
        pid_file.exists() && gpid_file.exists()
    })
    .await;
    let read = |p: &std::path::Path| -> i32 {
        std::fs::read_to_string(p).unwrap().trim().parse().unwrap()
    };
    let (pid, gpid) = (read(&pid_file), read(&gpid_file));
    assert!(alive(pid) && alive(gpid));
    assert_eq!(probe.inner.in_flight(), 1);

    cancel_now.notify_one();
    kernel.drained().await.unwrap();

    // The reply is dead letter at the kernel — the child is gone — but the
    // driver produced it, and it says what happened.
    eventually("the abandoned reply", || probe.replies().len() == 1).await;
    let (_, reply, consumed) = probe.replies().remove(0);
    assert_eq!(reply.stop, Stop::Abandoned);
    assert!(
        consumed.get(&DimKey::ComputeMs).is_some(),
        "the CPU it burned is real: {consumed:?}"
    );
    eventually("the interpreter and its child to be gone", || {
        !alive(pid) && !alive(gpid)
    })
    .await;
    assert_eq!(probe.inner.in_flight(), 0);
    kernel.shutdown();

    let state = kernel.state();
    let child = state
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    assert_eq!(state.agent(child).unwrap().status, Status::Aborted);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn an_abandon_before_the_run_starts_answers_abandoned_and_spawns_nothing() {
    let driver = sh();
    let corr = Corr::new(7);
    driver.abandon(corr);
    let (bytes, consumed) = driver
        .handle(Delivery {
            corr,
            from: AgentId::new(1),
            payload: request("echo hi"),
        })
        .await;
    let reply: tau_drivers::sandbox::wire::Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Abandoned);
    assert_eq!(reply.stdout, "");
    assert!(consumed.get(&DimKey::ComputeMs).is_none(), "nothing billed");
    assert_eq!(driver.in_flight(), 0);
}

#[tokio::test]
async fn dropping_the_driver_mid_run_leaves_no_process() {
    let dir = outside_dir("drop");
    let pid_file = dir.join("pid");
    let code = format!("echo $$ > {}; sleep 30", pid_file.display());
    let driver = sh();
    let fut = driver.handle(Delivery {
        corr: Corr::new(1),
        from: AgentId::new(1),
        payload: request(&code),
    });
    let run = tokio::spawn(fut);
    eventually("the run to start", || pid_file.exists()).await;
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));

    drop(driver);

    let (bytes, _) = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("the run ended once the driver was dropped")
        .unwrap();
    let reply: tau_drivers::sandbox::wire::Reply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.stop, Stop::Abandoned);
    eventually("the interpreter to be gone", || !alive(pid)).await;
    let _ = std::fs::remove_dir_all(dir);
}
