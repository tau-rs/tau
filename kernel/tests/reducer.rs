//! The reducer refuses what could not have happened.
//!
//! Each test builds a small legal prefix and then offers one entry the reducer
//! must reject, checking both the reason and that the state is untouched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::abi::{
    AgentId, BlobRef, Budget, BudgetError, Capability, Consumption, Corr, DimKey, DriverId,
    Endpoint, LogHeader, Msg, MsgKind, Name, Namespace, Seq, ABI,
};
use tau_kernel::log::{Entry, Log, LogError};
use tau_kernel::reducer::{fold, Refusal, State};

fn echo() -> DriverId {
    DriverId::new(Name::new("echo").unwrap())
}

/// A driver registered and a root spawned holding its capability.
fn booted() -> (State, Capability, AgentId) {
    let cap = Capability::mint(0);
    let root = AgentId::new(0);
    let state = fold(&[
        Entry::DriverRegistered {
            seq: Seq::new(0),
            driver: echo(),
            cap,
        },
        Entry::Spawned {
            seq: Seq::new(1),
            parent: None,
            agent: root,
            ns: Namespace::from_caps([cap]),
            budget: Budget::from_dims([(DimKey::Tokens, 100)]),
        },
    ])
    .unwrap();
    (state, cap, root)
}

fn refuse(state: &State, entry: &Entry) -> Refusal {
    let before = state.clone();
    let mut after = state.clone();
    let err = after.apply(entry).unwrap_err();
    assert_eq!(after, before, "a refused entry changes nothing");
    err
}

#[test]
fn entries_must_arrive_in_order() {
    let (state, ..) = booted();
    let err = refuse(
        &state,
        &Entry::Claimed {
            seq: Seq::new(7),
            agent: AgentId::new(0),
        },
    );
    assert_eq!(
        err,
        Refusal::OutOfOrder {
            expected: Seq::new(2),
            found: Seq::new(7)
        }
    );
}

#[test]
fn a_root_cannot_hold_a_capability_the_kernel_never_minted() {
    let state = fold(&[Entry::DriverRegistered {
        seq: Seq::new(0),
        driver: echo(),
        cap: Capability::mint(0),
    }])
    .unwrap();
    let err = refuse(
        &state,
        &Entry::Spawned {
            seq: Seq::new(1),
            parent: None,
            agent: AgentId::new(0),
            ns: Namespace::from_caps([Capability::mint(9)]),
            budget: Budget::empty(),
        },
    );
    assert_eq!(err, Refusal::UnknownCapability(Capability::mint(9)));
}

#[test]
fn authority_only_narrows() {
    let (state, _, root) = booted();
    let err = refuse(
        &state,
        &Entry::Spawned {
            seq: Seq::new(2),
            parent: Some(root),
            agent: AgentId::new(1),
            ns: Namespace::from_caps([Capability::mint(0), Capability::mint(1)]),
            budget: Budget::empty(),
        },
    );
    assert_eq!(err, Refusal::NotSubset { parent: root });
}

#[test]
fn a_child_cannot_be_granted_more_than_its_parent_has() {
    let (state, cap, root) = booted();
    let err = refuse(
        &state,
        &Entry::Spawned {
            seq: Seq::new(2),
            parent: Some(root),
            agent: AgentId::new(1),
            ns: Namespace::from_caps([cap]),
            budget: Budget::from_dims([(DimKey::Tokens, 101)]),
        },
    );
    assert_eq!(
        err,
        Refusal::Budget(BudgetError::Insufficient {
            dim: DimKey::Tokens,
            available: 100,
            requested: 101
        })
    );
}

#[test]
fn a_send_needs_the_capability_in_hand() {
    let (state, _, root) = booted();
    let stranger = Capability::mint(5);
    let msg = Msg::new(
        Seq::new(2),
        Endpoint::Agent { id: root },
        MsgKind::Request,
        BlobRef::EMPTY,
    )
    .with_corr(Corr::new(0));
    let err = refuse(&state, &Entry::Sent { msg, via: stranger });
    assert_eq!(
        err,
        Refusal::NotHeld {
            agent: root,
            cap: stranger
        }
    );
}

