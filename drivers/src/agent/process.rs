//! Subprocess supervision for the agent drivers (ADR-0013 §5): spawn with
//! an exact environment, drain the output with a bound, and climb one
//! cancel ladder.
//!
//! This module knows nothing about any CLI. Everything CLI-specific travels
//! in an [`Invocation`] — the argv, the first stdin message, how to
//! interrupt, and which printed line means "finished" — and everything the
//! run produced comes back in a [`Run`]. That is the seam the two adapter
//! lanes fill in.
//!
//! # The ladder
//!
//! ```text
//!   abandon, or the wall bound
//!        │
//!        ├─▶ rung 1  interrupt   in-band bytes on stdin, or SIGINT to the group
//!        │           wait `grace`
//!        ├─▶ rung 2  SIGTERM to the group
//!        │           wait `grace`
//!        └─▶ rung 3  SIGKILL to the group
//! ```
//!
//! The rung that was reached is in the [`Run`], because it is what decides
//! the bill: a CLI that answered the interrupt reported its own usage, and
//! one that needed a signal reported nothing, so the run is billed at the
//! ceiling (ADR-0013 §5). Unknown consumption is billed *up*, never down.
//!
//! Every signal goes to the **process group**, never to the pid: these CLIs
//! spawn shells, test runners and sub-agents of their own, and a `kill(pid)`
//! would orphan them. A grandchild that calls `setpgid` escapes the group;
//! that is ADR-0009 §6's row, unchanged here.
//!
//! # No clock
//!
//! `Instant::now` is denied workspace-wide and nothing here reads one. Time
//! is bounded the way the sandbox bounds it: a channel and a duration.

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde_json::Value;

/// How to reach a running child for the first rung of the ladder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Interrupt {
    /// Write these bytes on the child's stdin and wait: the CLI advertises
    /// an in-band interrupt and acknowledges it (`claude`).
    InBand(String),
    /// Send `SIGINT` to the process group: the CLI has no in-band channel
    /// (`codex exec`).
    Signal,
}

/// What the caller bounds a run with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// The whole run. Reaching it climbs the ladder, reporting
    /// `{ "limit": "wall" }`.
    pub wall: Duration,
    /// How long each rung of the ladder is given before the next.
    pub grace: Duration,
    /// How much of the output stream is kept. The terminal line is always
    /// kept, whatever the bound.
    pub transcript_bytes: usize,
    /// How much of stderr is kept.
    pub stderr_bytes: usize,
}

/// Everything CLI-specific about one run. The adapter fills this in; this
/// module reads it and never calls back.
pub struct Invocation {
    /// The binary.
    pub program: PathBuf,
    /// Its arguments, already assembled by the adapter.
    pub args: Vec<String>,
    /// The child's working directory.
    pub cwd: PathBuf,
    /// The child's **whole** environment. Nothing is inherited: the child
    /// gets exactly what the harness listed at registration, which is where
    /// `HOME` comes from — the CLI finding its own login under it is the
    /// CLI reading its own files, not the driver reading them
    /// (ADR-0013 §7, §8).
    pub env: Vec<(String, String)>,
    /// Written to stdin once, if the CLI takes its task that way. stdin
    /// stays open afterwards, for the interrupt, and is closed when the
    /// terminal line arrives.
    pub first_stdin: Option<String>,
    /// The first rung of the ladder.
    pub interrupt: Interrupt,
    /// Whether a printed line is the CLI's terminal event. Called on every
    /// line, so it must be cheap; a `fn` pointer rather than a closure,
    /// because it crosses onto the drain thread.
    pub terminal: fn(&[u8]) -> bool,
}

/// Which rung of the ladder was reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    /// The in-band interrupt, or `SIGINT`.
    Interrupt,
    /// `SIGTERM` to the group.
    Term,
    /// `SIGKILL` to the group.
    Kill,
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The child exited on its own, or its output ended and it was reaped.
    Completed,
    /// The requester was cancelled; the driver climbed to this rung.
    Abandoned(Rung),
    /// The wall bound was reached; the driver climbed to this rung.
    WallLimit(Rung),
}

impl Ending {
    /// The rung reached, if the driver had to stop the child.
    #[must_use]
    pub fn rung(self) -> Option<Rung> {
        match self {
            Self::Completed => None,
            Self::Abandoned(rung) | Self::WallLimit(rung) => Some(rung),
        }
    }
}

