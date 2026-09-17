//! Snapshots over the corpus and the fixtures (ADR-0011 §3, §5): every log
//! replays from each of its own snapshots to the same hash a full fold gives,
//! and a snapshot that does not belong to a log is refused at the join.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::integer_division
)]

use std::fs;
use std::path::{Path, PathBuf};

use tau_kernel::abi::{SnapshotHeader, ABI};
use tau_kernel::log::Log;
use tau_kernel::reducer::{fold, State, FOLD};
use tau_kernel::snapshot::{JoinError, Snapshot, SnapshotError};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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

fn read(path: &Path) -> Log {
    Log::read_from(fs::read(path).unwrap().as_slice()).unwrap()
}

/// The edges and the middle: `0`, `1`, `⌊n/2⌋`, `n−1`, `n`, deduplicated.
fn offsets(n: usize) -> Vec<usize> {
    let mut ks = vec![0, 1, n / 2, n.saturating_sub(1), n];
    ks.sort_unstable();
    ks.dedup();
    ks.retain(|k| *k <= n);
    ks
}

/// Snapshot `log` at `k`, write it, read it back, join it, fold the tail.
fn replay_from(log: &Log, k: usize) -> State {
    let head = fold(&log.entries()[..k]).unwrap();
    let snapshot = head.snapshot(log.prefix_at(k).unwrap());
    assert_eq!(snapshot.header().seq, k as u64);
    let mut bytes = Vec::new();
    snapshot.write_to(&mut bytes).unwrap();
    assert_eq!(
        bytes.iter().filter(|b| **b == b'\n').count(),
        2,
        "two lines, nothing else"
    );
    let restored = Snapshot::read_from(bytes.as_slice()).unwrap();
    assert_eq!(restored.header(), snapshot.header());
    let mut state = restored.join(log).unwrap();
    for entry in &log.entries()[k..] {
        state.apply(entry).unwrap();
    }
    state
}

#[test]
fn every_corpus_log_replays_from_each_of_its_snapshots() {
    let logs = logs_in(&corpus());
    assert!(logs.len() >= 10, "corpus holds {} logs", logs.len());
    for path in logs {
        let expected = fs::read_to_string(path.with_extension("hash"))
            .unwrap()
            .trim()
            .to_owned();
        let log = read(&path);
        for k in offsets(log.len()) {
            let hash = replay_from(&log, k).hash().to_string();
            assert_eq!(hash, expected, "{} from {k}", path.display());
        }
    }
}

#[test]
fn every_fixture_replays_from_each_of_its_snapshots() {
    let logs = logs_in(&fixtures());
    assert_eq!(logs.len(), 5, "the five milestone fixtures");
    for path in logs {
        let log = read(&path);
        let full = fold(log.entries()).unwrap().hash();
        for k in offsets(log.len()) {
            assert_eq!(
                replay_from(&log, k).hash(),
                full,
                "{} from {k}",
                path.display()
            );
        }
    }
}

#[test]
fn a_snapshot_at_zero_is_the_initial_state_and_the_header_line_alone() {
    let log = read(&corpus().join("m1b-wall.log"));
    let snapshot = State::initial().snapshot(log.prefix_at(0).unwrap());
    let header = snapshot.header();
    assert_eq!(header.magic, SnapshotHeader::MAGIC);
    assert_eq!(header.abi, ABI);
    assert_eq!(header.fold, FOLD);
    assert_eq!(header.seq, 0);
    assert_eq!(header.state, State::initial().hash().to_string());
    // The digest at zero is over the header line only: the same for every
    // log written at the same abi, and different from the digest one entry in.
    let mut empty = Vec::new();
    Log::in_memory().write_to(&mut empty).unwrap();
    let other = Log::read_from(empty.as_slice()).unwrap();
    assert_ne!(
        other.prefix_at(0),
        log.prefix_at(0),
        "abi 0 vs abi 2 headers"
    );
    assert_ne!(log.prefix_at(0), log.prefix_at(1));
    assert_eq!(log.prefix_at(log.len()), Some(log.prefix()));
    assert_eq!(log.prefix_at(log.len() + 1), None);
}

#[test]
fn the_prefix_digest_is_the_same_appended_or_read() {
    // ADR-0011 §1: maintained line by line by a writer that never had the
    // file, and rebuilt from parsed entries by a reader that never appended.
    let log = read(&corpus().join("m2a-hooks.log"));
    let mut appended = Log::in_memory();
    for entry in log.entries() {
        appended.append(entry.clone()).unwrap();
    }
    // Different header abi (0 vs current), so the digests differ at 0 ...
    assert_ne!(appended.prefix_at(0), log.prefix_at(0));
    // ... but a re-read of what was appended matches the appender at every k.
    let mut bytes = Vec::new();
    appended.write_to(&mut bytes).unwrap();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    for k in 0..=reread.len() {
        assert_eq!(reread.prefix_at(k), appended.prefix_at(k), "at {k}");
    }
    assert_eq!(reread.prefix(), appended.prefix());
}

// --- refusals, at the library level ------------------------------------------

