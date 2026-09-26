//! `tau`: the kernel's command line. Four commands: `replay` (ADR-0010 §7,
//! [#91](https://github.com/tau-rs/tau/issues/91)), `snapshot` (ADR-0011
//! §4, [#119](https://github.com/tau-rs/tau/issues/119)), and the two store
//! verbs `blobs` and `shred` (ADR-0012 §4,
//! [#199](https://github.com/tau-rs/tau/issues/199)).
//!
//! ```text
//! tau replay <log> [--from <snapshot>] [--expect <hash-file>]
//! tau snapshot <log> [--at <k>] <out>
//! tau blobs <store> [<ref>...]
//! tau shred <log> <store> <root>
//! ```
//!
//! `replay` reads a log written by any build, refuses one this build cannot
//! read, folds every entry through the reducer, and prints the entries
//! folded, the last `seq`, and the state hash. With `--expect`, the hash is
//! compared to the one in the file and a mismatch is an error naming both.
//! With `--from`, the fold starts from a snapshot instead of from zero: the
//! snapshot's header is checked, then its join to the log, then entries
//! `k..` are folded as before. A snapshot is never authoritative — every
//! check runs, none can be skipped, and a refusal means "refold from zero".
//!
//! `snapshot` folds the first `k` entries (default: all of them) and writes
//! the two-line snapshot file `replay --from` reads.
//!
//! `blobs` opens a store directory and says what it holds: one line per
//! reference with `present` (some copy opens), `shredded` (copies exist,
//! no key does) or `absent` (never stored), plus the tombstone list of
//! shredded owners. With references on the command line, it answers for
//! those instead of listing the store.
//!
//! `shred` is `Kernel::shred` from outside the kernel: it folds the log,
//! finds the subtree under `root`, refuses if any agent in it is still live
//! or cancelling (exit 9, naming the agent, nothing touched), and otherwise
//! drops every agent's key in the store. The log is not changed — a shred
//! is invisible to the fold by design (ADR-0012 §5) — and a store that
//! could not drop a key is reported as exactly that (exit 10), never as
//! erasure. The verb reads the log as written: it is for a run whose kernel
//! is finished, and the fold is its only view of who is live.
//!
//! A store directory has one writer (ADR-0012 §4 "One writer",
//! [#145](https://github.com/tau-rs/tau/issues/145)): the kernel that opened
//! it holds a lock on its `STORE` file for as long as it runs, and both verbs
//! open the store the same way, so beside a running kernel each is refused
//! (exit 11, naming the directory) before anything else is read. `shred`
//! opens the store before it reads the log on purpose: the lock says who
//! holds the directory *now*, while the log on disk lags a live kernel, so
//! the fold's live check is only consulted once nobody holds the store.
//! `blobs` is refused too, although it only reads: a `Disk` cannot promise
//! not to write, and a read-only view is a follow-on
//! ([#234](https://github.com/tau-rs/tau/issues/234)) if an operator needs
//! to inspect a running store.
//!
//! Neither verb initialises a store: a path with no `STORE` header is
//! refused (exit 3), so a mistyped path cannot be shredded or listed as an
//! empty store.
//!
//! The fold is the reducer alone: no hook program, no driver, no clock
//! (ADR-0008: a fold confirms verdicts, it never runs them). Nothing here
//! links or constructs any of those.
//!
//! # Exit codes
//!
//! Each failure has its own code so a script can tell them apart without
//! parsing the message; the message names what a human needs.
//!
//! | code | meaning |
//! |---|---|
//! | 0 | folded; and the hash matched `--expect`, if given |
//! | 1 | usage: an unknown command or flag, or a missing argument |
//! | 2 | the log or the `--expect` file could not be read (i/o) |
//! | 3 | the header is not one this build can read: missing, malformed, wrong magic, or an `abi` newer than this build's. The message names both numbers |
//! | 4 | an entry line is malformed. The message names the line |
//! | 5 | the reducer refused an entry. The message names the entry and the refusal |
//! | 6 | `--expect` did not match. The message names both hashes |
//! | 7 | the `--from` snapshot is not usable by this build: wrong magic, an `abi` newer than this build's, a `fold` other than this build's, or a body that does not parse. The message names the field and both values. Use another build, or refold from zero |
//! | 8 | the `--from` snapshot is not a snapshot of this log at its offset: `seq` past the end, the prefix digest disagrees, the state does not re-hash to what the header claims, or the body's `next_seq` is not the header's `seq`. The message names the check. No build will make them fit |
//! | 9 | `shred` refused: an agent in the subtree is live or cancelling. The message names it. Nothing was shredded; cancel it, wait for the abort, and shred again |
//! | 10 | `shred` could not drop a key: the store reports a write-path failure, this shred's or an earlier one's, so erasure is not promised. The message names the failure |
//! | 11 | the store is held by a running kernel (or another `tau`): its `STORE` file is locked. The message names the directory. Nothing was read or shredded; wait for the holder to finish |
//!
//! Exit 3 also covers a store the `blobs` and `shred` verbs cannot read: no
//! `STORE` header at the path, or a header naming a `magic`, `v`, `digest`
//! or `aead` this build does not know.
//!
//! # The header's promise
//!
//! A log at `abi: 2` or later was written after the `Entry` freeze
//! (ADR-0010): every build at `ABI ≥ 2` folds it to the same hash. A log at
//! `abi: 0` or `1` predates the freeze, and reading it is best effort (ADR-0010
//! §7); `replay` says so when it prints the header.

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tau_kernel::abi::{AgentId, BlobRef, LogHeader, ABI};
use tau_kernel::kernel::{shred_subtree, ShredError};
use tau_kernel::log::{Log, LogError};
use tau_kernel::reducer::{Refusal, State, StateHash};
use tau_kernel::snapshot::{JoinError, Snapshot, SnapshotError};
use tau_store::{Disk, OpenError, Status};

