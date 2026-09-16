//! The ABI guard, part one: the wire format is pinned by snapshot.
//!
//! These tests exist so that a change to the serialized shape of the frozen
//! types cannot happen quietly. Renaming a field, reordering an enum,
//! tightening a representation — each of them shows up here as a diff that a
//! human has to accept on purpose, and CI's diff gate then demands either an
//! [`ABI`] bump or an `abi-change` label with a linked ADR.
//!
//! A failing snapshot is not a broken test. It is the ABI telling you that you
//! changed it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::abi::{
    AgentId, BlobRef, Budget, Capability, Consumption, Corr, DimKey, DriverId, Endpoint, Entry,
    FailureMode, HookId, HookPoint, HookSource, LogHeader, Msg, MsgKind, Name, Namespace, Ruling,
    Seq, SnapshotHeader, ABI,
};
use tau_kernel::log::Log;

fn cap(n: u64) -> Capability {
    Capability::mint(n)
}

fn name(s: &str) -> Name {
    Name::new(s).expect("fixture name is valid")
}

#[test]
fn abi_version_is_pinned() {
    // Bumping the ABI is legal and expected; doing it without noticing is not.
    // This assertion is the tripwire, and its failure message is the
    // instruction: bump here, and say why in an ADR.
    // 0 → 1: `Endpoint::Hook`, ADR-0008 §6.
    // 1 → 2: `Entry` and the hook wire types join `kernel/src/abi/`; no byte
    // changes, but 2 is the first number that identifies the entry format,
    // ADR-0010 §2.
    assert_eq!(
        ABI, 2,
        "ABI version changed; update this test and link the ADR that authorises it"
    );
}

#[test]
fn msg_request_wire_format() {
    let msg = Msg::new(
        Seq::new(42),
        Endpoint::Agent {
            id: AgentId::new(7),
        },
        MsgKind::Request,
        BlobRef::from_bytes([0xab; 32]),
    )
    .with_corr(Corr::new(1001));

    insta::assert_json_snapshot!(msg);
}

#[test]
fn msg_reply_with_consumption_wire_format() {
    let msg = Msg::new(
        Seq::new(43),
        Endpoint::Driver {
            id: DriverId::new(name("anthropic")),
        },
        MsgKind::Reply,
        BlobRef::from_bytes([0x01; 32]),
    )
    .with_corr(Corr::new(1001))
    .with_consumption(Consumption::from_dims([
        (DimKey::Tokens, 1_234),
        (DimKey::CostMicroUsd, 9_900),
        (DimKey::Custom(name("cache-read-tokens")), 512),
    ]));

    insta::assert_json_snapshot!(msg);
}

#[test]
fn msg_notice_from_harness_wire_format() {
    let msg = Msg::new(
        Seq::new(0),
        Endpoint::Harness,
        MsgKind::Notice,
        BlobRef::EMPTY,
    );

    insta::assert_json_snapshot!(msg);
}

#[test]
fn msg_notice_from_hook_wire_format() {
    // The one case ABI 1 adds (ADR-0008 §6): a hook's `Emit` is a notice
    // from the hook, so an agent can `recv` by sender and a reader can tell
    // a budget warning from a cancel on the envelope alone.
    let msg = Msg::new(
        Seq::new(89),
        Endpoint::Hook { id: HookId::new(3) },
        MsgKind::Notice,
        BlobRef::from_bytes([0x4a; 32]),
    );

    insta::assert_json_snapshot!(msg);
}

#[test]
fn namespace_wire_format() {
    insta::assert_json_snapshot!(Namespace::from_caps([cap(3), cap(1), cap(2)]));
}

#[test]
fn budget_wire_format() {
    insta::assert_json_snapshot!(Budget::from_dims([
        (DimKey::Tokens, 100_000),
        (DimKey::CostMicroUsd, 5_000_000),
        (DimKey::WallMs, 600_000),
        (DimKey::Calls, 256),
        (DimKey::Depth, 4),
        (DimKey::ComputeMs, 30_000),
        (DimKey::Custom(name("gpu_seconds")), 12),
    ]));
}

#[test]
fn log_header_wire_format() {
    insta::assert_json_snapshot!(LogHeader::current());
}

/// A header with every field at a recognisable value: the numbers are the
/// ADR-0011 §1 example's, the digests are bytes a reader can see are
/// distinct. Not a header any log would produce; the shape is what is pinned.
fn snapshot_header() -> SnapshotHeader {
    SnapshotHeader {
        magic: SnapshotHeader::MAGIC,
        abi: 2,
        fold: 1,
        seq: 5017,
        prefix: "11".repeat(32),
        state: "22".repeat(32),
    }
}

