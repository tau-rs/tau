//! `tau shred` beside `Kernel::shred` (ADR-0012 §5, §6): a shred leaves the
//! log byte-for-byte as written, `tau replay` over that log prints the hash
//! the live kernel had, and the verb refuses a live subtree exactly as the
//! kernel does. `store/tests/shred.rs` runs the kernel-side suite on both
//! stores; this file is the operator's path to the same erasure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs;

use common::{
    finished, live, path, replay_hash, stderr, stdout, tau, CHILD_MSG, CHILD_RESULT, ROOT_MSG,
    ROOT_RESULT,
};
use tau_kernel::blob::{digest, Memory};
use tau_store::{Disk, Status};

#[tokio::test]
async fn a_shred_is_invisible_to_the_fold() {
    let untouched = finished(Box::new(Memory::new())).await;
    let shredded = finished(Box::new(Memory::new())).await;
    let result = digest(CHILD_RESULT);
    assert_eq!(
        shredded.kernel().read(result).as_deref(),
        Some(CHILD_RESULT)
    );

    shredded.kernel().shred(shredded.child).unwrap();

    assert_eq!(
        shredded.kernel().read(result),
        None,
        "the child's result is gone"
    );
    assert_eq!(
        shredded.kernel().read(digest(ROOT_MSG)).as_deref(),
        Some(ROOT_MSG),
        "the root's request is not"
    );
    assert_eq!(
        shredded.sink.contents(),
        untouched.sink.contents(),
        "the logs are the same bytes"
    );
    assert_eq!(
        shredded.kernel().state_hash(),
        untouched.kernel().state_hash()
    );

    let log = shredded.write_log("shred-invisible-to-the-fold.log");
    assert_eq!(
        replay_hash(&log),
        untouched.kernel().state_hash().to_string(),
        "`tau replay` over the shredded run's log prints the untouched run's hash"
    );
}

#[tokio::test]
async fn tau_shred_drops_the_subtrees_keys_and_the_fold_does_not_see_it() {
    let untouched = finished(Box::new(Memory::new())).await;
    let dir = tempfile::tempdir().unwrap();
    let mut run = finished(Box::new(Disk::open(dir.path()).unwrap())).await;
    run.release().await;
    let log = run.write_log("tau-shred-drops-keys.log");
    let before = fs::read(&log).unwrap();
    let child = run.child.to_string();

    let out = tau(&["shred", path(&log), path(dir.path()), &child]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(&format!("shredded=1 root={child} agents: {child}")),
        "{}",
        stdout(&out)
    );

    let disk = Disk::open_existing(dir.path()).unwrap();
    assert_eq!(disk.status(&digest(CHILD_RESULT)), Status::Shredded);
    assert_eq!(disk.status(&digest(ROOT_MSG)), Status::Present);
    assert_eq!(disk.status(&digest(ROOT_RESULT)), Status::Present);
    assert_eq!(disk.shredded().unwrap(), vec![run.child]);
    assert_eq!(fs::read(&log).unwrap(), before, "the log is untouched");
    assert_eq!(
        replay_hash(&log),
        untouched.kernel().state_hash().to_string(),
        "the fold does not see the shred"
    );
    drop(disk);

    // A second shred is a no-op: same exit, same store.
    let again = tau(&["shred", path(&log), path(dir.path()), &child]);
    assert!(again.status.success(), "{}", stderr(&again));
    let disk = Disk::open_existing(dir.path()).unwrap();
    assert_eq!(disk.status(&digest(CHILD_RESULT)), Status::Shredded);
    assert_eq!(disk.shredded().unwrap(), vec![run.child], "one tombstone");
}

#[tokio::test]
async fn tau_shred_of_the_root_takes_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = finished(Box::new(Disk::open(dir.path()).unwrap())).await;
    run.release().await;
    let log = run.write_log("tau-shred-root.log");

    let out = tau(&["shred", path(&log), path(dir.path()), &run.root.to_string()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("shredded=2"), "{}", stdout(&out));

    let disk = Disk::open_existing(dir.path()).unwrap();
    for bytes in [CHILD_RESULT, ROOT_MSG, ROOT_RESULT] {
        assert_eq!(disk.status(&digest(bytes)), Status::Shredded);
    }
    drop(disk);
    // The root's id is accepted bare as well as as the kernel prints it.
    let bare = run.root.get().to_string();
    let again = tau(&["shred", path(&log), path(dir.path()), &bare]);
    assert!(again.status.success(), "{}", stderr(&again));
}