const USAGE: &str = "usage:
  tau replay <log> [--from <snapshot>] [--expect <hash-file>]
  tau snapshot <log> [--at <k>] <out>
  tau blobs <store> [<ref>...]
  tau shred <log> <store> <root>

exit codes:
  0  folded (and matched --expect, if given); listed; shredded
  1  usage
  2  the log, the --from snapshot, the --expect file, the snapshot output, or the store could not be read or written
  3  the log header, or the store's STORE header, is not readable by this build (magic, v, digest, aead, or abi newer than this build's); or the path is not a store
  4  an entry line is malformed (the message names the line)
  5  the reducer refused an entry (the message names the entry)
  6  --expect did not match (the message names both hashes)
  7  the --from snapshot is not usable by this build (magic, abi, fold, or a malformed body; the message names the field and both values)
  8  the --from snapshot is not a snapshot of this log at its offset (seq, prefix, state, or next_seq; the message names the check)
  9  shred refused: an agent in the subtree is live or cancelling (the message names it); nothing was shredded
  10 shred could not drop a key: the store reports a write failure, so erasure is not promised (the message names it)
  11 the store is held by a running kernel or another tau (STORE is locked; the message names the directory); nothing was touched";

/// The first ABI every log the freeze covers was written at; below it, best effort.
const FROZEN_AT: u16 = 2;

/// What the command line asked for.
enum Cmd {
    Help,
    Replay {
        log: PathBuf,
        from: Option<PathBuf>,
        expect: Option<PathBuf>,
    },
    Snapshot {
        log: PathBuf,
        at: Option<usize>,
        out: PathBuf,
    },
    Blobs {
        store: PathBuf,
        refs: Vec<BlobRef>,
    },
    Shred {
        log: PathBuf,
        store: PathBuf,
        root: AgentId,
    },
}