/// What one run produced.
#[derive(Clone, Debug)]
pub struct Run {
    /// The stdout lines that were kept, in order, without their newlines.
    pub lines: Vec<Vec<u8>>,
    /// How many lines were dropped at the bound.
    pub dropped: usize,
    /// Whether any line was dropped.
    pub truncated: bool,
    /// The first `stderr_bytes` of standard error, as UTF-8, lossily.
    pub stderr: String,
    /// How the run ended.
    pub ending: Ending,
    /// Where the CLI's terminal line is in [`lines`](Self::lines), if it
    /// arrived. Not necessarily the last: #130 run 1 printed a
    /// `session_state_changed` after its `result`.
    pub terminal_at: Option<usize>,
    /// The child's exit status, if it was reaped.
    pub status: Option<ExitStatus>,
}

impl Run {
    /// The exit code, if the child exited rather than being signalled.
    #[must_use]
    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|status| status.code())
    }

    /// The kept lines as JSON values, one per line, parsed and unread. A
    /// line that is not JSON is carried as `{ "raw": "…" }`, so a CLI that
    /// prints a stray warning does not cost the caller the transcript.
    #[must_use]
    pub fn transcript(&self) -> Vec<Value> {
        self.lines
            .iter()
            .map(|line| {
                serde_json::from_slice(line).unwrap_or_else(|_| {
                    let mut raw = serde_json::Map::new();
                    raw.insert(
                        "raw".to_owned(),
                        Value::String(String::from_utf8_lossy(line).into_owned()),
                    );
                    Value::Object(raw)
                })
            })
            .collect()
    }

    /// Whether the CLI's own usage figures can be trusted for the bill
    /// (ADR-0013 §5): the terminal event arrived, and the driver did not
    /// have to go past the first rung to get it.
    ///
    /// When this is false the run is billed at the ceiling, because the
    /// turn the CLI was inside is spent at the provider and reported by
    /// nobody, and a driver that guessed low would make a cancel cheaper
    /// than a completion.
    #[must_use]
    pub fn usage_is_reported(&self) -> bool {
        self.terminal_seen()
            && self
                .ending
                .rung()
                .is_none_or(|rung| rung == Rung::Interrupt)
    }

    /// Whether the CLI's terminal line ever arrived.
    #[must_use]
    pub fn terminal_seen(&self) -> bool {
        self.terminal_at.is_some()
    }

    /// The line the CLI's terminal event arrived on, if it did. This is
    /// where an adapter reads the usage the bill is made of.
    #[must_use]
    pub fn terminal_line(&self) -> Option<&[u8]> {
        self.terminal_at
            .and_then(|at| self.lines.get(at))
            .map(Vec::as_slice)
    }
}

/// The cancel signal for one run: what `Driver::abandon` reaches, and what
/// the run thread listens on.
///
/// Separate from the run itself because `abandon` arrives on another thread
/// — possibly before [`run`] has spawned anything, which is why
/// [`Cancel::abandon`] is remembered rather than dropped.
#[derive(Debug, Default)]
pub struct Cancel {
    state: Mutex<CancelState>,
}

#[derive(Default)]
struct CancelState {
    abandoned: bool,
    tx: Option<mpsc::Sender<Event>>,
}

impl std::fmt::Debug for CancelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelState")
            .field("abandoned", &self.abandoned)
            .finish_non_exhaustive()
    }
}

impl Cancel {
    /// Stop the run: start the ladder now, or as soon as it starts.
    pub fn abandon(&self) {
        let mut state = lock(&self.state);
        state.abandoned = true;
        if let Some(tx) = &state.tx {
            let _ = tx.send(Event::Abandon);
        }
    }

    /// Whether an abandon has already arrived.
    #[must_use]
    pub fn abandoned(&self) -> bool {
        lock(&self.state).abandoned
    }

    /// Starts listening; returns whether an abandon already arrived.
    fn listen(&self, tx: mpsc::Sender<Event>) -> bool {
        let mut state = lock(&self.state);
        state.tx = Some(tx);
        state.abandoned
    }

