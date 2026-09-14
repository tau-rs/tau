//! The `soak` binary, driven the way `tier2.yml` drives it: a run that
//! writes the log and its hash to disk, then a refold of that file that
//! agrees — or does not — with the hash it was handed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn soak() -> Command {
    Command::new(env!("CARGO_BIN_EXE_soak"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tau-soak-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn soak_writes_a_log_whose_refold_matches_the_hash() {
    let dir = scratch("roundtrip");
    let log = dir.join("log.jsonl");
    let hash = dir.join("hash.txt");
    let out = soak()
        .args(["soak", "--seed", "7", "--events", "2000"])
        .arg("--log")
        .arg(&log)
        .arg("--hash")
        .arg(&hash)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("entries="), "no summary line: {stdout}");
    let written = std::fs::read_to_string(&hash).unwrap();
    assert_eq!(written.trim().len(), 64, "a hex sha256: {written:?}");

    let out = soak()
        .arg("refold")
        .arg("--log")
        .arg(&log)
        .arg("--expect")
        .arg(&hash)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn refold_fails_when_the_hash_disagrees() {
    let dir = scratch("mismatch");
    let log = dir.join("log.jsonl");
    let hash = dir.join("hash.txt");
    let out = soak()
        .args(["soak", "--seed", "7", "--events", "500"])
        .arg("--log")
        .arg(&log)
        .arg("--hash")
        .arg(&hash)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::write(&hash, format!("{:0>64}\n", "0")).unwrap();
    let out = soak()
        .arg("refold")
        .arg("--log")
        .arg(&log)
        .arg("--expect")
        .arg(&hash)
        .output()
        .unwrap();
    assert!(!out.status.success(), "a mismatch must be an error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mismatch"), "{stderr}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_bad_argument_is_an_error() {
    let out = soak().args(["soak", "--events", "abc"]).output().unwrap();
    assert!(!out.status.success());
}