/// Why `replay` stopped, with the exit code each reason maps to.
enum Failure {
    Usage(String),
    Io(String),
    Header(String),
    Malformed {
        line: usize,
        source: serde_json::Error,
    },
    Refused {
        index: usize,
        refusal: Refusal,
    },
    Mismatch {
        expected: String,
        folded: StateHash,
    },
    /// The snapshot is not one this build can use.
    Unusable(SnapshotError),
    /// The snapshot and the log do not belong together.
    Join(JoinError),
    /// `shred` refused: this agent in the subtree is live or cancelling.
    Live(AgentId),
    /// `shred` could not drop a key; the store's fault, as it reported it.
    Faulted(String),
    /// The store is held: another `Disk` has its `STORE` locked.
    Held(PathBuf),
}

impl Failure {
    const fn code(&self) -> u8 {
        match self {
            Self::Usage(_) => 1,
            Self::Io(_) => 2,
            Self::Header(_) => 3,
            Self::Malformed { .. } => 4,
            Self::Refused { .. } => 5,
            Self::Mismatch { .. } => 6,
            Self::Unusable(_) => 7,
            Self::Join(_) => 8,
            Self::Live(_) => 9,
            Self::Faulted(_) => 10,
            Self::Held(_) => 11,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) | Self::Io(m) | Self::Header(m) => f.write_str(m),
            Self::Unusable(e) => write!(f, "snapshot is not usable by this build: {e}"),
            Self::Live(id) => write!(
                f,
                "shred refused: {id} is still live; cancel it and wait for the abort before shredding"
            ),
            Self::Faulted(reason) => write!(f, "shred could not drop a key: {reason}"),
            Self::Held(path) => write!(
                f,
                "store held: {} is open in a running kernel or another tau (STORE is locked); wait for it to finish",
                path.display()
            ),
            Self::Join(e) => write!(f, "snapshot is not a snapshot of this log: {e}"),
            Self::Malformed { line, source } => {
                write!(f, "log entry on line {line} is malformed: {source}")
            }
            Self::Refused { index, refusal } => {
                write!(f, "the reducer refused entry {index}: {refusal}")
            }
            Self::Mismatch { expected, folded } => write!(
                f,
                "hash mismatch across builds: expected {expected}, folded {folded}"
            ),
        }
    }
}

fn usage(message: impl fmt::Display) -> Failure {
    Failure::Usage(format!("{message}\n{USAGE}"))
}

fn parse(args: impl Iterator<Item = String>) -> Result<Cmd, Failure> {
    let mut args = args.peekable();
    let verb = args
        .next()
        .ok_or_else(|| Failure::Usage(USAGE.to_owned()))?;
    match verb.as_str() {
        "--help" | "-h" | "help" => Ok(Cmd::Help),
        "replay" => parse_replay(args),
        "snapshot" => parse_snapshot(args),
        "blobs" => parse_blobs(args),
        "shred" => parse_shred(args),
        other => Err(usage(format_args!("unknown command `{other}`"))),
    }
}

/// The value after a flag.
fn value_of(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<String, Failure> {
    args.next()
        .ok_or_else(|| usage(format_args!("`{flag}` needs a value")))
}

fn parse_replay(mut args: impl Iterator<Item = String>) -> Result<Cmd, Failure> {
    let mut log: Option<PathBuf> = None;
    let mut from: Option<PathBuf> = None;
    let mut expect: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Cmd::Help),
            "--expect" => expect = Some(PathBuf::from(value_of("--expect", &mut args)?)),
            "--from" => from = Some(PathBuf::from(value_of("--from", &mut args)?)),
            flag if flag.starts_with("--") => {
                return Err(usage(format_args!("unknown flag `{flag}`")));
            }
            path => {
                if log.is_some() {
                    return Err(usage(format_args!(
                        "`replay` takes one log, got a second: `{path}`"
                    )));
                }
                log = Some(PathBuf::from(path));
            }
        }
    }
    let log = log.ok_or_else(|| usage("`replay` needs a log"))?;
    Ok(Cmd::Replay { log, from, expect })
}

