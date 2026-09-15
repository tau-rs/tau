//! The sandbox driver: a resource fence behind `send` (ADR-0009).
//!
//! A `send` to a sandbox capability carries a [`wire::Request`] — the code,
//! and what to feed it — and the reply is a [`wire::Reply`]: how the run
//! ended, what it printed, what it used. The driver never runs the code
//! itself. It spawns the `tau-sandbox-shim` binary in a fresh scratch
//! directory with exactly the configured environment, and the shim limits
//! itself with `setrlimit`, spawns the interpreter into a process group of
//! its own, bounds the wall clock, and leaves a report. The driver drains
//! the pipes with a bound, reads the report, settles the reply, deletes the
//! directory.
//!
//! ```text
//! agent ──send(cap, request)──▶ kernel ──Delivery──▶ SandboxDriver
//!                                                        │ spawn, new process group
//!                                                        ▼
//!                                                  tau-sandbox-shim   (setrlimit on self)
//!                                                        │ spawn, new process group
//!                                                        ▼
//!                                                  interpreter  main.py   (untrusted)
//! ```
//!
//! # What it is, and is not
//!
//! v0 is a resource fence, not a security boundary. It bounds CPU, address
//! space (Linux), file size, open files, output, and wall time, contains a
//! crash, scrubs the environment, and gives every run a fresh directory.
//! It does not hide the host filesystem, change the uid, or deny the
//! network: those are the rungs of ADR-0009 §6, each arriving inside the
//! shim behind the same two payloads.
//!
//! # Configuration is the harness's, not the request's
//!
//! One registration is one interpreter under one set of limits
//! ([`SandboxConfig`]); a harness that wants Python and a shell registers
//! two drivers. The model never chooses the interpreter, the environment,
//! or the limits: it chooses the code, and `describe()` tells it the cage
//! it is in.
//!
//! # Accounting
//!
//! The ceiling is one dimension, `compute_ms` = `cpu_seconds × 1000`, and
//! every reply that ran something reports the CPU the shim measured, user
//! plus system, rounded up to the millisecond. `error.unsupported` and
//! `error.host` ran nothing and bill nothing; `error.lost` — the run
//! started and its report never arrived — bills the ceiling, because
//! unknown consumption is billed up, never down. `wall_ms` is not reported:
//! the reducer charges it on every `Tick` the requester waits through.
//!
//! # Cancel
//!
//! `abandon` sends the shim `SIGTERM`; the shim kills the interpreter's
//! group, reaps it, and reports `abandoned` with the real usage. If the
//! report does not arrive within [`SandboxConfig::abandon_grace`], the
//! driver `SIGKILL`s both groups and replies `error.lost`. Dropping the last
//! handle to the driver abandons every open run the same way, so a harness
//! shutting down leaves no interpreter behind.

// Shared by path with the shim binary; each half uses one direction.
#[allow(dead_code)]
mod report;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use tau_kernel::abi::{Budget, Consumption, Corr, DimKey};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{BoxFuture, Delivery};

use report::{End, Report};
use wire::{ErrorKind, Reply, RunError, Stop, Truncated, Usage, VERSION};

/// The name of the supervisor binary, looked for beside the harness
/// executable when [`SandboxConfig::shim`] is not set.
pub const SHIM_NAME: &str = "tau-sandbox-shim";

/// The file in the scratch directory the shim reports through.
const REPORT_FILE: &str = ".tau-sandbox-report.json";