/// A running kernel holds its store, and the lock is consulted before the
/// log: the log path here does not exist, and the verb never notices.
#[tokio::test]
async fn tau_shred_refuses_a_store_a_running_kernel_holds_before_reading_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = live(Box::new(Disk::open(dir.path()).unwrap())).await;
    let child = run.child.to_string();
    let child_msg = digest(CHILD_MSG);
    let no_such_log = dir.path().join("no-such.log");

    let out = tau(&["shred", path(&no_such_log), path(dir.path()), &child]);
    assert_eq!(out.status.code(), Some(11), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("store held: {}", dir.path().display())),
        "names the directory: {}",
        stderr(&out)
    );
    assert_eq!(
        run.kernel().read(child_msg).as_deref(),
        Some(CHILD_MSG),
        "nothing was shredded"
    );
    assert!(
        !dir.path().join("shredded").exists(),
        "no tombstone was written"
    );

    // Once the kernel is gone the same command reaches the log, and it is
    // the log's turn to refuse.
    run.finish().await;
    run.release().await;
    let out = tau(&["shred", path(&no_such_log), path(dir.path()), &child]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

/// The fold's own refusal, reached once nobody holds the store: the log as
/// it stood with the child parked, and a kernel that is gone — which is
/// what a kernel that crashed mid-run leaves behind.
#[tokio::test]
async fn tau_shred_refuses_a_live_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = live(Box::new(Disk::open(dir.path()).unwrap())).await;
    let child = run.child.to_string();
    let child_msg = digest(CHILD_MSG);
    assert_eq!(run.kernel().read(child_msg).as_deref(), Some(CHILD_MSG));

    // The log as it stands with the child parked: the fold sees it live.
    let log = run.write_log("tau-shred-live.log");
    run.finish().await;
    run.release().await;
    let out = tau(&["shred", path(&log), path(dir.path()), &child]);
    assert_eq!(out.status.code(), Some(9), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("shred refused: {child} is still live")),
        "names the live agent: {}",
        stderr(&out)
    );
    // From the root too: the root is itself live, waiting on the child, and
    // is the first live agent the walk meets, as in `Kernel::shred`.
    let out = tau(&["shred", path(&log), path(dir.path()), &run.root.to_string()]);
    assert_eq!(out.status.code(), Some(9), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("shred refused: {} is still live", run.root)),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        Disk::open_existing(dir.path()).unwrap().status(&child_msg),
        Status::Present,
        "nothing was shredded"
    );
    assert!(
        !dir.path().join("shredded").exists(),
        "no tombstone was written"
    );

    let log = run.write_log("tau-shred-live-then-finished.log");
    let out = tau(&["shred", path(&log), path(dir.path()), &child]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        Disk::open_existing(dir.path()).unwrap().status(&child_msg),
        Status::Shredded
    );
}

#[tokio::test]
async fn tau_shred_of_an_agent_the_log_does_not_have_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = finished(Box::new(Disk::open(dir.path()).unwrap())).await;
    run.release().await;
    let log = run.write_log("tau-shred-unknown-agent.log");

    let out = tau(&["shred", path(&log), path(dir.path()), "agent:999"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("agent:999 is not an agent in"),
        "{}",
        stderr(&out)
    );

    let out = tau(&["shred", path(&log), path(dir.path()), "seven"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("`seven` is not an agent id"),
        "{}",
        stderr(&out)
    );

    let out = tau(&["shred", path(&log), path(dir.path())]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    assert_eq!(
        Disk::open_existing(dir.path())
            .unwrap()
            .status(&digest(CHILD_RESULT)),
        Status::Present
    );
}

#[tokio::test]
async fn tau_shred_refuses_a_directory_that_is_not_a_store() {
    let run = finished(Box::new(Memory::new())).await;
    let log = run.write_log("tau-shred-not-a-store.log");
    let child = run.child.to_string();

    let empty = tempfile::tempdir().unwrap();
    let out = tau(&["shred", path(&log), path(empty.path()), &child]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not a store (no STORE header)"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        fs::read_dir(empty.path()).unwrap().count(),
        0,
        "an empty directory was not initialised as a store"
    );

    let missing = empty.path().join("nowhere");
    let out = tau(&["shred", path(&log), path(&missing), &child]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(!missing.exists(), "a missing directory was not created");

    let bad = tempfile::tempdir().unwrap();
    fs::write(
        bad.path().join("STORE"),
        br#"{"magic":"TAUB","v":2,"digest":"sha256","aead":"xchacha20poly1305"}"#,
    )
    .unwrap();
    let out = tau(&["shred", path(&log), path(bad.path()), &child]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("v is 2"),
        "names the field and value: {}",
        stderr(&out)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn tau_shred_that_cannot_drop_a_key_is_not_reported_as_erasure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let mut run = finished(Box::new(Disk::open(dir.path()).unwrap())).await;
    run.release().await;
    let log = run.write_log("tau-shred-faulted.log");
    let keys = dir.path().join("keys");
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = keys.join(".probe");
    if fs::write(&probe, b"").is_ok() {
        // Root, or a filesystem that ignores permission bits: the failure
        // cannot be injected, so the test skips rather than assert a lie.
        fs::remove_file(&probe).unwrap();
        fs::set_permissions(&keys, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: {} stays writable", keys.display());
        return;
    }

    let out = tau(&[
        "shred",
        path(&log),
        path(dir.path()),
        &run.child.to_string(),
    ]);
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(10), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("shred could not drop a key: blob store: shred of"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        Disk::open_existing(dir.path())
            .unwrap()
            .status(&digest(CHILD_RESULT)),
        Status::Present,
        "the content is still readable: the failure is reported as a failure"
    );
}