#[test]
fn a_reply_to_an_exited_owner_is_dead_letter() {
    let (mut state, cap, root) = booted();
    let request = Msg::new(
        Seq::new(2),
        Endpoint::Agent { id: root },
        MsgKind::Request,
        BlobRef::EMPTY,
    )
    .with_corr(Corr::new(0));
    state
        .apply(&Entry::Sent {
            msg: request,
            via: cap,
        })
        .unwrap();
    state
        .apply(&Entry::Exited {
            seq: Seq::new(3),
            agent: root,
            result: BlobRef::EMPTY,
        })
        .unwrap();
    assert_eq!(
        state.owner(Corr::new(0)),
        None,
        "corrs die with their owner"
    );

    let reply = Msg::new(
        Seq::new(4),
        Endpoint::Driver { id: echo() },
        MsgKind::Reply,
        BlobRef::EMPTY,
    )
    .with_corr(Corr::new(0));
    let err = refuse(
        &state,
        &Entry::Replied {
            msg: reply,
            to: root,
        },
    );
    assert_eq!(err, Refusal::UnknownCorr(Some(Corr::new(0))));
}

#[test]
fn spending_the_grant_exhausts_the_agent() {
    let (mut state, cap, root) = booted();
    let request = |seq: u64, corr: u64| {
        Msg::new(
            Seq::new(seq),
            Endpoint::Agent { id: root },
            MsgKind::Request,
            BlobRef::EMPTY,
        )
        .with_corr(Corr::new(corr))
    };
    state
        .apply(&Entry::Sent {
            msg: request(2, 0),
            via: cap,
        })
        .unwrap();
    let reply = Msg::new(
        Seq::new(3),
        Endpoint::Driver { id: echo() },
        MsgKind::Reply,
        BlobRef::EMPTY,
    )
    .with_corr(Corr::new(0))
    .with_consumption(Consumption::from_dims([(DimKey::Tokens, 150)]));
    state
        .apply(&Entry::Replied {
            msg: reply,
            to: root,
        })
        .unwrap();

    let rec = state.agent(root).unwrap();
    assert_eq!(
        rec.budget.get(&DimKey::Tokens),
        Some(0),
        "drained, not negative"
    );
    assert_eq!(
        rec.spent.get(&DimKey::Tokens),
        Some(&150),
        "recorded in full"
    );
    assert!(rec.exhausted);

    let err = refuse(
        &state,
        &Entry::Sent {
            msg: request(4, 1),
            via: cap,
        },
    );
    assert_eq!(err, Refusal::Exhausted(root));
}

#[test]
fn a_resolution_must_name_a_message_in_the_mailbox() {
    let (state, _, root) = booted();
    let err = refuse(
        &state,
        &Entry::Resolved {
            seq: Seq::new(2),
            agent: root,
            matched: Seq::new(1),
        },
    );
    assert_eq!(
        err,
        Refusal::NotInMailbox {
            agent: root,
            seq: Seq::new(1)
        }
    );
}

#[test]
fn unspent_budget_returns_to_the_parent() {
    let (mut state, cap, root) = booted();
    state
        .apply(&Entry::Spawned {
            seq: Seq::new(2),
            parent: Some(root),
            agent: AgentId::new(1),
            ns: Namespace::from_caps([cap]),
            budget: Budget::from_dims([(DimKey::Tokens, 40)]),
        })
        .unwrap();
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(60)
    );
    state
        .apply(&Entry::Exited {
            seq: Seq::new(3),
            agent: AgentId::new(1),
            result: BlobRef::EMPTY,
        })
        .unwrap();
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100)
    );
    assert_eq!(state.result(AgentId::new(1)), Some(BlobRef::EMPTY));
}

#[test]
fn a_log_from_the_future_is_refused_at_the_header() {
    let header = LogHeader {
        magic: LogHeader::MAGIC,
        abi: ABI.saturating_add(1),
    };
    let mut bytes = serde_json::to_vec(&header).unwrap();
    bytes.push(b'\n');
    let err = Log::read_from(bytes.as_slice()).unwrap_err();
    assert!(matches!(err, LogError::Unreadable { .. }), "got {err:?}");
}

#[test]
fn an_empty_log_folds_to_the_initial_state() {
    let log = Log::in_memory();
    assert_eq!(fold(log.entries()).unwrap(), State::initial());
    assert_eq!(State::initial().hash(), State::initial().hash());
}
