//! The known-answer fixture: a store directory sealed by an older build,
//! committed, and opened by this one (#167).
//!
//! Every other test in this crate seals and opens inside one build, which
//! proves the cipher round-trips and nothing about *yesterday's* bytes. A
//! dependency bump that moved the sealed layout — a nonce rule, the tag's
//! place, a cipher or digest swap — would leave all of them green and
//! silently orphan every store on disk. The store's version of "we do not
//! break userspace" needs an artefact from the past, so here is one:
//! `tests/fixtures/known-answer-v1/` holds a store directory recorded once,
//! with a fixed owner key, and three questions are asked of it.
//!
//! | Assertion | Catches |
//! |---|---|
//! | the committed copy opens to the committed plaintext | cipher, nonce, layout drift |
//! | `put` of that plaintext yields the committed reference | digest drift (`sha2`) |
//! | sealing under the committed key reproduces the committed bytes | any change to the sealed format |
//!
//! The third row is only askable because sealing here is deterministic: the
//! nonce is the reference and never random (ADR-0012 §4), so the ciphertext
//! is a pure function of key and plaintext.
//!
//! **Nothing here re-records itself.** The fixture is deliberately outside
//! the `TAU_UPDATE_FIXTURES` flow, which re-records every fixture in the
//! run: an on-disk compatibility oracle that rewrites itself on mismatch
//! proves nothing. Re-recording is [`record_the_known_answer_fixture`],
//! `#[ignore]`d and run by name, and it cannot even finish without a human
//! editing [`REF_HEX`] by hand. See the fixture's `README.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use tau_kernel::abi::{AgentId, BlobRef};
use tau_kernel::blob::{digest, Blobs};
use tau_store::Disk;

/// The owner the committed copy is sealed for. Not 1: the owner id goes
/// into the additional data as eight big-endian bytes, and an id with
/// several non-zero bytes fails loudly if that ever becomes little-endian.
const OWNER: u64 = 1_234_567_890;

/// The reference the committed plaintext hashes to, and the name of its
/// directory under `objects/`. A change here is a change to the digest
/// behind every `BlobRef` on disk: read the diff, do not absorb it.
const REF_HEX: &str = "785881713a50131ad3a9f158972ed05f44ce4766ebcf1a88d4d09e021b533ef6";

/// The fixture's owner key, ascending bytes: a key no store would ever
/// generate, so a fixture key can never be mistaken for a live one, and a
/// key read at the wrong offset or in the wrong order fails to open.
const KEY: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];

/// The sentence the plaintext opens with. The rest is every byte value
/// once, so the payload crosses four ChaCha20 blocks and holds bytes no
/// text encoding would survive: a keystream counter or a block-order drift
/// shows up here instead of hiding inside a payload shorter than one block.
const SENTENCE: &[u8] = b"tau store v1 known-answer fixture: sealed once, opened forever.\n";

/// What a failure of any test in this file means, said once.
const MEANING: &str = "the sealed format moved: a store written by an older \
    build no longer reads. See store/tests/fixtures/known-answer-v1/README.md \
    before touching this fixture.";

fn owner() -> AgentId {
    AgentId::new(OWNER)
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/known-answer-v1")
}

fn expected_ref() -> BlobRef {
    BlobRef::from_hex(REF_HEX).unwrap()
}

/// The committed plaintext, as recorded.
fn plaintext() -> Vec<u8> {
    fs::read(fixture_dir().join("plaintext.bin")).unwrap()
}

/// The committed sealed copy: ciphertext and tag, no nonce.
fn committed_seal() -> Vec<u8> {
    fs::read(
        fixture_dir()
            .join("store/objects")
            .join(REF_HEX)
            .join(OWNER.to_string()),
    )
    .unwrap()
}

