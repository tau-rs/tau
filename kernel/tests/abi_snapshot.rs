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
    AgentId, BlobRef, Budget, Capability, Consumption, Corr, DimKey, DriverId, Endpoint, LogHeader,
    Msg, MsgKind, Name, Namespace, Seq, ABI,
};

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
    assert_eq!(
        ABI, 0,
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
}
