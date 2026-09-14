//! The Tier 2 soak, as a command `tier2.yml` can run on two platforms
//! (HANDOFF §8, Tier 2 item 2; ADR-0003).
//!
//! ```text
//! soak   --seed S --events N --log PATH --hash PATH [--check-every K] [--max-live M]
//! refold --log PATH --expect PATH
//! ```
//!
//! `soak` runs the generator in its linear-per-event mode, refolds the log in
//! the same process, and refuses to write a hash the refold did not
//! reproduce. `refold` reads a log another build wrote, folds it, and exits
//! non-zero unless the hash matches the one in `--expect`. The Linux job runs
//! the first; the macOS job runs the second on the artifact — that is the
//! cross-platform half of the check, the one that catches iteration-order
//! and formatting differences a single machine never sees.
//!
//! Wall-clock reporting is the workflow's job, not this program's: nothing
//! here reads a clock.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use tau_kernel::log::Log;
use tau_kernel::reducer::fold;
use tau_sim::{run_with, Options};

const USAGE: &str = "usage:
  soak   --seed S --events N --log PATH --hash PATH [--check-every K] [--max-live M]
  refold --log PATH --expect PATH";

/// What the command line asked for.
enum Cmd {
    Soak {
        seed: u64,
        options: Options,
        log: PathBuf,
        hash: PathBuf,
    },
    Refold {
        log: PathBuf,
        expect: PathBuf,
    },
}

fn parse(args: impl Iterator<Item = String>) -> Result<Cmd, String> {
    let mut args = args.peekable();
    let verb = args.next().ok_or_else(|| USAGE.to_owned())?;
    let mut seed: u64 = 1;
    let mut events: u64 = 1_000_000;
    let mut check_every: Option<u64> = Some(10_000);
    let mut max_live: Option<u64> = Some(64);
    let mut log: Option<PathBuf> = None;
    let mut hash: Option<PathBuf> = None;
    let mut expect: Option<PathBuf> = None;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value\n{USAGE}"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|e| format!("`{flag}`: {value:?} is not a number: {e}"))
        };
        match flag.as_str() {
            "--seed" => seed = number()?,
            "--events" => events = number()?,
            "--check-every" => check_every = Some(number()?).filter(|k| *k > 0),
            "--max-live" => max_live = Some(number()?).filter(|m| *m > 0),
            "--log" => log = Some(PathBuf::from(&value)),
            "--hash" => hash = Some(PathBuf::from(&value)),
            "--expect" => expect = Some(PathBuf::from(&value)),
            other => return Err(format!("unknown flag `{other}`\n{USAGE}")),
        }
    }
    let log = log.ok_or_else(|| format!("`--log` is required\n{USAGE}"))?;
    match verb.as_str() {
        "soak" => Ok(Cmd::Soak {
            seed,
            options: Options {
                events,
                check_every,
                max_live,
            },
            log,
            hash: hash.ok_or_else(|| format!("`--hash` is required\n{USAGE}"))?,
        }),
        "refold" => Ok(Cmd::Refold {
            log,
            expect: expect.ok_or_else(|| format!("`--expect` is required\n{USAGE}"))?,
        }),
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

fn soak(seed: u64, options: &Options, log: &PathBuf, hash: &PathBuf) -> Result<(), String> {
    let report = run_with(seed, options).map_err(|e| format!("run failed: {e}"))?;
    let incremental = report.state.hash();
    let refold = fold(report.log.entries())
        .map_err(|e| format!("refold refused an entry: {e}"))?
        .hash();
    if incremental != refold {
        return Err(format!(
            "hash mismatch on the same build: incremental {incremental}, refold {refold}"
        ));
    }
    let mut sink = BufWriter::new(
        File::create(log).map_err(|e| format!("cannot create {}: {e}", log.display()))?,
    );
    report
        .log
        .write_to(&mut sink)
        .map_err(|e| format!("cannot write {}: {e}", log.display()))?;
    sink.flush()
        .map_err(|e| format!("cannot flush {}: {e}", log.display()))?;
    std::fs::write(hash, format!("{incremental}\n"))
        .map_err(|e| format!("cannot write {}: {e}", hash.display()))?;
    println!(
        "seed={seed} entries={} refused={} peak_live={} records={} hash={incremental}",
        report.log.len(),
        report.refused,
        report.peak_live,
        report.state.agents().count(),
    );
    Ok(())
}

fn refold(log: &PathBuf, expect: &PathBuf) -> Result<(), String> {
    let expected = std::fs::read_to_string(expect)
        .map_err(|e| format!("cannot read {}: {e}", expect.display()))?;
    let expected = expected.trim();
    let file = File::open(log).map_err(|e| format!("cannot open {}: {e}", log.display()))?;
    let read = Log::read_from(BufReader::new(file))
        .map_err(|e| format!("cannot read {}: {e}", log.display()))?;
    let folded = fold(read.entries())
        .map_err(|e| format!("fold refused an entry: {e}"))?
        .hash();
    println!("entries={} hash={folded}", read.len());
    if folded.to_string() != expected {
        return Err(format!(
            "hash mismatch across builds: expected {expected}, folded {folded}"
        ));
    }
    Ok(())
}

fn main() -> ExitCode {
    let outcome = match parse(std::env::args().skip(1)) {
        Ok(Cmd::Soak {
            seed,
            options,
            log,
            hash,
        }) => soak(seed, &options, &log, &hash),
        Ok(Cmd::Refold { log, expect }) => refold(&log, &expect),
        Err(usage) => Err(usage),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("soak: {message}");
            ExitCode::FAILURE
        }
    }
}
