//! The disk store's own contract (ADR-0012 §4, §6): the header, the
//! layout, sealing, and what a reopened store can answer. No kernel here;
//! the port is driven directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use tau_kernel::abi::{AgentId, BlobRef};
use tau_kernel::blob::{digest, Blobs};
use tau_store::{Disk, OpenError, Status, AEAD, DIGEST, MAGIC, VERSION};

const A: &[u8] = b"the first payload";
const B: &[u8] = b"the second payload";
const KEY: [u8; 32] = [7; 32];
const OTHER_KEY: [u8; 32] = [9; 32];

fn owner(n: u64) -> AgentId {
    AgentId::new(n)
}

fn object(root: &Path, blob: &BlobRef, owner: AgentId) -> PathBuf {
    root.join("objects")
        .join(blob.to_hex())
        .join(owner.get().to_string())
}

fn key(root: &Path, owner: AgentId) -> PathBuf {
    root.join("keys").join(owner.get().to_string())
}

/// A store whose owner `n` has a known key, so sealing is reproducible.
fn store_with_key(root: &Path, n: u64, key_bytes: &[u8; 32]) -> Disk {
    let disk = Disk::open(root).unwrap();
    fs::write(key(root, owner(n)), key_bytes).unwrap();
    disk
}

#[test]
fn a_fresh_directory_becomes_a_store_with_the_named_header() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run-1");
    let disk = Disk::open(&path).unwrap();
    assert_eq!(disk.path(), path);
    let header = fs::read_to_string(path.join("STORE")).unwrap();
    assert_eq!(
        header,
        format!(r#"{{"aead":"{AEAD}","digest":"{DIGEST}","magic":"{MAGIC}","v":{VERSION}}}"#)
    );
    assert_eq!(
        header,
        r#"{"aead":"xchacha20poly1305","digest":"sha256","magic":"TAUB","v":1}"#
    );
    assert!(path.join("objects").is_dir());
    assert!(path.join("keys").is_dir());
    assert!(disk.fault().is_none());
}

#[test]
fn a_reopened_store_reads_what_it_wrote_and_not_what_it_shredded() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (digest(A), digest(B));
    let never = digest(b"never stored");
    {
        let mut disk = Disk::open(dir.path()).unwrap();
        assert_eq!(disk.put(owner(1), A), a);
        assert_eq!(disk.put(owner(2), B), b);
        assert_eq!(disk.status(&a), Status::Present);
        disk.shred(owner(1));
        disk.shred(owner(1));
        assert!(disk.fault().is_none(), "{:?}", disk.fault());
    }
    let disk = Disk::open(dir.path()).unwrap();
    assert_eq!(disk.status(&a), Status::Shredded);
    assert_eq!(disk.get(&a), None);
    assert_eq!(disk.status(&b), Status::Present);
    assert_eq!(disk.get(&b).as_deref(), Some(B));
    assert_eq!(disk.status(&never), Status::Absent);
    assert_eq!(disk.get(&never), None);
    assert_eq!(
        disk.shredded().unwrap(),
        vec![owner(1)],
        "one tombstone, not two"
    );
    assert!(!key(dir.path(), owner(1)).exists());
    assert!(key(dir.path(), owner(2)).exists());
    assert!(
        object(dir.path(), &a, owner(1)).exists(),
        "the object outlives its key"
    );
}

#[test]
fn deleting_the_keys_directory_shreds_everything() {
    // The backup argument of §4: `objects/` without `keys/` is worthless.
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (digest(A), digest(B));
    {
        let mut disk = Disk::open(dir.path()).unwrap();
        disk.put(owner(1), A);
        disk.put(owner(2), B);
    }
    let sealed_a = fs::read(object(dir.path(), &a, owner(1))).unwrap();
    let sealed_b = fs::read(object(dir.path(), &b, owner(2))).unwrap();
    fs::remove_dir_all(dir.path().join("keys")).unwrap();

    let disk = Disk::open(dir.path()).unwrap();
    assert_eq!(disk.get(&a), None);
    assert_eq!(disk.get(&b), None);
    assert_eq!(disk.status(&a), Status::Shredded);
    assert_eq!(disk.status(&b), Status::Shredded);
    assert_eq!(
        fs::read(object(dir.path(), &a, owner(1))).unwrap(),
        sealed_a
    );
    assert_eq!(
        fs::read(object(dir.path(), &b, owner(2))).unwrap(),
        sealed_b
    );
    assert_eq!(disk.get(&BlobRef::EMPTY), Some(Vec::new()));
}