/// The store directory as it was recorded, copied somewhere writable so a
/// test can never write into the source tree.
fn fixture_store(into: &Path) -> PathBuf {
    let root = into.join("known-answer-v1");
    copy_dir(&fixture_dir().join("store"), &root);
    root
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A fresh, empty store whose owner holds the fixture's key, so sealing in
/// it is the sealing that produced the fixture.
fn store_with_the_fixture_key(root: &Path) -> Disk {
    let disk = Disk::open(root).unwrap();
    fs::write(root.join("keys").join(OWNER.to_string()), KEY).unwrap();
    disk
}

#[test]
fn a_blob_sealed_by_an_older_build_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_store(dir.path());

    // The header first: a reader that refuses it never reaches an object,
    // so a `v`, `digest` or `aead` that moved must be read as a break.
    let fresh = tempfile::tempdir().unwrap();
    let today = fresh.path().join("today");
    drop(Disk::open(&today).unwrap());
    assert_eq!(
        fs::read_to_string(root.join("STORE")).unwrap(),
        fs::read_to_string(today.join("STORE")).unwrap(),
        "{MEANING}"
    );

    let disk = Disk::open(&root).unwrap();
    assert_eq!(
        disk.get(&expected_ref()).as_deref(),
        Some(plaintext().as_slice()),
        "{MEANING}"
    );
    assert_eq!(disk.status(&expected_ref()), tau_store::Status::Present);
    assert!(disk.fault().is_none(), "{:?}", disk.fault());
}

#[test]
fn the_committed_plaintext_still_digests_to_the_committed_reference() {
    let plaintext = plaintext();
    assert_eq!(plaintext.len(), SENTENCE.len() + 256, "{MEANING}");
    assert!(plaintext.starts_with(SENTENCE), "{MEANING}");
    assert_eq!(digest(&plaintext), expected_ref(), "{MEANING}");

    // And through the port, which is what fills `objects/`: a put of these
    // bytes today files them under the name they are filed under on disk.
    let dir = tempfile::tempdir().unwrap();
    let mut disk = Disk::open(dir.path()).unwrap();
    assert_eq!(disk.put(owner(), &plaintext), expected_ref(), "{MEANING}");

    // One reference in the fixture, and it is that one: a re-record that
    // left the old directory behind would otherwise pass unnoticed.
    let filed: Vec<_> = fs::read_dir(fixture_dir().join("store/objects"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(filed, vec![std::ffi::OsString::from(REF_HEX)]);
}

#[test]
fn sealing_the_committed_plaintext_reproduces_the_committed_bytes() {
    // The strongest of the three, and only possible because the nonce is
    // the reference and never random (ADR-0012 §4): today's build, the
    // fixture's key, the fixture's plaintext, and the same ciphertext down
    // to the last byte of the tag.
    let dir = tempfile::tempdir().unwrap();
    let mut disk = store_with_the_fixture_key(dir.path());
    let plaintext = plaintext();
    let blob = disk.put(owner(), &plaintext);
    assert!(disk.fault().is_none(), "{:?}", disk.fault());

    let resealed = fs::read(
        dir.path()
            .join("objects")
            .join(blob.to_hex())
            .join(OWNER.to_string()),
    )
    .unwrap();
    let committed = committed_seal();
    assert_eq!(
        resealed.len(),
        plaintext.len() + 16,
        "{MEANING} (ciphertext and tag, no nonce)"
    );
    assert_eq!(resealed, committed, "{MEANING}");
}

/// Re-records the fixture. `#[ignore]`d on purpose: it is a deliberate act,
/// never part of a test run and never reached by `TAU_UPDATE_FIXTURES`.
///
/// ```text
/// cargo test -p tau-store --test known_answer -- --ignored --nocapture \
///     record_the_known_answer_fixture
/// ```
///
/// It rebuilds the store directory from scratch, prints the reference it
/// filed the payload under, and then fails if that is not [`REF_HEX`] — so
/// a digest change cannot land without someone editing this file by hand
/// and reading `git diff -- store/tests/fixtures`.
#[test]
#[ignore = "re-records the committed fixture: run by name, on purpose"]
fn record_the_known_answer_fixture() {
    let dir = fixture_dir();
    let plaintext = [SENTENCE.to_vec(), (0u8..=255).collect()].concat();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("plaintext.bin"), &plaintext).unwrap();

    let root = dir.join("store");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let mut disk = store_with_the_fixture_key(&root);
    let blob = disk.put(owner(), &plaintext);
    assert!(disk.fault().is_none(), "{:?}", disk.fault());

    println!("recorded {}", root.display());
    println!("  reference: {}", blob.to_hex());
    assert_eq!(
        blob.to_hex(),
        REF_HEX,
        "the reference moved. This is a store format change: set REF_HEX to \
         the reference printed above, say why in the commit, and read the diff."
    );
}