fn parse_snapshot(mut args: impl Iterator<Item = String>) -> Result<Cmd, Failure> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut at: Option<usize> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Cmd::Help),
            "--at" => {
                let value = value_of("--at", &mut args)?;
                at = Some(value.parse().map_err(|e| {
                    usage(format_args!(
                        "`--at` wants a number of entries, got `{value}`: {e}"
                    ))
                })?);
            }
            flag if flag.starts_with("--") => {
                return Err(usage(format_args!("unknown flag `{flag}`")));
            }
            path => paths.push(PathBuf::from(path)),
        }
    }
    let mut paths = paths.into_iter();
    match (paths.next(), paths.next(), paths.next()) {
        (Some(log), Some(out), None) => Ok(Cmd::Snapshot { log, at, out }),
        (_, _, Some(extra)) => Err(usage(format_args!(
            "`snapshot` takes a log and an output path, got a third: `{}`",
            extra.display()
        ))),
        _ => Err(usage("`snapshot` needs a log and an output path")),
    }
}

/// Positional arguments only: `--help` is honoured, any other flag is a
/// usage error.
fn positionals(
    verb: &str,
    args: impl Iterator<Item = String>,
) -> Result<Option<Vec<String>>, Failure> {
    let mut out = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            flag if flag.starts_with("--") => {
                return Err(usage(format_args!("unknown flag `{flag}` for `{verb}`")));
            }
            _ => out.push(arg),
        }
    }
    Ok(Some(out))
}