#[test]
fn sealed_bytes_are_a_function_of_key_and_plaintext() {
    // The nonce is the reference and there is no randomness on the write
    // path: two stores with the same key seal the same bytes identically.
    let (one, two, three) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let a = digest(A);
    let mut first = store_with_key(one.path(), 1, &KEY);
    let mut second = store_with_key(two.path(), 1, &KEY);
    let mut other = store_with_key(three.path(), 1, &OTHER_KEY);
    first.put(owner(1), A);
    second.put(owner(1), A);
    other.put(owner(1), A);
    let sealed = fs::read(object(one.path(), &a, owner(1))).unwrap();
    assert_eq!(sealed, fs::read(object(two.path(), &a, owner(1))).unwrap());
    assert_ne!(
        sealed,
        fs::read(object(three.path(), &a, owner(1))).unwrap()
    );
    assert_ne!(sealed, A, "sealed, not stored in the clear");
    assert_eq!(sealed.len(), A.len() + 16, "ciphertext and tag, no nonce");

    first.put(owner(1), A);
    assert_eq!(
        fs::read(object(one.path(), &a, owner(1))).unwrap(),
        sealed,
        "a second put of the same bytes is the same file"
    );
    assert_eq!(first.get(&a).as_deref(), Some(A));
    assert_eq!(other.get(&a).as_deref(), Some(A));
}

#[test]
fn a_copy_does_not_open_under_another_key_reference_or_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (digest(A), digest(B));
    let mut disk = store_with_key(dir.path(), 1, &KEY);
    fs::write(key(dir.path(), owner(2)), KEY).unwrap();
    disk.put(owner(1), A);
    let sealed = fs::read(object(dir.path(), &a, owner(1))).unwrap();

    // Another reference: the nonce and the AAD both differ.
    fs::create_dir_all(object(dir.path(), &b, owner(1)).parent().unwrap()).unwrap();
    fs::write(object(dir.path(), &b, owner(1)), &sealed).unwrap();
    assert_eq!(disk.get(&b), None);
    assert_eq!(
        disk.status(&b),
        Status::Shredded,
        "a copy that does not open"
    );

    // Another owner with the very same key: the AAD names the owner.
    fs::write(object(dir.path(), &a, owner(2)), &sealed).unwrap();
    fs::remove_file(object(dir.path(), &a, owner(1))).unwrap();
    assert_eq!(disk.get(&a), None);

    // Back under its own name, but another key.
    fs::rename(
        object(dir.path(), &a, owner(2)),
        object(dir.path(), &a, owner(1)),
    )
    .unwrap();
    fs::write(key(dir.path(), owner(1)), OTHER_KEY).unwrap();
    let reopened = Disk::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&a), None);
    fs::write(key(dir.path(), owner(1)), KEY).unwrap();
    assert_eq!(
        reopened.get(&a).as_deref(),
        Some(A),
        "and it opens again under its key"
    );
}

#[test]
fn a_store_header_it_cannot_read_is_refused() {
    let good = serde_json::json!({"magic": MAGIC, "v": VERSION, "digest": DIGEST, "aead": AEAD});
    let cases: [(&str, serde_json::Value); 5] = [
        ("magic", "TAUX".into()),
        ("v", 2.into()),
        ("v", "1".into()),
        ("digest", "blake3".into()),
        ("aead", "aes256gcm".into()),
    ];
    for (field, bad) in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut header = good.clone();
        header
            .as_object_mut()
            .unwrap()
            .insert(field.into(), bad.clone());
        fs::write(dir.path().join("STORE"), header.to_string()).unwrap();
        match Disk::open(dir.path()) {
            Err(OpenError::Header { field: got, value }) => {
                assert_eq!(got, field);
                assert_eq!(value, bad.to_string(), "the value as written");
            }
            other => panic!("{field}={bad}: {other:?}"),
        }
    }

    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("STORE"), "not json").unwrap();
    assert!(matches!(
        Disk::open(dir.path()),
        Err(OpenError::Malformed(_))
    ));

    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("something-else"), "x").unwrap();
    assert!(
        matches!(Disk::open(dir.path()), Err(OpenError::NotAStore(_))),
        "a non-empty directory with no header is not initialised over"
    );
    assert!(!dir.path().join("STORE").exists());
}

#[test]
fn the_empty_reference_is_never_stored_and_never_shredded() {
    let dir = tempfile::tempdir().unwrap();
    let mut disk = Disk::open(dir.path()).unwrap();
    assert_eq!(disk.put(owner(1), b""), BlobRef::EMPTY);
    assert_eq!(fs::read_dir(dir.path().join("objects")).unwrap().count(), 0);
    assert!(!key(dir.path(), owner(1)).exists(), "no key for nothing");
    assert_eq!(disk.get(&BlobRef::EMPTY), Some(Vec::new()));
    assert_eq!(disk.status(&BlobRef::EMPTY), Status::Present);
    disk.shred(owner(1));
    assert_eq!(disk.get(&BlobRef::EMPTY), Some(Vec::new()));
    assert_eq!(disk.status(&BlobRef::EMPTY), Status::Present);
    assert_eq!(fs::read_dir(dir.path().join("objects")).unwrap().count(), 0);
}
