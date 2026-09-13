//! The reducer refuses what could not have happened.
//!
//! Each test builds a small legal prefix and then offers one entry the reducer
//! must reject, checking both the reason and that the state is untouched. The
//! cancel tests also fold the legal path — freeze, tick, abort — because the
//! abort is not an entry of its own and can only be observed as the
//! consequence of a tick.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tau_kernel::abi::{
    AgentId, BlobRef, Budget, BudgetError, Capability, Consumption, Corr, DimKey, DriverId,
    Endpoint, LogHeader, Msg, MsgKind, Name, Namespace, Seq, ABI,
};
use tau_kernel::log::{Entry, Log, LogError};
use tau_kernel::reducer::{fold, Outcome, Refusal, State, Status};

fn echo() -> DriverId {
    DriverId::new(Name::new("echo").unwrap())
}

fn agent(n: u64) -> AgentId {
    AgentId::new(n)
}

fn tokens(n: u64) -> Budget {
    Budget::from_dims([(DimKey::Tokens, n)])
}

/// A driver registered and a root spawned holding its capability.
fn booted() -> (State, Capability, AgentId) {
    let cap = Capability::mint(0);
    let root = agent(0);
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
            budget: tokens(100),
        },
    ])
    .unwrap();
    (state, cap, root)
}

/// Builds an entry against the current state and applies it. Panics if
/// refused.
fn step(state: &mut State, build: impl FnOnce(&State) -> Entry) {
    let entry = build(state);
    state.apply(&entry).unwrap();
}

fn spawned(state: &State, parent: AgentId, budget: u64) -> Entry {
    Entry::Spawned {
        seq: state.next_seq(),
        parent: Some(parent),
        agent: state.next_agent(),
        ns: Namespace::from_caps([Capability::mint(0)]),
        budget: tokens(budget),
    }
}

fn sent(state: &State, from: AgentId) -> Entry {
    Entry::Sent {
        msg: Msg::new(
            state.next_seq(),
            Endpoint::Agent { id: from },
            MsgKind::Request,
            BlobRef::EMPTY,
        )
        .with_corr(state.next_corr()),
        via: Capability::mint(0),
    }
}

fn cancelled(state: &State, by: Option<AgentId>, target: AgentId, grace: u64) -> Entry {
    Entry::Cancelled {
        seq: state.next_seq(),
        by,
        agent: target,
        grace,
        reason: BlobRef::EMPTY,
    }
}

fn tick(state: &State, now: u64) -> Entry {
    Entry::Tick {
        seq: state.next_seq(),
        now,
    }
}

fn exited(state: &State, who: AgentId) -> Entry {
    Entry::Exited {
        seq: state.next_seq(),
        agent: who,
        result: BlobRef::EMPTY,
    }
}

fn claimed(state: &State, who: AgentId, by: Option<AgentId>) -> Entry {
    Entry::Claimed {
        seq: state.next_seq(),
        agent: who,
        by,
    }
}

fn refuse(state: &State, entry: &Entry) -> Refusal {
    let before = state.clone();
    let mut after = state.clone();
    let err = after.apply(entry).unwrap_err();
    assert_eq!(after, before, "a refused entry changes nothing");
    err
}

/// root(100) → child 1 (40) → grandchild 2 (10), with child 1 holding one
/// open request. The shape every cancel test starts from.
fn family() -> (State, AgentId, AgentId, AgentId) {
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned(s, root, 40));
    let child = agent(1);
    step(&mut state, |s| spawned(s, child, 10));
    let grandchild = agent(2);
    step(&mut state, |s| sent(s, child));
    assert_eq!(state.owner(Corr::new(0)), Some(child));
    (state, root, child, grandchild)
}

#[test]
fn entries_must_arrive_in_order() {
    let (state, ..) = booted();
    let err = refuse(
        &state,
        &Entry::Claimed {
            seq: Seq::new(7),
            agent: agent(0),
            by: None,
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
            agent: agent(0),
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
            agent: agent(1),
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
            agent: agent(1),
            ns: Namespace::from_caps([cap]),
            budget: tokens(101),
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
    let (mut state, _, root) = booted();
    step(&mut state, |s| sent(s, root));
    step(&mut state, |s| exited(s, root));
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
    step(&mut state, |s| sent(s, root));
    let reply = Msg::new(
        Seq::new(3),
        Endpoint::Driver { id: echo() },
        MsgKind::Reply,
        BlobRef::EMPTY,
    )
    .with_corr(Corr::new(0))
    .with_consumption(Consumption::from_dims([(DimKey::Tokens, 150)]));
    step(&mut state, |_| Entry::Replied {
        msg: reply,
        to: root,
    });

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

    let err = refuse(&state, &sent(&state, root));
    let _ = cap;
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
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned(s, root, 40));
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(60)
    );
    step(&mut state, |s| exited(s, agent(1)));
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100)
    );
    assert_eq!(
        state.result(agent(1)),
        Some(Outcome::Exited(BlobRef::EMPTY))
    );
}

