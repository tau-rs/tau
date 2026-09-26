//! One writer per store (ADR-0012 §4, amendment 2026-09-26): a `Disk`
//! holds an exclusive advisory lock on `STORE` for as long as it lives, so
//! a second `Disk` on the same directory is refused, from this process or
//! another, and a writer that dies takes its lock with it.
//!
//! The other process is this test binary run again with `--exact` on
//! [`hold_the_store_for_the_parent`], which only does anything when
//! `TAU_STORE_HOLD` names a directory: it opens the store, says `held` on
//! stdout, and waits on stdin. No sleeps: the parent reads the word, then
//! acts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use tau_kernel::abi::AgentId;
use tau_kernel::blob::Blobs;
use tau_store::{Disk, OpenError};

const HOLD: &str = "TAU_STORE_HOLD";

/// Not a test of anything on its own: the far side of the two-process
/// tests below. Returns at once unless `TAU_STORE_HOLD` is set.
#[test]
fn hold_the_store_for_the_parent() {
    let Ok(path) = std::env::var(HOLD) else {
        return;
    };
    let disk = Disk::open(&path).unwrap();
    println!("held");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(disk);
}

/// The holder process and the pipe it reported on. The pipe stays open
/// until the holder is reaped, so its exit is never a broken pipe.
struct Holder {
    child: Child,
    _stdout: BufReader<ChildStdout>,
}

impl Holder {
    /// Spawns the holder on `store` and returns once it has the lock.
    fn spawn(store: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "hold_the_store_for_the_parent",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HOLD, store)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).unwrap();
            assert_ne!(n, 0, "the holder exited before holding the store");
            // libtest prints `test <name> ... ` with no newline before
            // the test runs, so the word ends that line rather than owning it.
            if line.trim_end().ends_with("held") {
                break;
            }
        }
        Self {
            child,
            _stdout: stdout,
        }
    }

    /// Lets the holder finish on its own: the orderly release.
    fn release(mut self) {
        self.child.stdin.take().unwrap().write_all(b"\n").unwrap();
        let status = self.child.wait().unwrap();
        assert!(status.success(), "the holder failed: {status}");
    }

    /// Kills the holder mid-hold: the crash.
    fn crash(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

fn held_by(err: OpenError, dir: &Path) {
    match err {
        OpenError::Held(path) => assert_eq!(path, dir, "names the store"),
        other => panic!("expected Held, got {other:?}"),
    }
}

#[test]
fn a_second_disk_on_one_directory_is_refused_until_the_first_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let first = Disk::open(dir.path()).unwrap();

    let err = Disk::open(dir.path()).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "{}: store held by another Disk (STORE is locked); one writer per store",
            dir.path().display()
        )
    );
    held_by(err, dir.path());
    held_by(
        Disk::open_existing(dir.path()).unwrap_err(),
        dir.path(),
        // `open_existing` is `tau shred`'s and `tau blobs`' way in, and it
        // contends the same: every `Disk` is a writer.
    );

    // The lock is the header, not a file of its own: the layout is as the
    // crate docs draw it.
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["STORE", "keys", "objects"]);

    drop(first);
    let mut second = Disk::open(dir.path()).unwrap();
    second.put(AgentId::new(1), b"the second writer, after the first");
    assert!(second.fault().is_none());
}

#[test]
fn a_store_held_by_another_process_is_refused_until_it_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let holder = Holder::spawn(dir.path());

    held_by(Disk::open(dir.path()).unwrap_err(), dir.path());
    held_by(Disk::open_existing(dir.path()).unwrap_err(), dir.path());

    holder.release();
    let disk = Disk::open(dir.path()).unwrap();
    assert!(disk.fault().is_none());
}

#[test]
fn a_writer_that_dies_holding_the_store_does_not_brick_it() {
    let dir = tempfile::tempdir().unwrap();
    let holder = Holder::spawn(dir.path());
    held_by(Disk::open(dir.path()).unwrap_err(), dir.path());

    holder.crash();
    // Nothing to clean up, nothing stale to detect: the lock went with the
    // process, and the store opens as if it had been closed properly.
    let mut disk = Disk::open(dir.path()).unwrap();
    disk.put(AgentId::new(1), b"written after the crash");
    assert!(disk.fault().is_none());
}