/// Everything the harness decides about one sandbox capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxConfig {
    /// The interpreter and its leading arguments (`["python3"]`,
    /// `["/bin/sh"]`). The entry file is appended. An absolute path is
    /// safest: the run's environment is exactly [`env`](Self::env), so
    /// `PATH` is whatever the harness put there, or nothing.
    pub interpreter: Vec<String>,
    /// The file the code is written to in the scratch directory
    /// (`main.py`). A bare file name, no directory.
    pub entry: String,
    /// `RLIMIT_CPU`, in whole seconds, and the ceiling: `compute_ms` is
    /// this times 1000. At least 1.
    pub cpu_seconds: u32,
    /// `RLIMIT_AS`, the address-space bound. Linux only: refused at
    /// construction on macOS, where XNU accepts the limit and does not
    /// enforce it. Must leave room for the shim itself, which is under it.
    pub memory_bytes: Option<u64>,
    /// The wall bound the shim enforces. Not reported: the reducer charges
    /// the requester's `wall_ms` while it waits.
    pub wall: Duration,
    /// How much of each of stdout and stderr is kept. The rest is drained
    /// and dropped, and the reply says so.
    pub output_bytes: usize,
    /// The largest `code` accepted. Over it is `error.unsupported`.
    pub code_bytes: usize,
    /// `RLIMIT_FSIZE`: the largest file the run may write.
    pub file_bytes: u64,
    /// `RLIMIT_NOFILE`.
    pub open_files: u64,
    /// The run's whole environment. Nothing is inherited from the harness;
    /// `HOME` is the scratch directory unless set here.
    pub env: Vec<(String, String)>,
    /// Where the shim binary is. `None`: [`SHIM_NAME`] beside the current
    /// executable.
    pub shim: Option<PathBuf>,
    /// Where scratch directories are made. `None`: the system temporary
    /// directory.
    pub scratch_root: Option<PathBuf>,
    /// How long after `SIGTERM` the driver waits for the shim's report
    /// before killing both process groups and replying `error.lost`.
    pub abandon_grace: Duration,
    /// The opening sentence of `describe()`. `None`: "Run code with
    /// `<interpreter>`." The limits are always appended; the harness may
    /// replace the sentence, not remove them.
    pub description: Option<String>,
}

impl SandboxConfig {
    /// A config with the defaults of ADR-0009 §7: 64 KiB of code and of
    /// each output stream, 16 MiB per file, 64 open files, no memory bound,
    /// an empty environment, the shim beside the executable, a one-second
    /// abandon grace.
    #[must_use]
    pub fn new(
        interpreter: impl IntoIterator<Item = impl Into<String>>,
        entry: impl Into<String>,
        cpu_seconds: u32,
        wall: Duration,
    ) -> Self {
        Self {
            interpreter: interpreter.into_iter().map(Into::into).collect(),
            entry: entry.into(),
            cpu_seconds,
            memory_bytes: None,
            wall,
            output_bytes: 64 * 1024,
            code_bytes: 64 * 1024,
            file_bytes: 16 * 1024 * 1024,
            open_files: 64,
            env: Vec::new(),
            shim: None,
            scratch_root: None,
            abandon_grace: Duration::from_secs(1),
            description: None,
        }
    }

    /// The registration ceiling (ADR-0009 §3): `compute_ms` =
    /// `cpu_seconds × 1000`.
    #[must_use]
    pub fn ceiling(&self) -> Budget {
        Budget::from_dims([(DimKey::ComputeMs, u64::from(self.cpu_seconds) * 1_000)])
    }

    /// What the model reads (ADR-0009 §7): the harness's sentence, then
    /// how the code runs and under what limits.
    #[must_use]
    pub fn describe(&self) -> String {
        let interpreter = self.interpreter.join(" ");
        let about = self
            .description
            .clone()
            .unwrap_or_else(|| format!("Run code with `{interpreter}`."));
        let memory = self
            .memory_bytes
            .map(|bytes| format!("{} memory, ", bytes_text(bytes)))
            .unwrap_or_default();
        format!(
            "{about} The code is written to {entry} in an empty directory and run with \
             `{interpreter} {entry}`; `stdin` is fed to it. Limits: {cpu} s CPU, {memory}\
             {wall} wall, {out} of stdout and of stderr. No network. Nothing persists \
             between calls.",
            entry = self.entry,
            cpu = self.cpu_seconds,
            wall = duration_text(self.wall),
            out = bytes_text(u64::try_from(self.output_bytes).unwrap_or(u64::MAX)),
        )
    }
}

/// `256 MiB`, `64 KiB`, or plain bytes when not a whole multiple.
fn bytes_text(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes >> 20)
    } else if bytes >= KIB && bytes.is_multiple_of(KIB) {
        format!("{} KiB", bytes >> 10)
    } else {
        format!("{bytes} bytes")
    }
}

/// `10 s`, or `500 ms` when not whole seconds.
fn duration_text(d: Duration) -> String {
    if d.subsec_millis() == 0 && d.subsec_nanos() == 0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// Why a sandbox driver could not be built from its config.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// `interpreter` is empty.
    #[error("interpreter is empty")]
    NoInterpreter,
    /// `entry` is not a bare file name.
    #[error("entry `{entry}` must be a bare file name")]
    BadEntry {
        /// The entry as configured.
        entry: String,
    },
    /// A bound of zero: nothing could run under it.
    #[error("{what} must be at least 1")]
    ZeroBound {
        /// Which bound.
        what: &'static str,
    },
    /// `memory_bytes` is set on a platform that does not enforce
    /// `RLIMIT_AS`. Refused rather than set and ignored.
    #[error("memory_bytes is not enforced on {platform}; leave it unset")]
    MemoryBoundUnsupported {
        /// The platform.
        platform: &'static str,
    },
    /// No `shim` was configured and the current executable's directory
    /// cannot be found.
    #[error("cannot locate the shim beside the current executable: {0}")]
    ShimPath(String),
}

