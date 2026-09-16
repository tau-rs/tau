//! `tau`: the kernel's command line. One command so far, `replay`
//! (ADR-0010 §7, [#91](https://github.com/tau-rs/tau/issues/91)).
//!
//! ```text
//! tau replay <log> [--expect <hash-file>]
//! ```
//!
//! `replay` reads a log written by any build, refuses one this build cannot
//! read, folds every entry through the reducer, and prints the entries
//! folded, the last `seq`, and the state hash. With `--expect`, the hash is
//! compared to the one in the file and a mismatch is an error naming both.
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

use tau_kernel::abi::{LogHeader, ABI};
use tau_kernel::log::{Log, LogError};
use tau_kernel::reducer::{Refusal, State, StateHash};

const USAGE: &str = "usage:
  tau replay <log> [--expect <hash-file>]

exit codes:
  0  folded (and matched --expect, if given)
  1  usage
  2  the log or the --expect file could not be read
  3  the header is not readable by this build (magic, or abi newer than this build's)
  4  an entry line is malformed (the message names the line)
  5  the reducer refused an entry (the message names the entry)
  6  --expect did not match (the message names both hashes)";

/// The first ABI every log the freeze covers was written at; below it, best effort.
const FROZEN_AT: u16 = 2;

/// What the command line asked for.
enum Cmd {
    Help,
    Replay {
        log: PathBuf,
        expect: Option<PathBuf>,
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
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) | Self::Io(m) | Self::Header(m) => f.write_str(m),
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

fn parse(args: impl Iterator<Item = String>) -> Result<Cmd, Failure> {
    let mut args = args.peekable();
    let verb = args
        .next()
        .ok_or_else(|| Failure::Usage(USAGE.to_owned()))?;
    match verb.as_str() {
        "--help" | "-h" | "help" => return Ok(Cmd::Help),
        "replay" => {}
        other => {
            return Err(Failure::Usage(format!(
                "unknown command `{other}`\n{USAGE}"
            )));
        }
    }
    let mut log: Option<PathBuf> = None;
    let mut expect: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Cmd::Help),
            "--expect" => {
                let value = args
                    .next()
                    .ok_or_else(|| Failure::Usage(format!("`--expect` needs a value\n{USAGE}")))?;
                expect = Some(PathBuf::from(value));
            }
            flag if flag.starts_with("--") => {
                return Err(Failure::Usage(format!("unknown flag `{flag}`\n{USAGE}")));
            }
            path => {
                if log.is_some() {
                    return Err(Failure::Usage(format!(
                        "`replay` takes one log, got a second: `{path}`\n{USAGE}"
                    )));
                }
                log = Some(PathBuf::from(path));
            }
        }
    }
    let log = log.ok_or_else(|| Failure::Usage(format!("`replay` needs a log\n{USAGE}")))?;
    Ok(Cmd::Replay { log, expect })
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

fn replay(log: &Path, expect: Option<&Path>) -> Result<(), Failure> {
    let expected = expect
        .map(|p| {
            std::fs::read_to_string(p)
                .map(|s| s.trim().to_owned())
                .map_err(|e| Failure::Io(format!("cannot read {}: {e}", p.display())))
        })
        .transpose()?;
    let read = read(log)?;
    println!("{}", describe(read.header()));
    let mut state = State::initial();
    for (index, entry) in read.entries().iter().enumerate() {
        state
            .apply(entry)
            .map_err(|refusal| Failure::Refused { index, refusal })?;
    }
    let folded = state.hash();
    let last_seq = read
        .entries()
        .last()
        .map_or_else(|| "none".to_owned(), |e| e.seq().to_string());
    println!("entries={} last_seq={last_seq} hash={folded}", read.len());
    if let Some(expected) = expected {
        if folded.to_string() != expected {
            return Err(Failure::Mismatch { expected, folded });
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let outcome = match parse(std::env::args().skip(1)) {
        Ok(Cmd::Help) => {
            println!("{USAGE}");
            Ok(())
        }
        Ok(Cmd::Replay { log, expect }) => replay(&log, expect.as_deref()),
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
