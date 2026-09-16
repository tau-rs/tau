//! `tau-sandbox-shim`: the supervisor the sandbox driver spawns for every
//! run (ADR-0009 §4).
//!
//! It limits *itself* with `setrlimit`, spawns the interpreter into a
//! process group of its own — so the limits are inherited and the whole
//! tree can be signalled — and waits. The wall bound is a `recv_timeout` on
//! a channel fed by the thread that waits on the child: a duration, never a
//! clock read. A `SIGTERM` from the driver's `abandon`, the wall bound, or
//! the interpreter's own exit each end the same way: `SIGKILL` to the
//! interpreter's group, reap, `getrusage(RUSAGE_CHILDREN)`, and the report
//! written to the path the driver named. The shim writes nothing to its
//! stdout or stderr; those pipes belong to the interpreter.
//!
//! No `tokio`, no dependency on the library or on the kernel crate: the
//! report shape is compiled in by path. Any exit other than `0`, or a
//! missing report, is `error.lost` at the driver.
//!
//! ```text
//! tau-sandbox-shim --report <path> --cpu-s <n> --wall-ms <n> --file-bytes <n>
//!                  --open-files <n> [--memory-bytes <n>] -- <argv>...
//! ```

#[cfg(unix)]
#[path = "../sandbox/report.rs"]
#[allow(dead_code)]
mod report;

#[cfg(unix)]
mod shim {

    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::PathBuf;
    use std::process::{Command, ExitCode, ExitStatus};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::report::{End, Outcome, RawUsage, Report};
    use nix::sys::resource::{getrusage, setrlimit, Resource, UsageWho};
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    use signal_hook::consts::SIGTERM;
    use signal_hook::iterator::Signals;

    /// What the driver asked for, parsed from the command line.
    struct Args {
        report: PathBuf,
        cpu_s: u64,
        wall: Duration,
        file_bytes: u64,
        open_files: u64,
        memory_bytes: Option<u64>,
        argv: Vec<String>,
    }

    /// One of the things the main thread waits for.
    enum Event {
        /// The interpreter is gone; here is how it went.
        Exited(std::io::Result<ExitStatus>),
        /// The driver said stop.
        Term,
    }

    fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut report = None;
        let mut cpu_s = None;
        let mut wall_ms = None;
        let mut file_bytes = None;
        let mut open_files = None;
        let mut memory_bytes = None;
        let mut argv = Vec::new();
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            if flag == "--" {
                argv.extend(args);
                break;
            }
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            let number = || {
                value
                    .parse::<u64>()
                    .map_err(|e| format!("{flag} {value}: {e}"))
            };
            match flag.as_str() {
                "--report" => report = Some(PathBuf::from(&value)),
                "--cpu-s" => cpu_s = Some(number()?),
                "--wall-ms" => wall_ms = Some(number()?),
                "--file-bytes" => file_bytes = Some(number()?),
                "--open-files" => open_files = Some(number()?),
                "--memory-bytes" => memory_bytes = Some(number()?),
                other => return Err(format!("unknown flag {other}")),
            }
        }
        if argv.is_empty() {
            return Err("no interpreter after --".to_owned());
        }
        Ok(Args {
            report: report.ok_or("--report is required")?,
            cpu_s: cpu_s.ok_or("--cpu-s is required")?,
            wall: Duration::from_millis(wall_ms.ok_or("--wall-ms is required")?),
            file_bytes: file_bytes.ok_or("--file-bytes is required")?,
            open_files: open_files.ok_or("--open-files is required")?,
            memory_bytes,
            argv,
        })
    }

    /// Limits on this process, inherited by everything it spawns. Soft and
    /// hard alike, so nothing below can raise them.
    fn fence(args: &Args) -> Result<(), String> {
        let set = |resource: Resource, limit: u64| {
            setrlimit(resource, limit, limit).map_err(|e| format!("setrlimit({resource:?}): {e}"))
        };
        set(Resource::RLIMIT_CPU, args.cpu_s)?;
        set(Resource::RLIMIT_FSIZE, args.file_bytes)?;
        set(Resource::RLIMIT_NOFILE, args.open_files)?;
        set(Resource::RLIMIT_CORE, 0)?;
        if let Some(bytes) = args.memory_bytes {
            set(Resource::RLIMIT_AS, bytes)?;
        }
        Ok(())
    }

    fn usage() -> RawUsage {
        let Ok(u) = getrusage(UsageWho::RUSAGE_CHILDREN) else {
            return RawUsage::default();
        };
        let micros = |t: nix::sys::time::TimeVal| {
            let secs = u64::try_from(t.tv_sec()).unwrap_or(0);
            let micros = u64::try_from(t.tv_usec()).unwrap_or(0);
            secs.saturating_mul(1_000_000).saturating_add(micros)
        };
        let max_rss = u64::try_from(u.max_rss()).unwrap_or(0);
        // Linux reports `ru_maxrss` in kibibytes; the BSDs and macOS in bytes.
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        let max_rss_bytes = max_rss;
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
        let max_rss_bytes = max_rss.saturating_mul(1024);
        RawUsage {
            cpu_user_us: micros(u.user_time()),
            cpu_sys_us: micros(u.system_time()),
            max_rss_bytes,
        }
    }

    fn signal_name(raw: i32) -> String {
        Signal::try_from(raw)
            .map(|s| s.as_ref().to_owned())
            .unwrap_or_else(|_| format!("SIG{raw}"))
    }

    /// Whether `status`, with `used` CPU, is the host kernel enforcing
    /// `RLIMIT_CPU`: `SIGXCPU` is unambiguous; a `SIGKILL` the shim did not
    /// send, with the CPU near the limit, is the hard limit on Linux, which
    /// wins over `SIGXCPU` when soft and hard are equal.
    ///
    /// "Near": Linux checks the limit against tick-sampled CPU time and
    /// `getrusage` reports scheduler runtime, and on a virtualised host the
    /// two drift by more than a few ticks. Nothing else in the fence sends
    /// `SIGKILL`, so the check only has to tell the limit from a kill by a
    /// third party, and a quarter of the limit does that.
    fn hit_cpu_limit(status: ExitStatus, used: RawUsage, cpu_s: u64) -> bool {
        let Some(sig) = status.signal() else {
            return false;
        };
        if sig == Signal::SIGXCPU as i32 {
            return true;
        }
        if sig != Signal::SIGKILL as i32 {
            return false;
        }
        let total = used.cpu_user_us.saturating_add(used.cpu_sys_us);
        let limit = cpu_s.saturating_mul(1_000_000);
        total.saturating_mul(4) >= limit.saturating_mul(3)
    }

    fn classify(status: ExitStatus, used: RawUsage, cpu_s: u64) -> End {
        if let Some(code) = status.code() {
            return End::Exit(code);
        }
        if hit_cpu_limit(status, used, cpu_s) {
            return End::CpuLimit;
        }
        match status.signal() {
            Some(sig) => End::Signal(signal_name(sig)),
            None => End::Signal("unknown".to_owned()),
        }
    }

    fn run(args: &Args) -> Result<(), String> {
        fence(args)?;

        let (tx, rx) = mpsc::channel::<Event>();
        // SIGTERM from the driver becomes an event on the same channel the wait
        // thread feeds, so the main thread has exactly one thing to block on.
        let mut signals = Signals::new([SIGTERM]).map_err(|e| format!("signal handler: {e}"))?;
        let term_tx = tx.clone();
        std::thread::spawn(move || {
            for _ in signals.forever() {
                let _ = term_tx.send(Event::Term);
            }
        });

        let (program, rest) = args.argv.split_first().ok_or("no interpreter after --")?;
        let spawned = Command::new(program).args(rest).process_group(0).spawn();
        let child = match spawned {
            Ok(child) => child,
            Err(e) => {
                // Nothing ran. Tell the driver why, in the report it expects.
                return Report {
                    interpreter_pid: 0,
                    outcome: Some(Outcome {
                        end: End::HostError(format!("cannot start `{program}`: {e}")),
                        usage: RawUsage::default(),
                    }),
                }
                .write(&args.report)
                .map_err(|e| format!("write report: {e}"));
            }
        };
        let pid = child.id();
        let Some(group) = i32::try_from(pid)
            .ok()
            .and_then(|raw| (raw > 0).then_some(Pid::from_raw(raw)))
        else {
            return Err(format!("interpreter pid {pid} is not a valid pgid"));
        };
        // The partial report: the driver's last resort reads the pid from here.
        Report {
            interpreter_pid: pid,
            outcome: None,
        }
        .write(&args.report)
        .map_err(|e| format!("write partial report: {e}"))?;

        let wait_tx = tx;
        std::thread::spawn(move || {
            let mut child = child;
            let _ = wait_tx.send(Event::Exited(child.wait()));
        });

        // The wall bound and the abandon signal, without reading a clock.
        let mut killed_by_shim = None;
        let status = loop {
            match rx.recv_timeout(args.wall) {
                Ok(Event::Exited(status)) => break status,
                Ok(Event::Term) => {
                    let _ = killpg(group, Signal::SIGKILL);
                    killed_by_shim.get_or_insert(End::Abandoned);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = killpg(group, Signal::SIGKILL);
                    killed_by_shim.get_or_insert(End::WallLimit);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the wait thread is gone".to_owned());
                }
            }
        };
        // Whatever the interpreter left behind in its group goes with it.
        let _ = killpg(group, Signal::SIGKILL);

        let status = status.map_err(|e| format!("wait: {e}"))?;
        let used = usage();
        let end = match killed_by_shim {
            Some(end) => end,
            None => classify(status, used, args.cpu_s),
        };
        Report {
            interpreter_pid: pid,
            outcome: Some(Outcome { end, usage: used }),
        }
        .write(&args.report)
        .map_err(|e| format!("write report: {e}"))
    }

    pub(super) fn main() -> ExitCode {
        // RED PROOF (scratch, never merged): a 64 KiB heap block the shim
        // forgets on purpose. `asan+lsan (sandbox)` must go red and name
        // this line in the LSan report (#64).
        std::mem::forget(vec![0xA5u8; 64 * 1024]);
        let args = match parse(std::env::args().skip(1)) {
            Ok(args) => args,
            Err(_) => return ExitCode::from(2),
        };
        match run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        }
    }
}

fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    {
        shim::main()
    }
    #[cfg(not(unix))]
    {
        std::process::ExitCode::FAILURE
    }
}
