//! `tau replay` over the corpus and over every way it can refuse (#91,
//! ADR-0010 §6: "`tau replay` folding all of them to their sidecars is the
//! acceptance test for the CLI").
//!
//! Every test runs the built binary as a subprocess: the exit code *is* the
//! contract, and a test that called the functions would not see it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tau_kernel::abi::ABI;
use tau_kernel::log::Log;
use tau_kernel::reducer::{fold, State};

/// The corpus lives at the repository root, not under `kernel/`.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The smallest corpus log: the one every failure test is derived from.
fn small_log() -> PathBuf {
    corpus().join("m1b-wall.log")
}

fn tau(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tau"))
        .args(args)
        .output()
        .expect("the tau binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The `hash=` field of the summary line, the last one on stdout.
fn printed_hash(out: &Output) -> String {
    let text = stdout(out);
    let last = text.lines().last().unwrap_or_default();
    last.split("hash=")
        .nth(1)
        .unwrap_or_else(|| panic!("no hash on the last line: {text}"))
        .trim()
        .to_owned()
}

/// A scratch file under cargo's per-crate temp dir; the name keeps parallel
/// tests apart.
fn scratch(name: &str, contents: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn log_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn logs_in(dir: &Path) -> Vec<PathBuf> {
    let mut logs: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    logs.sort();
    logs
}

// --- the acceptance test: the corpus ---------------------------------------

#[test]
fn every_corpus_log_folds_to_its_sidecar() {
    let logs = logs_in(&corpus());
    // The sentinel's floor (tier3.yml): an empty corpus must not pass here either.
    assert!(
        logs.len() >= 9,
        "corpus holds {} logs; the floor is 9",
        logs.len()
    );
    for log in logs {
        let sidecar = log.with_extension("hash");
        let expected = fs::read_to_string(&sidecar).unwrap().trim().to_owned();
        let out = tau(&[
            "replay",
            log.to_str().unwrap(),
            "--expect",
            sidecar.to_str().unwrap(),
        ]);
        assert!(
            out.status.success(),
            "{}: exit {:?}\n{}",
            log.display(),
            out.status.code(),
            stderr(&out)
        );
        assert_eq!(printed_hash(&out), expected, "{}", log.display());
    }
}

#[test]
fn every_fixture_folds_to_what_the_reducer_folds_in_process() {
    let logs = logs_in(&fixtures());
    assert_eq!(logs.len(), 4, "the four milestone fixtures");
    for log in logs {
        let bytes = fs::read(&log).unwrap();
        let in_process = fold(Log::read_from(bytes.as_slice()).unwrap().entries())
            .unwrap()
            .hash()
            .to_string();
        let out = tau(&["replay", log.to_str().unwrap()]);
        assert!(out.status.success(), "{}: {}", log.display(), stderr(&out));
        assert_eq!(printed_hash(&out), in_process, "{}", log.display());
    }
}

// --- what the header lets the CLI promise ------------------------------------

#[test]
fn a_pre_freeze_header_is_reported_as_best_effort() {
    let out = tau(&["replay", small_log().to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("header: abi 0"), "{text}");
    assert!(text.contains("not frozen"), "{text}");
    assert!(text.contains("entries=9 last_seq=seq:8 hash="), "{text}");
}

#[test]
fn a_frozen_header_with_no_entries_folds_to_the_initial_state() {
    let log = scratch(
        "replay-empty-frozen.log",
        &format!("{{\"magic\":[84,65,85,0],\"abi\":{ABI}}}\n"),
    );
    let out = tau(&["replay", log.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains(&format!("header: abi {ABI}")), "{text}");
    assert!(!text.contains("not frozen"), "{text}");
    assert!(text.contains("entries=0 last_seq=none hash="), "{text}");
    assert_eq!(printed_hash(&out), State::initial().hash().to_string());
}

// --- one test per exit code --------------------------------------------------

#[test]
fn exit_1_usage() {
    for args in [
        &[][..],
        &["bogus"][..],
        &["replay"][..],
        &["replay", "--nope", "x"][..],
    ] {
        let out = tau(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out).contains("usage:"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    let two = small_log();
    let out = tau(&["replay", two.to_str().unwrap(), two.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let out = tau(&["replay", two.to_str().unwrap(), "--expect"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("`--expect` needs a value"));
}

#[test]
fn help_exits_0_and_documents_the_exit_codes() {
    for args in [&["--help"][..], &["replay", "--help"][..]] {
        let out = tau(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let text = stdout(&out);
        assert!(
            text.contains("tau replay <log> [--expect <hash-file>]"),
            "{text}"
        );
        for code in 0..=6 {
            assert!(
                text.contains(&format!("\n  {code}  ")),
                "code {code} missing:\n{text}"
            );
        }
    }
}

#[test]
fn exit_2_when_the_log_cannot_be_opened() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-does-not-exist.log");
    let out = tau(&["replay", missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("cannot open"), "{}", stderr(&out));
}

#[test]
fn exit_2_when_the_expect_file_cannot_be_read() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-does-not-exist.hash");
    let out = tau(&[
        "replay",
        small_log().to_str().unwrap(),
        "--expect",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[test]
fn exit_3_when_the_header_abi_is_newer_than_this_build_naming_both() {
    let log = scratch("replay-abi-99.log", "{\"magic\":[84,65,85,0],\"abi\":99}\n");
    let out = tau(&["replay", log.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("abi 99"), "{err}");
    assert!(err.contains(&format!("abi <= {ABI}")), "{err}");
}

#[test]
fn exit_3_when_the_magic_is_wrong() {
    let log = scratch(
        "replay-wrong-magic.log",
        "{\"magic\":[78,79,80,0],\"abi\":0}\n",
    );
    let out = tau(&["replay", log.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("magic"), "{}", stderr(&out));
}

#[test]
fn exit_3_when_the_header_is_missing_or_malformed() {
    let empty = scratch("replay-no-header.log", "");
    let out = tau(&["replay", empty.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("no header"), "{}", stderr(&out));

    let garbage = scratch("replay-bad-header.log", "not json\n");
    let out = tau(&["replay", garbage.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("header is malformed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn exit_4_on_an_unknown_entry_tag_naming_the_line() {
    let mut lines = log_lines(&small_log());
    // Line 1 is the header, line 2 the first entry: the bad line is line 3.
    lines.truncate(2);
    lines.push("{\"entry\":\"teleported\",\"seq\":1}".to_owned());
    let log = scratch("replay-unknown-tag.log", &format!("{}\n", lines.join("\n")));
    let out = tau(&["replay", log.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("line 3"), "{err}");
    assert!(err.contains("teleported"), "{err}");
}

#[test]
fn exit_5_on_an_out_of_order_seq_naming_the_entry() {
    let mut lines = log_lines(&small_log());
    lines.swap(1, 2);
    let log = scratch(
        "replay-out-of-order.log",
        &format!("{}\n", lines.join("\n")),
    );
    let out = tau(&["replay", log.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("refused entry 0"), "{err}");
    assert!(err.contains("out of order"), "{err}");
}

#[test]
fn exit_6_on_an_expect_mismatch_naming_both_hashes() {
    let sidecar = scratch("replay-wrong.hash", "deadbeef\n");
    let out = tau(&[
        "replay",
        small_log().to_str().unwrap(),
        "--expect",
        sidecar.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(6), "{}", stderr(&out));
    let err = stderr(&out);
    let folded = printed_hash(&out);
    assert!(err.contains("expected deadbeef"), "{err}");
    assert!(err.contains(&format!("folded {folded}")), "{err}");
}