fn parse_blobs(args: impl Iterator<Item = String>) -> Result<Cmd, Failure> {
    let Some(args) = positionals("blobs", args)? else {
        return Ok(Cmd::Help);
    };
    let mut args = args.into_iter();
    let store = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| usage("`blobs` needs a store directory"))?;
    let refs = args
        .map(|hex| {
            BlobRef::from_hex(&hex).map_err(|e| {
                usage(format_args!(
                    "`{hex}` is not a payload reference (64 lowercase hex characters): {e}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Cmd::Blobs { store, refs })
}

fn parse_shred(args: impl Iterator<Item = String>) -> Result<Cmd, Failure> {
    let Some(args) = positionals("shred", args)? else {
        return Ok(Cmd::Help);
    };
    let mut args = args.into_iter();
    match (args.next(), args.next(), args.next(), args.next()) {
        (Some(log), Some(store), Some(root), None) => Ok(Cmd::Shred {
            log: PathBuf::from(log),
            store: PathBuf::from(store),
            root: parse_agent(&root)?,
        }),
        (_, _, _, Some(extra)) => Err(usage(format_args!(
            "`shred` takes a log, a store and a root agent, got a fourth: `{extra}`"
        ))),
        _ => Err(usage(
            "`shred` needs a log, a store directory and a root agent",
        )),
    }
}

/// An agent id as the kernel prints it (`agent:7`) or bare (`7`).
fn parse_agent(text: &str) -> Result<AgentId, Failure> {
    text.strip_prefix("agent:")
        .unwrap_or(text)
        .parse::<u64>()
        .map(AgentId::new)
        .map_err(|e| {
            usage(format_args!(
                "`{text}` is not an agent id (`agent:<n>` or `<n>`): {e}"
            ))
        })
}

/// What the header lets this build promise about the fold.
fn describe(header: LogHeader) -> String {
    if header.abi >= FROZEN_AT {
        format!(
            "header: abi {} (entry format frozen; every build at abi >= {FROZEN_AT} folds this log to the same hash)",
            header.abi
        )
    } else {
        format!(
            "header: abi {} (the entry format was not frozen when this log was written; reading it is best effort, ADR-0010 §7)",
            header.abi
        )
    }
}

fn read(path: &Path) -> Result<Log, Failure> {
    let file = File::open(path)
        .map_err(|e| Failure::Io(format!("cannot open {}: {e}", path.display())))?;
    Log::read_from(BufReader::new(file)).map_err(|e| match e {
        LogError::Io(e) => Failure::Io(format!("cannot read {}: {e}", path.display())),
        LogError::MissingHeader => Failure::Header("log has no header".to_owned()),
        LogError::MalformedHeader(source) => {
            Failure::Header(format!("log header is malformed: {source}"))
        }
        LogError::Unreadable { header } if header.magic != LogHeader::MAGIC => {
            Failure::Header(format!(
                "log header has magic {:?}, not a tau log ({:?})",
                header.magic,
                LogHeader::MAGIC
            ))
        }
        LogError::Unreadable { header } => Failure::Header(format!(
            "log header says abi {}; this build reads abi <= {ABI}",
            header.abi
        )),
        LogError::MalformedEntry { line, source } => Failure::Malformed { line, source },
        other => Failure::Io(format!("cannot read {}: {other}", path.display())),
    })
}

/// The first `k` entries of `read`, or a usage error naming both numbers.
fn head(read: &Log, at: Option<usize>) -> Result<&[tau_kernel::log::Entry], Failure> {
    let k = at.unwrap_or(read.len());
    read.entries().get(..k).ok_or_else(|| {
        usage(format_args!(
            "`--at {k}` is past the end: the log has {} entries",
            read.len()
        ))
    })
}

fn apply_all(
    state: &mut State,
    entries: &[tau_kernel::log::Entry],
    offset: usize,
) -> Result<(), Failure> {
    for (i, entry) in entries.iter().enumerate() {
        state.apply(entry).map_err(|refusal| Failure::Refused {
            index: offset.saturating_add(i),
            refusal,
        })?;
    }
    Ok(())
}

/// Reads and checks a snapshot (exit 7), then joins it to `read` (exit 8).
fn restore(path: &Path, read: &Log) -> Result<State, Failure> {
    let file = File::open(path)
        .map_err(|e| Failure::Io(format!("cannot open {}: {e}", path.display())))?;
    let snapshot = Snapshot::read_from(BufReader::new(file)).map_err(|e| match e {
        SnapshotError::Io(e) => Failure::Io(format!("cannot read {}: {e}", path.display())),
        other => Failure::Unusable(other),
    })?;
    snapshot.join(read).map_err(Failure::Join)
}

fn replay(log: &Path, from: Option<&Path>, expect: Option<&Path>) -> Result<(), Failure> {
    let expected = expect
        .map(|p| {
            std::fs::read_to_string(p)
                .map(|s| s.trim().to_owned())
                .map_err(|e| Failure::Io(format!("cannot read {}: {e}", p.display())))
        })
        .transpose()?;
    let read = read(log)?;
    println!("{}", describe(read.header()));
    let (mut state, from) = match from {
        Some(path) => {
            let state = restore(path, &read)?;
            let k = usize::try_from(state.next_seq().get()).unwrap_or(usize::MAX);
            (state, Some(k))
        }
        None => (State::initial(), None),
    };
    let tail = read.entries().get(from.unwrap_or(0)..).unwrap_or_default();
    apply_all(&mut state, tail, from.unwrap_or(0))?;
    let folded = state.hash();
    let last_seq = read
        .entries()
        .last()
        .map_or_else(|| "none".to_owned(), |e| e.seq().to_string());
    match from {
        Some(k) => println!(
            "entries={} from={k} last_seq={last_seq} hash={folded}",
            read.len()
        ),
        None => println!("entries={} last_seq={last_seq} hash={folded}", read.len()),
    }
    if let Some(expected) = expected {
        if folded.to_string() != expected {
            return Err(Failure::Mismatch { expected, folded });
        }
    }
    Ok(())
}

fn snapshot(log: &Path, at: Option<usize>, out: &Path) -> Result<(), Failure> {
    let read = read(log)?;
    println!("{}", describe(read.header()));
    let entries = head(&read, at)?;
    let k = entries.len();
    let mut state = State::initial();
    apply_all(&mut state, entries, 0)?;
    // `head` bounded `k` by the length, so the digest exists.
    let prefix = read
        .prefix_at(k)
        .ok_or_else(|| usage(format_args!("`--at {k}` is past the end")))?;
    let snapshot = state.snapshot(prefix);
    let file = File::create(out)
        .map_err(|e| Failure::Io(format!("cannot create {}: {e}", out.display())))?;
    snapshot
        .write_to(std::io::BufWriter::new(file))
        .map_err(|e| Failure::Io(format!("cannot write {}: {e}", out.display())))?;
    let header = snapshot.header();
    println!(
        "snapshot={} seq={} fold={} prefix={} hash={}",
        out.display(),
        header.seq,
        header.fold,
        header.prefix,
        header.state
    );
    Ok(())
}

/// Opens a store that already exists; never initialises one (exit 2, 3 or
/// 11). The `Disk` holds the store until it is dropped.
fn open_store(path: &Path) -> Result<Disk, Failure> {
    Disk::open_existing(path).map_err(|e| match e {
        OpenError::Io { path, source } => {
            Failure::Io(format!("cannot open {}: {source}", path.display()))
        }
        OpenError::Held(path) => Failure::Held(path),
        OpenError::NotAStore(_) | OpenError::NoHeader(_) => Failure::Header(e.to_string()),
        other => Failure::Header(format!("{}: {other}", path.display())),
    })
}

const fn status_word(status: Status) -> &'static str {
    match status {
        Status::Present => "present",
        Status::Shredded => "shredded",
        Status::Absent => "absent",
    }
}

fn join_ids(ids: &[AgentId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn blobs(store: &Path, refs: &[BlobRef]) -> Result<(), Failure> {
    let disk = open_store(store)?;
    let io = |what: &str, e: std::io::Error| {
        Failure::Io(format!("cannot read {}: {e}", store.join(what).display()))
    };
    let shredded = disk.shredded().map_err(|e| io("shredded", e))?;
    let listed = if refs.is_empty() {
        disk.references().map_err(|e| io("objects", e))?
    } else {
        refs.to_vec()
    };
    println!(
        "store={} references={} shredded={}",
        store.display(),
        listed.len(),
        shredded.len()
    );
    if !shredded.is_empty() {
        println!("shredded: {}", join_ids(&shredded));
    }
    for blob in &listed {
        println!("{blob} {}", status_word(disk.status(blob)));
    }
    Ok(())
}

fn shred(log: &Path, store: &Path, root: AgentId) -> Result<(), Failure> {
    // The store first: a held one is refused before the log is read, since
    // the lock is current and the log on disk is not while a kernel runs.
    let mut disk = open_store(store)?;
    let read = read(log)?;
    println!("{}", describe(read.header()));
    let mut state = State::initial();
    apply_all(&mut state, read.entries(), 0)?;
    if state.agent(root).is_none() {
        return Err(usage(format_args!(
            "{root} is not an agent in {}",
            log.display()
        )));
    }
    let subtree = state.subtree(root);
    shred_subtree(&state, &mut disk, root).map_err(|e| match e {
        ShredError::Live(id) => Failure::Live(id),
        ShredError::Faulted { reason } => Failure::Faulted(reason),
        other => Failure::Faulted(other.to_string()),
    })?;
    println!(
        "shredded={} root={root} agents: {}",
        subtree.len(),
        join_ids(&subtree)
    );
    Ok(())
}

fn main() -> ExitCode {
    let outcome = match parse(std::env::args().skip(1)) {
        Ok(Cmd::Help) => {
            println!("{USAGE}");
            Ok(())
        }
        Ok(Cmd::Replay { log, from, expect }) => replay(&log, from.as_deref(), expect.as_deref()),
        Ok(Cmd::Snapshot { log, at, out }) => snapshot(&log, at, &out),
        Ok(Cmd::Blobs { store, refs }) => blobs(&store, &refs),
        Ok(Cmd::Shred { log, store, root }) => shred(&log, &store, root),
        Err(usage) => Err(usage),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("tau: {failure}");
            ExitCode::from(failure.code())
        }
    }
}