// ------------------------------------------------------------------ wait

#[test]
fn only_the_parent_or_the_harness_may_claim() {
    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| exited(s, grandchild));

    let err = refuse(&state, &claimed(&state, grandchild, Some(root)));
    assert_eq!(
        err,
        Refusal::NotChild {
            agent: root,
            child: grandchild
        }
    );

    // The parent may; so may the harness, for what the tree leaves behind.
    let mut by_parent = state.clone();
    step(&mut by_parent, |s| claimed(s, grandchild, Some(child)));
    assert_eq!(by_parent.result(grandchild), None);
    step(&mut state, |s| claimed(s, grandchild, None));
    assert_eq!(state.result(grandchild), None);

    let err = refuse(&state, &claimed(&state, grandchild, None));
    assert_eq!(err, Refusal::NoResult(grandchild), "a claim happens once");
}

#[test]
fn wait_any_sees_children_in_completion_order() {
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned(s, root, 10));
    step(&mut state, |s| spawned(s, root, 10));
    let (first, second) = (agent(1), agent(2));

    step(&mut state, |s| exited(s, second));
    step(&mut state, |s| exited(s, first));

    assert_eq!(state.next_completed_child(root).unwrap().agent, second);
    step(&mut state, |s| claimed(s, second, Some(root)));
    assert_eq!(state.next_completed_child(root).unwrap().agent, first);
    step(&mut state, |s| claimed(s, first, Some(root)));
    assert_eq!(state.next_completed_child(root), None);
    assert!(!state.has_live_children(root));
}

// ---------------------------------------------------------------- cancel

#[test]
fn a_cancel_stays_inside_the_cancellers_subtree() {
    let (mut state, root, child, _) = family();
    step(&mut state, |s| spawned(s, root, 10));
    let sibling = agent(3);

    let err = refuse(&state, &cancelled(&state, Some(child), sibling, 0));
    assert_eq!(
        err,
        Refusal::NotDescendant {
            agent: child,
            target: sibling
        }
    );
    let err = refuse(&state, &cancelled(&state, Some(child), child, 0));
    assert_eq!(
        err,
        Refusal::NotDescendant {
            agent: child,
            target: child
        },
        "an agent cannot cancel itself; that is exit"
    );
    let err = refuse(&state, &cancelled(&state, Some(child), root, 0));
    assert_eq!(
        err,
        Refusal::NotDescendant {
            agent: child,
            target: root
        }
    );
}

#[test]
fn a_cancel_cannot_be_renegotiated() {
    let (mut state, root, child, _) = family();
    step(&mut state, |s| cancelled(s, Some(root), child, 10));
    let err = refuse(&state, &cancelled(&state, Some(root), child, 1));
    assert_eq!(err, Refusal::AlreadyCancelling(child));
}

#[test]
fn a_frozen_agent_may_not_start_new_work() {
    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| cancelled(s, Some(root), child, 10));

    assert_eq!(refuse(&state, &sent(&state, child)), Refusal::Frozen(child));
    assert_eq!(
        refuse(&state, &spawned(&state, child, 1)),
        Refusal::Frozen(child)
    );
    assert_eq!(
        refuse(&state, &cancelled(&state, Some(child), grandchild, 0)),
        Refusal::Frozen(child)
    );
}

#[test]
fn a_cancel_freezes_the_whole_subtree_atomically_and_notifies_it() {
    let (mut state, root, child, grandchild) = family();
    let at = state.next_seq();
    step(&mut state, |s| cancelled(s, Some(root), child, 10));

    for id in [child, grandchild] {
        let rec = state.agent(id).unwrap();
        assert_eq!(rec.status, Status::Cancelling, "{id}");
        assert_eq!(rec.deadline, Some(10), "{id}");
        let notice = rec.mailbox.last().unwrap();
        assert_eq!(notice.kind, MsgKind::Notice);
        assert_eq!(notice.from, Endpoint::Agent { id: root });
        assert_eq!(notice.seq, at, "the notice is the freeze entry");
    }
    assert_eq!(state.agent(root).unwrap().status, Status::Live);
    assert_eq!(
        state.owner(Corr::new(0)),
        Some(child),
        "requests stay open through the grace period"
    );
}