#[test]
fn snapshot_header_wire_format() {
    // ADR-0011 §2: the snapshot header joins the frozen surface so an old
    // snapshot can always be *refused* legibly, whatever became of the state
    // behind it.
    insta::assert_json_snapshot!(snapshot_header());
}

#[test]
fn every_frozen_type_round_trips() {
    // A snapshot pins how a value is written. This pins that reading it back
    // yields the same value — the property replay actually depends on.
    let msg = Msg::new(
        Seq::new(9),
        Endpoint::Driver {
            id: DriverId::new(name("clock")),
        },
        MsgKind::Partial,
        BlobRef::from_bytes([0x7f; 32]),
    )
    .with_corr(Corr::new(5))
    .with_consumption(Consumption::from_dims([(DimKey::ComputeMs, 3)]));

    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(serde_json::from_str::<Msg>(&json).unwrap(), msg);

    let ns = Namespace::from_caps([cap(1), cap(9)]);
    let json = serde_json::to_string(&ns).unwrap();
    assert_eq!(serde_json::from_str::<Namespace>(&json).unwrap(), ns);

    let budget = Budget::from_dims([(DimKey::Tokens, 1), (DimKey::Custom(name("x")), 2)]);
    let json = serde_json::to_string(&budget).unwrap();
    assert_eq!(serde_json::from_str::<Budget>(&json).unwrap(), budget);

    let header = LogHeader::current();
    let json = serde_json::to_string(&header).unwrap();
    assert_eq!(serde_json::from_str::<LogHeader>(&json).unwrap(), header);

    let header = snapshot_header();
    let json = serde_json::to_string(&header).unwrap();
    assert_eq!(
        serde_json::from_str::<SnapshotHeader>(&json).unwrap(),
        header
    );

    // Every entry kind, and every variant split the snapshots pin: the
    // property `tau replay` depends on (ADR-0010 §5).
    for entry in one_of_every_kind() {
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(
            serde_json::from_str::<Entry>(&json).unwrap(),
            entry,
            "round trip lost something in {json}"
        );
    }

    // And the whole file: what `write_to` writes, `read_from` reads back
    // equal, header included.
    let mut log = Log::in_memory();
    for entry in one_of_every_kind() {
        log.append(entry).unwrap();
    }
    let mut bytes = Vec::new();
    log.write_to(&mut bytes).unwrap();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(reread.header(), log.header());
    assert_eq!(reread.entries(), log.entries());
}

// ------------------------------------------------------------------- entries
//
// ADR-0010 §3: one snapshot per kind, named as in the table there, with every
// optional or variant-bearing field exercised across the set. The `seq`
// values are the table's; they do not form a valid log and are not meant to.

fn blob(byte: u8) -> BlobRef {
    BlobRef::from_bytes([byte; 32])
}

fn agent(n: u64) -> AgentId {
    AgentId::new(n)
}

fn hook(n: u64) -> HookId {
    HookId::new(n)
}

fn driver_registered() -> Entry {
    Entry::DriverRegistered {
        seq: Seq::new(0),
        driver: DriverId::new(name("tool")),
        cap: cap(0),
        ceiling: Budget::from_dims([(DimKey::Tokens, 30)]),
    }
}

fn spawned_root() -> Entry {
    Entry::Spawned {
        seq: Seq::new(1),
        parent: None,
        agent: agent(0),
        ns: Namespace::from_caps([cap(0)]),
        budget: Budget::from_dims([
            (DimKey::Tokens, 70),
            (DimKey::Calls, 10),
            (DimKey::Depth, 2),
        ]),
    }
}

fn spawned_child() -> Entry {
    Entry::Spawned {
        seq: Seq::new(2),
        parent: Some(agent(0)),
        agent: agent(1),
        ns: Namespace::empty(),
        budget: Budget::from_dims([(DimKey::Tokens, 20), (DimKey::Depth, 1)]),
    }
}

fn sent() -> Entry {
    Entry::Sent {
        msg: Msg::new(
            Seq::new(12),
            Endpoint::Agent { id: agent(0) },
            MsgKind::Request,
            blob(0xab),
        )
        .with_corr(Corr::new(0)),
        via: cap(0),
    }
}

fn replied() -> Entry {
    Entry::Replied {
        msg: Msg::new(
            Seq::new(17),
            Endpoint::Driver {
                id: DriverId::new(name("tool")),
            },
            MsgKind::Reply,
            blob(0x01),
        )
        .with_corr(Corr::new(0))
        .with_consumption(Consumption::from_dims([(DimKey::Tokens, 12)])),
        to: agent(0),
    }
}

fn resolved() -> Entry {
    Entry::Resolved {
        seq: Seq::new(5),
        agent: agent(1),
        matched: Seq::new(4),
    }
}

fn exited() -> Entry {
    Entry::Exited {
        seq: Seq::new(7),
        agent: agent(0),
        result: blob(0x22),
    }
}

