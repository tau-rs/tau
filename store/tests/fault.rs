//! A store that cannot write, seen from the harness (ADR-0012 §1).
//!
//! `put` and `shred` are infallible by contract, so `Disk` records its
//! first I/O failure and the kernel asks after every write. These tests
//! inject a real failure — a read-only `keys/`, which is what a store on a
//! full or unwritable volume looks like from the kernel's side — and check
//! that the harness hears it without holding the store, which `boot_with`
//! took from it by value.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{run, tokio_spawner, CHILD_RESULT, ROOT_RESULT};
use tau_kernel::abi::{Budget, DimKey, Namespace};
use tau_kernel::blob::digest;
use tau_kernel::kernel::{Kernel, KernelError, ShredError};
use tau_kernel::log::{Entry, Log};
use tau_kernel::syscall::program;
use tau_store::Disk;

const RESULT: &[u8] = b"a result the disk will not take";

/// Makes `dir` unwritable and says whether that actually bites. It does
/// not for root, for whom permission bits are advice, and a test that
/// cannot inject the failure must skip rather than assert a lie.
fn sealed_shut(dir: &Path) -> bool {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.join(".probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            fs::remove_file(&probe).unwrap();
            false
        }
        Err(_) => true,
    }
}

/// Lets `dir` be written again, so the temporary directory can be removed.
fn reopen(dir: &Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn a_put_the_store_cannot_write_faults_the_kernel() {
    let tmp = tempfile::tempdir().unwrap();
    let disk = Disk::open(tmp.path()).unwrap();
    let keys = tmp.path().join("keys");
    if !sealed_shut(&keys) {
        eprintln!("skipped: {} stays writable", keys.display());
        return;
    }

    let kernel = Kernel::boot_with(
        Log::with_sink(Vec::new()).unwrap(),
        tokio_spawner,
        Box::new(disk),
    );
    kernel
        .spawn_root(
            program(|root| async move { root.exit(RESULT) }),
            Namespace::from_caps([]),
            Budget::from_dims([(DimKey::Tokens, 100)]),
        )
        .unwrap();
    let drained = kernel.drained().await;
    let asked = kernel.fault();
    let entries = kernel.entries();
    kernel.shutdown();
    reopen(&keys);

    let Err(KernelError::Faulted { reason }) = drained else {
        panic!("a store that cannot write faults the run: {drained:?}");
    };
    assert!(
        reason.starts_with("blob store: put of"),
        "the reason names the write that failed: {reason}"
    );
    assert_eq!(
        asked.as_ref(),
        Some(&reason),
        "the harness can ask the kernel, holding no store of its own"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, Entry::Exited { .. })),
        "the exit is not logged: the log never names a payload the store did not take"
    );
    assert_eq!(
        kernel.read(digest(RESULT)),
        None,
        "and the payload reads as missing, which is what the fault is for"
    );
}

#[tokio::test]
async fn a_shred_that_cannot_drop_a_key_is_not_reported_as_erasure() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run(
        Box::new(Disk::open(tmp.path()).unwrap()),
        CHILD_RESULT,
        ROOT_RESULT,
    )
    .await;
    let keys = tmp.path().join("keys");
    if !sealed_shut(&keys) {
        eprintln!("skipped: {} stays writable", keys.display());
        return;
    }

    let shredded = run.kernel.shred(run.child);
    let asked = run.kernel.fault();
    let still_readable = run.kernel.read(digest(CHILD_RESULT)).is_some();
    reopen(&keys);

    let Err(ShredError::Faulted { reason }) = shredded else {
        panic!("a key that could not be dropped is not an erasure: {shredded:?}");
    };
    assert!(
        reason.starts_with("blob store: shred of"),
        "the reason names the shred that failed: {reason}"
    );
    assert_eq!(asked.as_ref(), Some(&reason), "the kernel faulted with it");
    assert!(
        still_readable,
        "the key is still there, and the harness was told so rather than told `erased`"
    );
}

#[tokio::test]
async fn a_store_that_writes_cleanly_never_faults() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run(
        Box::new(Disk::open(tmp.path()).unwrap()),
        CHILD_RESULT,
        ROOT_RESULT,
    )
    .await;
    assert_eq!(run.kernel.fault(), None, "a whole run, nothing to report");
    assert_eq!(run.kernel.shred(run.child), Ok(()));
    assert_eq!(run.kernel.fault(), None, "and a shred that worked");
}
