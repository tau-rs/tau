//! Tier 1 job 3: the fast determinism check (HANDOFF §8, ADR-0003).
//!
//! Three fixed seeds drive the reducer through a random legal workload; each
//! log is folded twice more — once from memory, once through a JSON
//! round-trip — and every fold must hash the same. This is the canary for
//! reducer nondeterminism, not the proof: the proof is the Tier 2 soak.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{fold, StateHash};
use tau_sim::{run, Report};

/// Accepted entries per seed.
///
/// Each seed must stay under the 5s quick ceiling with room for a cold CI
/// runner, and the cost is quadratic: every entry sweeps every agent record
/// (the conservation check, the tick, the generator's own choosers), and
/// records persist after exit. 10k events was the handoff's guess and ran
/// ~3.2s per seed on an M-series laptop; this size runs well under 1s.
const EVENTS: u64 = 5_000;

const SEED_A: u64 = 0x7a75_0000_0000_0001;
const SEED_B: u64 = 0x7a75_0000_0000_0002;
const SEED_C: u64 = 0x7a75_0000_0000_0003;

/// The hash of a fold over the log as a replay CLI would read it: written out
/// as newline-delimited JSON and parsed back.
fn hash_after_round_trip(log: &Log) -> StateHash {
    let mut bytes = Vec::new();
    log.write_to(&mut bytes).unwrap();
    let read = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(read.len(), log.len());
    fold(read.entries()).unwrap().hash()
}

fn refolds_to_the_same_hash(seed: u64) {
    let report = run(seed, EVENTS).unwrap();
    let incremental = report.state.hash();
    let refold = fold(report.log.entries()).unwrap().hash();
    let round_trip = hash_after_round_trip(&report.log);
    assert_eq!(incremental, refold, "seed {seed:#x}: refold diverged");
    assert_eq!(
        incremental, round_trip,
        "seed {seed:#x}: fold of the serialized log diverged"
    );
    assert!(report.state.is_drained(), "the epilogue drains the tree");
    assert!(report.accepted >= EVENTS);
    assert!(
        report.refused > 0,
        "the generator proposes illegal entries too"
    );
}

#[test]
fn seed_a_refolds_to_the_same_hash() {
    refolds_to_the_same_hash(SEED_A);
}

#[test]
fn seed_b_refolds_to_the_same_hash() {
    refolds_to_the_same_hash(SEED_B);
}

#[test]
fn seed_c_refolds_to_the_same_hash() {
    refolds_to_the_same_hash(SEED_C);
}

#[test]
fn the_generator_is_a_pure_function_of_the_seed() {
    let first = run(SEED_A, 500).unwrap();
    let again = run(SEED_A, 500).unwrap();
    assert_eq!(first.log.entries(), again.log.entries());
    assert_eq!(first.state.hash(), again.state.hash());

    let other = run(SEED_B, 500).unwrap();
    assert_ne!(first.log.entries(), other.log.entries());
}

#[test]
fn every_entry_kind_and_an_overdraft_appear() {
    let Report { log, state, .. } = run(SEED_A, 2_000).unwrap();
    let kind = |e: &Entry| match e {
        Entry::DriverRegistered { .. } => "driver_registered",
        Entry::Spawned { .. } => "spawned",
        Entry::Sent { .. } => "sent",
        Entry::Replied { .. } => "replied",
        Entry::Resolved { .. } => "resolved",
        Entry::Exited { .. } => "exited",
        Entry::Claimed { .. } => "claimed",
        Entry::Cancelled { .. } => "cancelled",
        Entry::Tick { .. } => "tick",
    };
    let seen: std::collections::BTreeSet<&str> = log.entries().iter().map(kind).collect();
    for want in [
        "driver_registered",
        "spawned",
        "sent",
        "replied",
        "resolved",
        "exited",
        "claimed",
        "cancelled",
        "tick",
    ] {
        assert!(
            seen.contains(want),
            "no `{want}` entry in {} entries",
            log.len()
        );
    }
    assert!(
        state.agents().any(|(_, a)| !a.overdraft.is_empty()),
        "some driver reported above its ceiling"
    );
    assert!(
        state
            .agents()
            .any(|(_, a)| a.status == tau_kernel::reducer::Status::Aborted),
        "some agent was hard-aborted at a deadline"
    );
}