/// The driver. Cheap to clone; every clone shares one table of open runs.
/// Dropping the last clone abandons every open run.
#[derive(Clone)]
pub struct SandboxDriver {
    inner: Arc<Inner>,
}

struct Inner {
    settings: Arc<Settings>,
    ceiling: Budget,
    schema: ToolSchema,
    runs: Arc<Runs>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.runs.abandon_all();
    }
}

/// What a run thread needs from the config, resolved once.
struct Settings {
    config: SandboxConfig,
    shim: PathBuf,
    scratch_root: PathBuf,
    ceiling_ms: u64,
}

impl SandboxDriver {
    /// Builds the driver, validating the config.
    ///
    /// # Errors
    ///
    /// [`ConfigError`], as described on each variant.
    pub fn new(config: SandboxConfig) -> Result<Self, ConfigError> {
        if config.interpreter.is_empty() {
            return Err(ConfigError::NoInterpreter);
        }
        if config.entry.is_empty() || Path::new(&config.entry).components().count() != 1 {
            return Err(ConfigError::BadEntry {
                entry: config.entry.clone(),
            });
        }
        let nonzero =
            |ok: bool, what: &'static str| ok.then_some(()).ok_or(ConfigError::ZeroBound { what });
        nonzero(config.cpu_seconds >= 1, "cpu_seconds")?;
        nonzero(!config.wall.is_zero(), "wall")?;
        nonzero(config.code_bytes >= 1, "code_bytes")?;
        nonzero(config.file_bytes >= 1, "file_bytes")?;
        nonzero(config.open_files >= 1, "open_files")?;
        if config.memory_bytes.is_some() && !cfg!(target_os = "linux") {
            return Err(ConfigError::MemoryBoundUnsupported {
                platform: std::env::consts::OS,
            });
        }
        let shim = match &config.shim {
            Some(path) => path.clone(),
            None => std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .map(|dir| dir.join(SHIM_NAME))
                .ok_or_else(|| ConfigError::ShimPath("no current executable".to_owned()))?,
        };
        let scratch_root = config
            .scratch_root
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        let ceiling = config.ceiling();
        let ceiling_ms = ceiling.get(&DimKey::ComputeMs).unwrap_or(0);
        let schema = ToolSchema {
            description: config.describe(),
            input_schema: serde_json::to_vec(&wire::schema()).unwrap_or_default(),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                settings: Arc::new(Settings {
                    config,
                    shim,
                    scratch_root,
                    ceiling_ms,
                }),
                ceiling,
                schema,
                runs: Arc::new(Runs::default()),
            }),
        })
    }

    /// The ceiling to register this driver with. See
    /// [`SandboxConfig::ceiling`].
    #[must_use]
    pub fn ceiling(&self) -> Budget {
        self.inner.ceiling.clone()
    }

    /// The config this driver was built from.
    #[must_use]
    pub fn config(&self) -> &SandboxConfig {
        &self.inner.settings.config
    }

    /// Where the shim is expected.
    #[must_use]
    pub fn shim_path(&self) -> &Path {
        &self.inner.settings.shim
    }

    /// How many runs are open right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.runs.in_flight()
    }
}

impl fmt::Debug for SandboxDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxDriver")
            .field("config", &self.inner.settings.config)
            .field("shim", &self.inner.settings.shim)
            .field("in_flight", &self.in_flight())
            .finish_non_exhaustive()
    }
}