fn claimed_by_parent() -> Entry {
    Entry::Claimed {
        seq: Seq::new(47),
        agent: agent(1),
        by: Some(agent(0)),
    }
}

fn claimed_by_harness() -> Entry {
    Entry::Claimed {
        seq: Seq::new(48),
        agent: agent(0),
        by: None,
    }
}

fn cancelled_by_agent() -> Entry {
    Entry::Cancelled {
        seq: Seq::new(4),
        by: Some(agent(0)),
        agent: agent(1),
        grace: 10,
        reason: blob(0x33),
    }
}

fn cancelled_by_harness() -> Entry {
    Entry::Cancelled {
        seq: Seq::new(6),
        by: None,
        agent: agent(0),
        grace: 0,
        reason: BlobRef::EMPTY,
    }
}

fn tick() -> Entry {
    Entry::Tick {
        seq: Seq::new(4),
        now: 50,
    }
}

fn attached_native() -> Entry {
    Entry::Attached {
        seq: Seq::new(1),
        hook: hook(0),
        point: HookPoint::PreSend,
        failure: FailureMode::Closed,
        program: HookSource::Native(name("tattle-shell")),
    }
}

fn attached_rule() -> Entry {
    Entry::Attached {
        seq: Seq::new(2),
        hook: hook(1),
        point: HookPoint::PreDeliver,
        failure: FailureMode::Open,
        program: HookSource::Rule("when pre_deliver then allow".to_owned()),
    }
}

fn attached_on_budget() -> Entry {
    Entry::Attached {
        seq: Seq::new(3),
        hook: hook(2),
        point: HookPoint::OnBudget {
            dim: DimKey::Tokens,
            below: 50,
        },
        failure: FailureMode::Open,
        program: HookSource::Native(name("low-tokens")),
    }
}

fn verdicts() -> Entry {
    // All four rulings in one roll, in `HookId` order. A real roll at a pre
    // point stops at the `Deny`; the shape is what is pinned, not the rule.
    Entry::Verdicts {
        seq: Seq::new(32),
        point: HookPoint::OnSpawn,
        subject: agent(1),
        roll: vec![
            (hook(0), Ruling::Allow),
            (
                hook(1),
                Ruling::Emit {
                    to: agent(0),
                    payload: blob(0x44),
                },
            ),
            (
                hook(2),
                Ruling::Failed {
                    mode: FailureMode::Closed,
                    error: blob(0x55),
                },
            ),
            (hook(3), Ruling::Deny(blob(0x66))),
        ],
    }
}

fn emitted() -> Entry {
    Entry::Emitted {
        hook: hook(5),
        to: agent(0),
        msg: Msg::new(
            Seq::new(14),
            Endpoint::Hook { id: hook(5) },
            MsgKind::Notice,
            blob(0x77),
        ),
    }
}

/// Every kind and every variant split, in the order of the ADR-0010 §3 table.
fn one_of_every_kind() -> Vec<Entry> {
    vec![
        driver_registered(),
        spawned_root(),
        spawned_child(),
        sent(),
        replied(),
        resolved(),
        exited(),
        claimed_by_parent(),
        claimed_by_harness(),
        cancelled_by_agent(),
        cancelled_by_harness(),
        tick(),
        attached_native(),
        attached_rule(),
        attached_on_budget(),
        verdicts(),
        emitted(),
    ]
}

macro_rules! entry_snapshot {
    ($($name:ident => $fixture:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                insta::assert_json_snapshot!($fixture());
            }
        )*
    };
}

entry_snapshot!(
    entry_driver_registered => driver_registered,
    entry_spawned_root => spawned_root,
    entry_spawned_child => spawned_child,
    entry_sent => sent,
    entry_replied => replied,
    entry_resolved => resolved,
    entry_exited => exited,
    entry_claimed_by_parent => claimed_by_parent,
    entry_claimed_by_harness => claimed_by_harness,
    entry_cancelled_by_agent => cancelled_by_agent,
    entry_cancelled_by_harness => cancelled_by_harness,
    entry_tick => tick,
    entry_attached_native => attached_native,
    entry_attached_rule => attached_rule,
    entry_attached_on_budget => attached_on_budget,
    entry_verdicts => verdicts,
    entry_emitted => emitted,
);

#[test]
fn log_file_wire_format() {
    // ADR-0010 §4, pinned as bytes rather than prose: a header line, then one
    // entry per line, newline-separated, nothing else. This is the file a
    // reader in another language is written from.
    let mut log = Log::in_memory();
    for entry in one_of_every_kind() {
        log.append(entry).unwrap();
    }
    let mut bytes = Vec::new();
    log.write_to(&mut bytes).unwrap();
    insta::assert_snapshot!(String::from_utf8(bytes).unwrap());
}