#[test]
fn a_tick_past_the_deadline_aborts_the_subtree_and_conserves_budget() {
    let (mut state, root, child, grandchild) = family();
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(60)
    );
    step(&mut state, |s| cancelled(s, Some(root), child, 10));

    step(&mut state, |s| tick(s, 9));
    assert_eq!(state.agent(child).unwrap().status, Status::Cancelling);
    assert_eq!(state.expiring(10), vec![grandchild, child], "deepest first");

    step(&mut state, |s| tick(s, 10));
    for id in [child, grandchild] {
        let rec = state.agent(id).unwrap();
        assert_eq!(rec.status, Status::Aborted, "{id}");
        assert!(rec.mailbox.is_empty(), "{id}");
        assert_eq!(rec.budget.get(&DimKey::Tokens), None, "{id} returned it");
        assert_eq!(rec.deadline, None, "{id}");
    }
    assert_eq!(state.owner(Corr::new(0)), None, "no correlation left open");
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100),
        "everything carved came back"
    );
    let order: Vec<_> = state.completed().iter().map(|c| c.agent).collect();
    assert_eq!(order, vec![grandchild, child]);
    assert_eq!(state.result(child), Some(Outcome::Aborted));
    assert!(!state.is_drained(), "the root is still live");

    // The parent claims its child's abort like any other outcome.
    step(&mut state, |s| claimed(s, child, Some(root)));
    assert_eq!(state.result(child), None);
}

#[test]
fn a_grace_of_zero_aborts_at_once() {
    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| cancelled(s, Some(root), child, 0));
    assert_eq!(state.agent(child).unwrap().status, Status::Aborted);
    assert_eq!(state.agent(grandchild).unwrap().status, Status::Aborted);
    assert_eq!(state.owner(Corr::new(0)), None);
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100)
    );
}

#[test]
fn an_exit_during_grace_is_an_ordinary_exit() {
    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| cancelled(s, Some(root), child, 10));
    step(&mut state, |s| exited(s, grandchild));
    step(&mut state, |s| exited(s, child));

    assert_eq!(state.agent(child).unwrap().status, Status::Exited);
    assert_eq!(state.result(child), Some(Outcome::Exited(BlobRef::EMPTY)));
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100)
    );

    step(&mut state, |s| tick(s, 10));
    assert_eq!(
        state.completed().len(),
        2,
        "the deadline finds nothing to do"
    );
}

#[test]
fn an_outer_cancel_brings_a_deadline_earlier_never_later() {
    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| cancelled(s, Some(child), grandchild, 50));
    assert_eq!(state.agent(grandchild).unwrap().deadline, Some(50));
    step(&mut state, |s| cancelled(s, Some(root), child, 10));
    assert_eq!(state.agent(grandchild).unwrap().deadline, Some(10));

    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| cancelled(s, Some(child), grandchild, 10));
    step(&mut state, |s| cancelled(s, Some(root), child, 50));
    assert_eq!(
        state.agent(grandchild).unwrap().deadline,
        Some(10),
        "the inner deadline stands"
    );
    assert_eq!(state.agent(child).unwrap().deadline, Some(50));
}

#[test]
fn the_harness_may_cancel_anyone() {
    let (mut state, root, _, _) = family();
    step(&mut state, |s| cancelled(s, None, root, 0));
    assert!(state.is_drained());
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100),
        "the root's remainder stays on its record"
    );
    let from = &state.completed().first().unwrap().agent;
    assert_eq!(*from, agent(2), "deepest first");
}

#[test]
fn a_finished_agent_cannot_be_cancelled() {
    let (mut state, root, child, _) = family();
    step(&mut state, |s| cancelled(s, Some(root), child, 0));
    let err = refuse(&state, &cancelled(&state, Some(root), child, 0));
    assert_eq!(err, Refusal::AgentExited(child));
}

// ----------------------------------------------------------------- clock

#[test]
fn the_clock_does_not_rewind() {
    let (mut state, ..) = booted();
    step(&mut state, |s| tick(s, 5));
    assert_eq!(state.now(), 5);
    step(&mut state, |s| tick(s, 5));
    let err = refuse(&state, &tick(&state, 4));
    assert_eq!(err, Refusal::ClockRewound { now: 5, found: 4 });
}

// -------------------------------------------------------------------- log

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