    /// Stops listening, so a later `abandon` does not write to a channel
    /// whose run is over.
    fn quiet(&self) {
        lock(&self.state).tx = None;
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the supervising loop waits for.
enum Event {
    /// `abandon` was called for this run.
    Abandon,
    /// The child is gone; here is how it went.
    Exited(io::Result<ExitStatus>),
    /// One output pipe reached end of file.
    Drained,
    /// The CLI's terminal line arrived: stdin is no longer needed.
    Terminal,
}

/// Runs one child to its end, or stops it.
///
/// # Errors
///
/// The spawn itself: a missing binary, a `cwd` that is not a directory, a
/// refusal from the host. Nothing ran, so the caller reports `error.host`
/// and bills nothing.
pub fn run(invocation: &Invocation, bounds: Bounds, cancel: &Cancel) -> io::Result<Run> {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .current_dir(&invocation.cwd)
        .env_clear()
        .envs(invocation.env.iter().map(|(k, v)| (k, v)))
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel::<Event>();
    let stdin = Arc::new(Mutex::new(child.stdin.take()));
    if let Some(first) = invocation.first_stdin.clone() {
        // On a thread: a task at the bound is larger than a pipe buffer, and
        // a child that reads its stdin lazily would deadlock the supervisor.
        let stdin = Arc::clone(&stdin);
        drop(
            std::thread::Builder::new()
                .name("tau-agent-stdin".to_owned())
                .spawn(move || {
                    if let Some(pipe) = lock(&stdin).as_mut() {
                        let _ = pipe.write_all(first.as_bytes());
                        let _ = pipe.flush();
                    }
                }),
        );
    }

    let lines = Arc::new(Mutex::new(Lines::new(bounds.transcript_bytes)));
    let stderr = Arc::new(Mutex::new(Bytes::new(bounds.stderr_bytes)));
    let mut drains: u32 = 0;
    if let Some(pipe) = child.stdout.take() {
        drains = drains.saturating_add(1);
        drain_lines(pipe, Arc::clone(&lines), invocation.terminal, tx.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        drains = drains.saturating_add(1);
        drain_bytes(pipe, Arc::clone(&stderr), tx.clone());
    }
    reap(child, tx.clone());

    // Only now can the run be abandoned; an abandon that came first is
    // honoured here rather than lost.
    let abandoned_already = cancel.listen(tx);
    let mut rung = None;
    let mut stopped = None;
    if abandoned_already {
        stopped = Some(Stopped::Abandoned);
        climb(&mut rung, pid, &invocation.interrupt, &stdin);
    }

    let mut status = None;
    let mut drained: u32 = 0;
    let mut deadline = if stopped.is_some() {
        bounds.grace
    } else {
        bounds.wall
    };
    while status.is_none() || drained < drains {
        match rx.recv_timeout(deadline) {
            Ok(Event::Exited(result)) => {
                status = result.ok();
                // Whatever the CLI left in its group goes with it, which is
                // also what closes the pipes a grandchild still holds.
                signal_group(pid, Signal::SIGKILL);
                deadline = bounds.grace;
            }
            Ok(Event::Drained) => drained = drained.saturating_add(1),
            Ok(Event::Terminal) => {
                // The CLI has said its last word; stdin is what a shell
                // would close, and some CLIs wait for the close to exit.
                drop(lock(&stdin).take());
            }
            Ok(Event::Abandon) if status.is_none() => {
                if stopped.is_none() {
                    stopped = Some(Stopped::Abandoned);
                }
                climb(&mut rung, pid, &invocation.interrupt, &stdin);
                deadline = bounds.grace;
            }
            Ok(Event::Abandon) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if status.is_none() => {
                if stopped.is_none() {
                    stopped = Some(Stopped::Wall);
                }
                if !climb(&mut rung, pid, &invocation.interrupt, &stdin) {
                    // Past SIGKILL and still nothing: the child, or a
                    // grandchild that left the group, is beyond us.
                    break;
                }
                deadline = bounds.grace;
            }
            // The child is reaped and the pipes are still held open by
            // something that escaped the group: take what there is.
            Err(_) => break,
        }
    }
    cancel.quiet();
    drop(lock(&stdin).take());

    let kept = std::mem::replace(&mut *lock(&lines), Lines::new(0));
    let errors = std::mem::replace(&mut *lock(&stderr), Bytes::new(0));
    let ending = match (stopped, rung) {
        (None, _) | (Some(_), None) => Ending::Completed,
        (Some(Stopped::Abandoned), Some(rung)) => Ending::Abandoned(rung),
        (Some(Stopped::Wall), Some(rung)) => Ending::WallLimit(rung),
    };
    Ok(Run {
        lines: kept.kept,
        dropped: kept.dropped,
        truncated: kept.dropped > 0,
        stderr: String::from_utf8_lossy(&errors.data).into_owned(),
        ending,
        terminal_at: kept.terminal_at,
        status,
    })
}

/// Why the driver stopped a child.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stopped {
    Abandoned,
    Wall,
}

/// Climbs one rung. Returns false when there is nothing above the last one.
fn climb(
    rung: &mut Option<Rung>,
    pid: u32,
    interrupt: &Interrupt,
    stdin: &Mutex<Option<ChildStdin>>,
) -> bool {
    let next = match rung {
        None => {
            match interrupt {
                Interrupt::InBand(bytes) => {
                    if let Some(pipe) = lock(stdin).as_mut() {
                        let _ = pipe.write_all(bytes.as_bytes());
                        let _ = pipe.flush();
                    }
                }
                Interrupt::Signal => signal_group(pid, Signal::SIGINT),
            }
            Rung::Interrupt
        }
        Some(Rung::Interrupt) => {
            signal_group(pid, Signal::SIGTERM);
            Rung::Term
        }
        Some(Rung::Term) => {
            signal_group(pid, Signal::SIGKILL);
            Rung::Kill
        }
        Some(Rung::Kill) => return false,
    };
    *rung = Some(next);
    true
}

fn signal_group(raw: u32, sig: Signal) {
    if let Ok(pid) = i32::try_from(raw) {
        if pid > 0 {
            let _ = killpg(Pid::from_raw(pid), sig);
        }
    }
}

/// Waits for the child on its own thread, so the supervisor never blocks on
/// a process that has stopped talking.
fn reap(mut child: Child, tx: mpsc::Sender<Event>) {
    drop(
        std::thread::Builder::new()
            .name("tau-agent-wait".to_owned())
            .spawn(move || {
                let _ = tx.send(Event::Exited(child.wait()));
            }),
    );
}

/// The kept output lines, to the bound.
struct Lines {
    kept: Vec<Vec<u8>>,
    bytes: usize,
    bound: usize,
    dropped: usize,
    terminal_at: Option<usize>,
}

impl Lines {
    fn new(bound: usize) -> Self {
        Self {
            kept: Vec::new(),
            bytes: 0,
            bound,
            dropped: 0,
            terminal_at: None,
        }
    }

    /// Keeps a line if there is room — and always if it is the terminal
    /// one, whatever the bound: a transcript without the event that ends it
    /// would lose the usage the bill is made of (ADR-0013 §2).
    fn push(&mut self, line: Vec<u8>, terminal: bool) {
        let next = self.bytes.saturating_add(line.len());
        if terminal || next <= self.bound {
            if terminal {
                self.terminal_at = Some(self.kept.len());
            }
            self.bytes = next;
            self.kept.push(line);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

/// A bounded byte buffer, for stderr.
struct Bytes {
    data: Vec<u8>,
    bound: usize,
}

impl Bytes {
    fn new(bound: usize) -> Self {
        Self {
            data: Vec::new(),
            bound,
        }
    }
}

/// Reads `pipe` to the end on a thread, splitting it into lines and keeping
/// what the bound allows.
fn drain_lines(
    mut pipe: impl Read + Send + 'static,
    into: Arc<Mutex<Lines>>,
    terminal: fn(&[u8]) -> bool,
    tx: mpsc::Sender<Event>,
) {
    drop(
        std::thread::Builder::new()
            .name("tau-agent-stdout".to_owned())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut pending: Vec<u8> = Vec::new();
                loop {
                    let read = match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    pending.extend_from_slice(buf.get(..read).unwrap_or_default());
                    while let Some(at) = pending.iter().position(|&b| b == b'\n') {
                        let mut line: Vec<u8> = pending.drain(..=at).collect();
                        line.pop();
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        if keep(&into, line, terminal) {
                            let _ = tx.send(Event::Terminal);
                        }
                    }
                }
                if !pending.is_empty() && keep(&into, pending, terminal) {
                    let _ = tx.send(Event::Terminal);
                }
                let _ = tx.send(Event::Drained);
            }),
    );
}

/// Keeps one line; returns whether it was the terminal one.
fn keep(into: &Mutex<Lines>, line: Vec<u8>, terminal: fn(&[u8]) -> bool) -> bool {
    if line.is_empty() {
        return false;
    }
    let is_terminal = terminal(&line);
    lock(into).push(line, is_terminal);
    is_terminal
}

/// Reads `pipe` to the end on a thread, keeping the first `bound` bytes.
fn drain_bytes(
    mut pipe: impl Read + Send + 'static,
    into: Arc<Mutex<Bytes>>,
    tx: mpsc::Sender<Event>,
) {
    drop(
        std::thread::Builder::new()
            .name("tau-agent-stderr".to_owned())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let read = match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let mut sink = lock(&into);
                    let room = sink.bound.saturating_sub(sink.data.len());
                    let keep = read.min(room);
                    let chunk = buf.get(..keep).unwrap_or_default();
                    sink.data.extend_from_slice(chunk);
                }
                let _ = tx.send(Event::Drained);
            }),
    );
}