impl Driver for SandboxDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        // Registered now, not when the future is first polled: `abandon`
        // may arrive in between, and it must find the entry.
        let (flight, abandoned_early) = self.inner.runs.enter(request.corr);
        if abandoned_early {
            drop(flight);
            let reply = plain(Stop::Abandoned);
            return Box::pin(async move { (encode(&reply), Consumption::none()) });
        }
        let settings = Arc::clone(&self.inner.settings);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("tau-sandbox".to_owned())
            .spawn(move || {
                let _flight = flight;
                let (reply, consumed) = run(&settings, &_flight.slot, &request.payload);
                let _ = tx.send((encode(&reply), consumed));
            });
        match spawned {
            Ok(_) => Box::pin(async move {
                rx.await.unwrap_or_else(|_| {
                    let reply = refuse(ErrorKind::Host, "the run thread ended without a reply");
                    (encode(&reply), Consumption::none())
                })
            }),
            Err(e) => {
                let reply = refuse(ErrorKind::Host, &format!("cannot start a run thread: {e}"));
                Box::pin(async move { (encode(&reply), Consumption::none()) })
            }
        }
    }

    fn describe(&self) -> Option<ToolSchema> {
        Some(self.inner.schema.clone())
    }

    fn abandon(&self, corr: Corr) {
        self.inner.runs.abandon(corr);
    }
}

// --- the registry -----------------------------------------------------------

/// The runs `abandon` can reach: `corr → slot`, and the corrs abandoned
/// before the driver saw them (the model drivers' `abandoned_early`).
#[derive(Default)]
struct Runs {
    state: Mutex<RunsState>,
}

#[derive(Default)]
struct RunsState {
    open: BTreeMap<Corr, Arc<Slot>>,
    abandoned_early: BTreeSet<Corr>,
}

/// One run's signal path: whether it was abandoned, and the channel to its
/// thread once the thread is listening.
#[derive(Default)]
struct Slot {
    state: Mutex<SlotState>,
}

#[derive(Default)]
struct SlotState {
    abandoned: bool,
    tx: Option<mpsc::Sender<Event>>,
}

impl Slot {
    fn abandon(&self) {
        let mut state = lock(&self.state);
        state.abandoned = true;
        if let Some(tx) = &state.tx {
            let _ = tx.send(Event::Abandon);
        }
    }

    /// Starts listening; returns whether an abandon already arrived.
    fn listen(&self, tx: mpsc::Sender<Event>) -> bool {
        let mut state = lock(&self.state);
        state.tx = Some(tx);
        state.abandoned
    }
}

/// One run's registration, removed when the run ends however it ends.
struct Flight {
    runs: Arc<Runs>,
    corr: Corr,
    slot: Arc<Slot>,
}

impl Drop for Flight {
    fn drop(&mut self) {
        lock(&self.runs.state).open.remove(&self.corr);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Runs {
    fn enter(self: &Arc<Self>, corr: Corr) -> (Flight, bool) {
        let mut state = lock(&self.state);
        let early = state.abandoned_early.remove(&corr);
        let slot = Arc::new(Slot::default());
        state.open.insert(corr, Arc::clone(&slot));
        drop(state);
        (
            Flight {
                runs: Arc::clone(self),
                corr,
                slot,
            },
            early,
        )
    }

    fn abandon(&self, corr: Corr) {
        let mut state = lock(&self.state);
        match state.open.get(&corr) {
            Some(slot) => slot.abandon(),
            None => {
                state.abandoned_early.insert(corr);
            }
        }
    }

    fn abandon_all(&self) {
        let state = lock(&self.state);
        for slot in state.open.values() {
            slot.abandon();
        }
    }

    fn in_flight(&self) -> usize {
        lock(&self.state).open.len()
    }
}

// --- one run ----------------------------------------------------------------

/// What the run thread waits for.
enum Event {
    /// `abandon` was called for this run.
    Abandon,
    /// The shim is gone; here is how it went.
    Exited(std::io::Result<ExitStatus>),
    /// One output pipe reached end of file.
    Drained,
}

/// One output stream, kept to the bound.
#[derive(Default)]
struct Stream {
    data: Vec<u8>,
    truncated: bool,
}

/// Scratch directories are named uniquely per process by this counter.
static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

fn run(settings: &Settings, slot: &Slot, payload: &[u8]) -> (Reply, Consumption) {
    let config = &settings.config;
    let request: wire::Request = match serde_json::from_slice(payload) {
        Ok(request) => request,
        Err(e) => {
            return none(refuse(
                ErrorKind::Unsupported,
                &format!("payload is not a sandbox v{VERSION} request: {e}"),
            ))
        }
    };
    if let Some(v) = request.v {
        if v != VERSION {
            return none(refuse(
                ErrorKind::Unsupported,
                &format!("sandbox version {v} is not supported; this driver speaks v{VERSION}"),
            ));
        }
    }
    if request.code.len() > config.code_bytes {
        return none(refuse(
            ErrorKind::Unsupported,
            &format!(
                "code is {} bytes; the bound is {}",
                request.code.len(),
                config.code_bytes
            ),
        ));
    }

    let scratch = Scratch::create(&settings.scratch_root);
    let scratch = match scratch {
        Ok(scratch) => scratch,
        Err(e) => return none(refuse(ErrorKind::Host, &format!("scratch directory: {e}"))),
    };
    if let Err(e) = std::fs::write(scratch.dir.join(&config.entry), &request.code) {
        return none(refuse(
            ErrorKind::Host,
            &format!("write {}: {e}", config.entry),
        ));
    }
    let report_path = scratch.dir.join(REPORT_FILE);

    let mut command = Command::new(&settings.shim);
    command
        .arg("--report")
        .arg(&report_path)
        .arg("--cpu-s")
        .arg(config.cpu_seconds.to_string())
        .arg("--wall-ms")
        .arg(config.wall.as_millis().to_string())
        .arg("--file-bytes")
        .arg(config.file_bytes.to_string())
        .arg("--open-files")
        .arg(config.open_files.to_string());
    if let Some(bytes) = config.memory_bytes {
        command.arg("--memory-bytes").arg(bytes.to_string());
    }
    command
        .arg("--")
        .args(&config.interpreter)
        .arg(&config.entry)
        .current_dir(&scratch.dir)
        .env_clear()
        .env("HOME", &scratch.dir)
        .envs(config.env.iter().map(|(k, v)| (k, v)))
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return none(refuse(
                ErrorKind::Host,
                &format!("cannot start {}: {e}", settings.shim.display()),
            ))
        }
    };
    let shim_pid = child.id();

