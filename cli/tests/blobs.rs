//! `tau blobs`: what a store holds, from outside the kernel (ADR-0012 §4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs;

use common::{finished, path, stderr, stdout, tau, CHILD_RESULT, ROOT_MSG, ROOT_RESULT};
use tau_kernel::abi::BlobRef;
use tau_kernel::blob::digest;
use tau_store::Disk;

#[tokio::test]
async fn tau_blobs_lists_every_reference_with_its_status() {
    let dir = tempfile::tempdir().unwrap();
    let run = finished(Box::new(Disk::open(dir.path()).unwrap())).await;
    run.kernel.shred(run.child).unwrap();

    let out = tau(&["blobs", path(dir.path())]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let mut lines = text.lines();
    assert_eq!(
        lines.next().unwrap(),
        format!("store={} references=3 shredded=1", dir.path().display())
    );
    assert_eq!(lines.next().unwrap(), format!("shredded: {}", run.child));
    let mut rest: Vec<&str> = lines.collect();
    rest.sort_unstable();
    let mut expected = vec![
        format!("{} shredded", digest(CHILD_RESULT)),
        format!("{} present", digest(ROOT_MSG)),
        format!("{} present", digest(ROOT_RESULT)),
    ];
    expected.sort_unstable();
    assert_eq!(rest, expected, "{text}");
}

#[tokio::test]
async fn tau_blobs_answers_for_the_references_asked() {
    let dir = tempfile::tempdir().unwrap();
    let run = finished(Box::new(Disk::open(dir.path()).unwrap())).await;
    run.kernel.shred(run.child).unwrap();
    let never = digest(b"never stored").to_hex();
    let empty = BlobRef::EMPTY.to_hex();
    let child = digest(CHILD_RESULT).to_hex();

    let out = tau(&["blobs", path(dir.path()), &never, &empty, &child]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let mut lines = text.lines();
    assert!(
        lines.next().unwrap().ends_with("references=3 shredded=1"),
        "{text}"
    );
    assert_eq!(lines.next().unwrap(), format!("shredded: {}", run.child));
    assert_eq!(lines.next().unwrap(), format!("{never} absent"));
    assert_eq!(
        lines.next().unwrap(),
        format!("{empty} present"),
        "the empty reference is always present"
    );
    assert_eq!(lines.next().unwrap(), format!("{child} shredded"));
    assert_eq!(lines.next(), None);
}

#[test]
fn tau_blobs_of_a_fresh_store_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    drop(Disk::open(dir.path()).unwrap());
    let out = tau(&["blobs", path(dir.path())]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        format!("store={} references=0 shredded=0", dir.path().display())
    );
}

#[test]
fn tau_blobs_refuses_a_bad_reference_and_a_directory_that_is_not_a_store() {
    let dir = tempfile::tempdir().unwrap();
    drop(Disk::open(dir.path()).unwrap());

    let out = tau(&["blobs", path(dir.path()), "abc"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("`abc` is not a payload reference"),
        "{}",
        stderr(&out)
    );

    let out = tau(&["blobs", path(dir.path()), "--verbose"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    let out = tau(&["blobs"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    let empty = tempfile::tempdir().unwrap();
    let out = tau(&["blobs", path(empty.path())]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not a store (no STORE header)"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        fs::read_dir(empty.path()).unwrap().count(),
        0,
        "not initialised"
    );

    fs::write(empty.path().join("STORE"), b"not json").unwrap();
    let out = tau(&["blobs", path(empty.path())]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("STORE header is not JSON"),
        "{}",
        stderr(&out)
    );
}