fn snapshot_of(log: &Log, k: usize) -> Snapshot {
    fold(&log.entries()[..k])
        .unwrap()
        .snapshot(log.prefix_at(k).unwrap())
}

fn lines_of(snapshot: &Snapshot) -> (String, String) {
    let mut bytes = Vec::new();
    snapshot.write_to(&mut bytes).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let mut lines = text.lines();
    (
        lines.next().unwrap().to_owned(),
        lines.next().unwrap().to_owned(),
    )
}

fn with_header(header: &SnapshotHeader, body: &str) -> Result<Snapshot, SnapshotError> {
    let text = format!("{}\n{body}\n", serde_json::to_string(header).unwrap());
    Snapshot::read_from(text.as_bytes())
}

#[test]
fn read_from_refuses_a_header_this_build_cannot_use_before_the_body() {
    let log = read(&corpus().join("m1b-wall.log"));
    let good = snapshot_of(&log, 4);
    let (_, body) = lines_of(&good);

    let mut magic = good.header().clone();
    magic.magic = *b"TAU\0";
    assert!(matches!(
        with_header(&magic, &body),
        Err(SnapshotError::Magic { found, expected }) if found == *b"TAU\0" && expected == *b"TAUS"
    ));

    let mut abi = good.header().clone();
    abi.abi = ABI + 1;
    assert!(matches!(
        with_header(&abi, &body),
        Err(SnapshotError::Abi { found, reads }) if found == ABI + 1 && reads == ABI
    ));

    let mut fold_v = good.header().clone();
    fold_v.fold = FOLD + 1;
    assert!(matches!(
        with_header(&fold_v, &body),
        Err(SnapshotError::Fold { found, expected }) if found == FOLD + 1 && expected == FOLD
    ));

    // The header checks come first: a bad header with a bad body is refused
    // for the header, and only a good header gets its body parsed.
    assert!(matches!(
        with_header(&fold_v, "not json"),
        Err(SnapshotError::Fold { .. })
    ));
    assert!(matches!(
        with_header(good.header(), "not json"),
        Err(SnapshotError::MalformedState(_))
    ));
    assert!(matches!(
        with_header(good.header(), "{\"next_seq\":4}"),
        Err(SnapshotError::MalformedState(_))
    ));
    let (header_line, _) = lines_of(&good);
    assert!(matches!(
        Snapshot::read_from(format!("{header_line}\n").as_bytes()),
        Err(SnapshotError::MissingState)
    ));
    assert!(matches!(
        Snapshot::read_from(b"".as_slice()),
        Err(SnapshotError::MissingHeader)
    ));
    assert!(matches!(
        Snapshot::read_from(b"nope\n".as_slice()),
        Err(SnapshotError::MalformedHeader(_))
    ));
    // An older abi is fine: the shapes are ones this build knows.
    let mut older = good.header().clone();
    older.abi = ABI - 1;
    assert!(with_header(&older, &body).is_ok());
}

#[test]
fn join_refuses_in_order_naming_the_check_and_both_values() {
    let log = read(&corpus().join("m1b-wall.log"));
    let other = read(&corpus().join("m1a-cancel.log"));
    let good = snapshot_of(&log, 4);
    let (_, body) = lines_of(&good);

    // seq: past the end, before any hashing.
    let mut beyond = good.header().clone();
    beyond.seq = log.len() as u64 + 1;
    let snapshot = with_header(&beyond, &body).unwrap();
    assert_eq!(
        snapshot.join(&log),
        Err(JoinError::BeyondEnd {
            seq: log.len() as u64 + 1,
            len: log.len()
        })
    );

    // prefix: another log at the same offset.
    let expected = good.header().prefix.clone();
    let found = hex(other.prefix_at(4).unwrap());
    assert_ne!(expected, found);
    assert_eq!(
        good.clone().join(&other),
        Err(JoinError::Prefix {
            seq: 4,
            expected,
            found
        })
    );

    // state: the header claims a hash the body does not re-hash to.
    let mut stated = good.header().clone();
    stated.state = "00".repeat(32);
    let snapshot = with_header(&stated, &body).unwrap();
    assert_eq!(
        snapshot.join(&log),
        Err(JoinError::StateHash {
            expected: "00".repeat(32),
            found: good.state().hash()
        })
    );

    // next_seq: header and body agree on everything but the offset. The
    // body's hash must still be the header's, so the body is edited and
    // the header re-stated from it.
    let mut edited: serde_json::Value = serde_json::from_str(&body).unwrap();
    edited["next_seq"] = serde_json::Value::from(3);
    let moved: State = serde_json::from_value(edited).unwrap();
    let mut header = good.header().clone();
    header.state = moved.hash().to_string();
    let snapshot = with_header(&header, &serde_json::to_string(&moved).unwrap()).unwrap();
    assert_eq!(
        snapshot.join(&log),
        Err(JoinError::NextSeq { header: 4, body: 3 })
    );

    // And the good one joins, positioned at 4.
    let state = good.join(&log).unwrap();
    assert_eq!(state.next_seq().get(), 4);
}

fn hex(digest: [u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