    // The pipes: stdin written and closed, both outputs drained to the end
    // so a chatty run never stalls, keeping the first `output_bytes`.
    let (tx, rx) = mpsc::channel::<Event>();
    if let Some(mut stdin) = child.stdin.take() {
        let input = request.stdin.clone().unwrap_or_default();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let stdout = Arc::new(Mutex::new(Stream::default()));
    let stderr = Arc::new(Mutex::new(Stream::default()));
    let mut drains = 0;
    if let Some(pipe) = child.stdout.take() {
        drains += 1;
        drain(pipe, Arc::clone(&stdout), config.output_bytes, tx.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        drains += 1;
        drain(pipe, Arc::clone(&stderr), config.output_bytes, tx.clone());
    }
    let wait_tx = tx.clone();
    std::thread::spawn(move || {
        let _ = wait_tx.send(Event::Exited(child.wait()));
    });

    // Now the run can be abandoned; an abandon that came first is honoured.
    if slot.listen(tx) {
        signal(shim_pid, Signal::SIGTERM);
    }

    // The wait, without a clock: the shim bounds the wall itself, so a shim
    // that is still there past `wall + abandon_grace` is a shim in trouble.
    let grace = config.abandon_grace;
    let mut deadline = config.wall.saturating_add(grace);
    let mut status = None;
    let mut drained = 0;
    let mut last_resort = false;
    while status.is_none() || drained < drains {
        match rx.recv_timeout(deadline) {
            Ok(Event::Exited(result)) => {
                status = Some(result);
                // Whatever the interpreter left in its group goes with it,
                // which is also what closes the pipes.
                if let Some(pid) = interpreter_pid(&report_path) {
                    signal_group(pid, Signal::SIGKILL);
                }
                deadline = grace;
            }
            Ok(Event::Drained) => drained += 1,
            // Only while the shim is still ours to signal: once reaped, its
            // pid may belong to someone else.
            Ok(Event::Abandon) if status.is_none() => {
                signal(shim_pid, Signal::SIGTERM);
                deadline = grace;
            }
            Ok(Event::Abandon) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if status.is_none() && !last_resort => {
                last_resort = true;
                signal_group(shim_pid, Signal::SIGKILL);
                if let Some(pid) = interpreter_pid(&report_path) {
                    signal_group(pid, Signal::SIGKILL);
                }
                deadline = grace;
            }
            // Past the last resort with no exit, or the pipes held open by
            // something that escaped its group: take what there is.
            Err(_) => break,
        }
    }

    let report = Report::read(&report_path).ok();
    let shim_ok = matches!(&status, Some(Ok(status)) if status.success());
    let (stop, consumed) = match (shim_ok, report.and_then(|r| r.outcome)) {
        (true, Some(outcome)) => {
            let usage = Usage {
                cpu_user_us: outcome.usage.cpu_user_us,
                cpu_sys_us: outcome.usage.cpu_sys_us,
                max_rss_bytes: outcome.usage.max_rss_bytes,
            };
            let stop = match outcome.end {
                End::Exit(code) => Stop::Exit(code),
                End::Signal(name) => Stop::Signal(name),
                End::CpuLimit => Stop::CpuLimit,
                End::WallLimit => Stop::WallLimit,
                End::Abandoned => Stop::Abandoned,
                End::HostError(message) => {
                    return none(refuse(ErrorKind::Host, &message));
                }
            };
            (
                (stop, usage),
                Consumption::from_dims([(DimKey::ComputeMs, usage.compute_ms())]),
            )
        }
        _ => {
            let how = match &status {
                None => "the shim did not exit in time".to_owned(),
                Some(Ok(status)) => format!("the shim ended with {status} and no report"),
                Some(Err(e)) => format!("waiting on the shim failed: {e}"),
            };
            let stop = Stop::Error(RunError {
                kind: ErrorKind::Lost,
                message: format!("{how}; billed at the ceiling"),
            });
            (
                (stop, Usage::default()),
                Consumption::from_dims([(DimKey::ComputeMs, settings.ceiling_ms)]),
            )
        }
    };
    let out = take(&stdout);
    let err = take(&stderr);
    let reply = Reply {
        v: VERSION,
        stop: stop.0,
        stdout: String::from_utf8_lossy(&out.data).into_owned(),
        stderr: String::from_utf8_lossy(&err.data).into_owned(),
        truncated: Truncated {
            stdout: out.truncated,
            stderr: err.truncated,
        },
        usage: stop.1,
    };
    drop(scratch);
    (reply, consumed)
}

/// A fresh directory, removed on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn create(root: &Path) -> std::io::Result<Self> {
        let seq = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("tau-sandbox-{}-{seq}", std::process::id()));
        std::fs::create_dir(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self { dir })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Reads `pipe` to the end on a thread, keeping the first `bound` bytes.
fn drain(
    mut pipe: impl Read + Send + 'static,
    into: Arc<Mutex<Stream>>,
    bound: usize,
    tx: mpsc::Sender<Event>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let n = match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut stream = lock(&into);
            let room = bound.saturating_sub(stream.data.len());
            let keep = n.min(room);
            stream
                .data
                .extend_from_slice(buf.get(..keep).unwrap_or_default());
            if keep < n {
                stream.truncated = true;
            }
        }
        let _ = tx.send(Event::Drained);
    });
}

fn take(stream: &Mutex<Stream>) -> Stream {
    std::mem::take(&mut *lock(stream))
}

/// The interpreter's pid from the shim's partial report, if it wrote one.
fn interpreter_pid(report: &Path) -> Option<u32> {
    Report::read(report)
        .ok()
        .map(|r| r.interpreter_pid)
        .filter(|&pid| pid > 0)
}

fn pid(raw: u32) -> Option<Pid> {
    i32::try_from(raw)
        .ok()
        .filter(|&p| p > 0)
        .map(Pid::from_raw)
}

fn signal(raw: u32, sig: Signal) {
    if let Some(pid) = pid(raw) {
        let _ = kill(pid, sig);
    }
}

fn signal_group(raw: u32, sig: Signal) {
    if let Some(pid) = pid(raw) {
        let _ = killpg(pid, sig);
    }
}

/// A reply that ran nothing.
fn plain(stop: Stop) -> Reply {
    Reply {
        v: VERSION,
        stop,
        stdout: String::new(),
        stderr: String::new(),
        truncated: Truncated::default(),
        usage: Usage::default(),
    }
}

fn refuse(kind: ErrorKind, message: &str) -> Reply {
    plain(Stop::Error(RunError {
        kind,
        message: message.to_owned(),
    }))
}

fn none(reply: Reply) -> (Reply, Consumption) {
    (reply, Consumption::none())
}

/// A reply, as bytes. Serializing these types cannot fail in practice; if
/// it ever does, the reply says so instead of being empty.
fn encode(reply: &Reply) -> Vec<u8> {
    serde_json::to_vec(reply).unwrap_or_else(|_| {
        format!(
            r#"{{"v":{VERSION},"stop":{{"error":{{"kind":"host","message":"reply could not be serialized"}}}},"stdout":"","stderr":"","truncated":{{"stdout":false,"stderr":false}},"usage":{{"cpu_user_us":0,"cpu_sys_us":0,"max_rss_bytes":0}}}}"#
        )
        .into_bytes()
    })
}
