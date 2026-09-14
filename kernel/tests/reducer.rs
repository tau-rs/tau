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

/// Tokens and calls: what a child needs to send.
fn grant(tokens: u64, calls: u64) -> Budget {
    Budget::from_dims([(DimKey::Tokens, tokens), (DimKey::Calls, calls)])
}

/// Tokens, calls, and a wall grant.
fn timed(tokens: u64, calls: u64, wall: u64) -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, tokens),
        (DimKey::Calls, calls),
        (DimKey::WallMs, wall),
    ])
}

/// The root's grant in these tests: 100 tokens, 10 calls, three levels.
fn root_grant() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 100),
        (DimKey::Calls, 10),
        (DimKey::Depth, 3),
    ])
}

/// What one request to the echo driver may cost at most, as registered.
const CEILING: u64 = 10;

/// A driver registered and a root spawned holding its capability.
fn booted() -> (State, Capability, AgentId) {
    booted_with(root_grant())
}

/// A driver registered with a `CEILING` of tokens, and a root spawned with
/// `budget`, holding the driver's capability.
fn booted_with(budget: Budget) -> (State, Capability, AgentId) {
    let cap = Capability::mint(0);
    let root = agent(0);
    let state = fold(&[
        Entry::DriverRegistered {
            seq: Seq::new(0),
            driver: echo(),
            cap,
            ceiling: tokens(CEILING),
        },
        Entry::Spawned {
            seq: Seq::new(1),
            parent: None,
            agent: root,
            ns: Namespace::from_caps([cap]),
            budget,
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

/// A child of `parent` with `budget` tokens and one call.
fn spawned(state: &State, parent: AgentId, budget: u64) -> Entry {
    spawned_with(state, parent, grant(budget, 1))
}

fn spawned_with(state: &State, parent: AgentId, budget: Budget) -> Entry {
    Entry::Spawned {
        seq: state.next_seq(),
        parent: Some(parent),
        agent: state.next_agent(),
        ns: Namespace::from_caps([Capability::mint(0)]),
        budget,
    }
}

fn replied(state: &State, corr: Corr, to: AgentId, consumed: Consumption) -> Entry {
    Entry::Replied {
        msg: Msg::new(
            state.next_seq(),
            Endpoint::Driver { id: echo() },
            MsgKind::Reply,
            BlobRef::EMPTY,
        )
        .with_corr(corr)
        .with_consumption(consumed),
        to,
    }
}

fn used(tokens: u64) -> Consumption {
    Consumption::from_dims([(DimKey::Tokens, tokens)])
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
/// open request (and so `CEILING` tokens in reservation). The shape every
/// cancel test starts from.
fn family() -> (State, AgentId, AgentId, AgentId) {
    let (mut state, _, root) = booted();
    // Two calls: one to hand the grandchild, one to send with.
    step(&mut state, |s| spawned_with(s, root, grant(40, 2)));
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
        ceiling: tokens(CEILING),
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
    let order: Vec<_> = state.completed().map(|c| c.agent).collect();
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
    let from = &state.completed().next().unwrap().agent;
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

// --------------------------------------------------------------- budgets

#[test]
fn a_send_reserves_the_ceiling_and_the_reply_settles_it() {
    let (mut state, _, root) = booted();
    step(&mut state, |s| sent(s, root));
    let rec = state.agent(root).unwrap();
    assert_eq!(
        rec.budget.get(&DimKey::Tokens),
        Some(100 - CEILING),
        "the ceiling is held before delivery"
    );
    assert_eq!(rec.budget.get(&DimKey::Calls), Some(9), "one call charged");
    assert_eq!(rec.reserved.get(&Corr::new(0)), Some(&tokens(CEILING)));
    assert_eq!(rec.spent.get(&DimKey::Calls), Some(&1));
    assert_eq!(rec.spent.get(&DimKey::Tokens), None, "nothing spent yet");

    step(&mut state, |s| replied(s, Corr::new(0), root, used(3)));
    let rec = state.agent(root).unwrap();
    assert_eq!(
        rec.budget.get(&DimKey::Tokens),
        Some(97),
        "the unused part of the reservation came back"
    );
    assert!(rec.reserved.is_empty(), "settled");
    assert_eq!(rec.spent.get(&DimKey::Tokens), Some(&3));
    assert!(rec.overdraft.is_empty());
}

#[test]
fn a_send_that_cannot_reserve_the_ceiling_is_refused_before_delivery() {
    // Tokens short of the ceiling.
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned(s, root, 5));
    let child = agent(1);
    assert_eq!(
        refuse(&state, &sent(&state, child)),
        Refusal::Budget(BudgetError::Insufficient {
            dim: DimKey::Tokens,
            available: 5,
            requested: CEILING
        })
    );
    assert_eq!(state.owner(Corr::new(0)), None, "nothing was delivered");

    // Calls exhausted: the reducer charges one per send.
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned_with(s, root, grant(50, 1)));
    step(&mut state, |s| sent(s, child));
    assert_eq!(
        refuse(&state, &sent(&state, child)),
        Refusal::Budget(BudgetError::Insufficient {
            dim: DimKey::Calls,
            available: 0,
            requested: 1
        })
    );

    // No calls grant at all.
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned_with(s, root, tokens(50)));
    assert_eq!(
        refuse(&state, &sent(&state, child)),
        Refusal::Budget(BudgetError::NoGrant { dim: DimKey::Calls })
    );
}

#[test]
fn a_report_above_the_ceiling_is_charged_in_full_and_recorded_as_overdraft() {
    let (mut state, _, root) = booted();
    step(&mut state, |s| sent(s, root));
    step(&mut state, |s| replied(s, Corr::new(0), root, used(15)));
    let rec = state.agent(root).unwrap();
    assert_eq!(
        rec.budget.get(&DimKey::Tokens),
        Some(90),
        "no refund, and nothing beyond the reservation was taken"
    );
    assert_eq!(rec.spent.get(&DimKey::Tokens), Some(&15), "charged in full");
    assert_eq!(rec.overdraft.get(&DimKey::Tokens), Some(&5), "and visible");

    // A dimension that was never reserved is overdraft entirely.
    step(&mut state, |s| sent(s, root));
    step(&mut state, |s| {
        replied(
            s,
            Corr::new(1),
            root,
            Consumption::from_dims([(DimKey::Tokens, 2), (DimKey::ComputeMs, 7)]),
        )
    });
    let rec = state.agent(root).unwrap();
    assert_eq!(rec.budget.get(&DimKey::Tokens), Some(88));
    assert_eq!(rec.spent.get(&DimKey::Tokens), Some(&17));
    assert_eq!(rec.spent.get(&DimKey::ComputeMs), Some(&7));
    assert_eq!(rec.overdraft.get(&DimKey::Tokens), Some(&5));
    assert_eq!(rec.overdraft.get(&DimKey::ComputeMs), Some(&7));
}

#[test]
fn a_finished_agent_releases_its_reservations_and_an_orphan_cascades_upward() {
    // child 1 holds corr 0 with CEILING reserved, and 10 carved to grandchild.
    let (mut state, root, child, grandchild) = family();
    assert_eq!(
        state.agent(child).unwrap().budget.get(&DimKey::Tokens),
        Some(40 - 10 - CEILING)
    );
    step(&mut state, |s| exited(s, child));
    let rec = state.agent(child).unwrap();
    assert!(rec.reserved.is_empty(), "no leaked reservation");
    assert_eq!(state.owner(Corr::new(0)), None);
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(90),
        "the child's remainder, reservation included, came back"
    );

    // The grandchild is now an orphan. Its remainder goes to the nearest live
    // ancestor — the root — not to the dead child's record (#15).
    step(&mut state, |s| exited(s, grandchild));
    assert_eq!(
        state.agent(child).unwrap().budget.get(&DimKey::Tokens),
        None,
        "nothing stranded on the dead record"
    );
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100),
        "as if the grandchild had exited first"
    );
}

#[test]
fn the_roots_record_is_the_ledger_even_after_the_root_exits() {
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned(s, root, 40));
    step(&mut state, |s| exited(s, root));
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(60),
        "the root keeps its remainder"
    );
    step(&mut state, |s| exited(s, agent(1)));
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100),
        "an orphan with no live ancestor returns to the root's record"
    );
}

// ----------------------------------------------------------------- depth

#[test]
fn a_child_is_born_one_level_shallower_and_the_parent_keeps_its_own() {
    let (state, root, child, grandchild) = family();
    let depth = |id| state.agent(id).unwrap().budget.get(&DimKey::Depth);
    assert_eq!(depth(root), Some(3), "not carved");
    assert_eq!(depth(child), Some(2));
    assert_eq!(depth(grandchild), Some(1));
}

#[test]
fn a_spawn_at_depth_zero_is_refused() {
    let (mut state, _, _, grandchild) = family();
    step(&mut state, |s| spawned(s, grandchild, 1));
    let leaf = agent(3);
    assert_eq!(
        state.agent(leaf).unwrap().budget.get(&DimKey::Depth),
        Some(0)
    );
    assert_eq!(
        refuse(&state, &spawned(&state, leaf, 1)),
        Refusal::Budget(BudgetError::Insufficient {
            dim: DimKey::Depth,
            available: 0,
            requested: 1
        })
    );
}

#[test]
fn a_child_may_be_narrowed_in_depth_but_not_widened() {
    let (mut state, _, root) = booted();
    let narrow = Budget::from_dims([(DimKey::Tokens, 10), (DimKey::Depth, 0)]);
    step(&mut state, |s| spawned_with(s, root, narrow));
    let child = agent(1);
    assert_eq!(
        state.agent(child).unwrap().budget.get(&DimKey::Depth),
        Some(0),
        "asked for less than the default"
    );
    assert!(refuse(&state, &spawned(&state, child, 1))
        .to_string()
        .contains("depth"));

    let wide = Budget::from_dims([(DimKey::Tokens, 10), (DimKey::Depth, 5)]);
    assert_eq!(
        refuse(&state, &spawned_with(&state, root, wide)),
        Refusal::Budget(BudgetError::Insufficient {
            dim: DimKey::Depth,
            available: 2,
            requested: 5
        })
    );
}

#[test]
fn a_parent_without_a_depth_grant_cannot_spawn() {
    let (state, _, root) = booted_with(tokens(100));
    assert_eq!(
        refuse(&state, &spawned(&state, root, 1)),
        Refusal::Budget(BudgetError::NoGrant { dim: DimKey::Depth })
    );
}

#[test]
fn a_parents_depth_is_unchanged_after_a_child_exits() {
    let (mut state, root, child, _) = family();
    let depth = |s: &State, id| s.agent(id).unwrap().budget.get(&DimKey::Depth);
    assert_eq!(depth(&state, root), Some(3));
    step(&mut state, |s| exited(s, child));
    assert_eq!(depth(&state, root), Some(3), "depth is not handed back");
    assert_eq!(
        depth(&state, child),
        None,
        "a finished record holds nothing"
    );
}

#[test]
fn a_parents_depth_is_unchanged_after_a_child_is_aborted() {
    // Past a cancel deadline.
    let (mut state, root, child, _) = family();
    let depth = |s: &State, id| s.agent(id).unwrap().budget.get(&DimKey::Depth);
    step(&mut state, |s| cancelled(s, Some(root), child, 10));
    step(&mut state, |s| tick(s, 10));
    assert_eq!(state.agent(child).unwrap().status, Status::Aborted);
    assert_eq!(depth(&state, root), Some(3), "after a deadline abort");

    // By wall exhaustion.
    let (mut state, _, root) = booted_with(timed_root());
    step(&mut state, |s| spawned_with(s, root, timed(40, 2, 400)));
    let child = agent(1);
    step(&mut state, |s| tick(s, 400));
    assert_eq!(state.agent(child).unwrap().status, Status::Aborted);
    assert_eq!(depth(&state, root), Some(3), "after a wall abort");
}

/// The reproduction from #26: a root granted `depth: 2` spawns a child with
/// no depth request, the child exits, and the root's own depth must not
/// have grown. Four rounds once doubled it every time (2, 3, 5, 9, 17).
#[test]
fn depth_does_not_grow_with_spawn_and_exit_rounds() {
    let grant = Budget::from_dims([
        (DimKey::Tokens, 100),
        (DimKey::Calls, 10),
        (DimKey::Depth, 2),
    ]);
    let (mut state, _, root) = booted_with(grant);
    let depth = |s: &State, id| s.agent(id).unwrap().budget.get(&DimKey::Depth);
    for round in 1..=4 {
        step(&mut state, |s| spawned(s, root, 1));
        let child = agent(round);
        assert_eq!(
            depth(&state, child),
            Some(1),
            "round {round}: born at parent - 1"
        );
        step(&mut state, |s| exited(s, child));
        assert_eq!(
            depth(&state, root),
            Some(2),
            "round {round}: root unchanged"
        );
    }
}

// ------------------------------------------------------------------ wall

fn timed_root() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 100),
        (DimKey::Calls, 10),
        (DimKey::Depth, 3),
        (DimKey::WallMs, 1_000),
    ])
}

#[test]
fn the_clock_spends_wall_and_the_tick_that_empties_it_aborts() {
    let (mut state, _, root) = booted_with(timed_root());
    step(&mut state, |s| spawned_with(s, root, timed(40, 2, 400)));
    let child = agent(1);
    let wall = |s: &State, id: AgentId| s.agent(id).unwrap().budget.get(&DimKey::WallMs);
    let spent = |s: &State, id: AgentId| s.agent(id).unwrap().spent.get(&DimKey::WallMs).copied();
    assert_eq!(wall(&state, root), Some(600), "carved");
    assert_eq!(wall(&state, child), Some(400));

    step(&mut state, |s| tick(s, 100));
    assert_eq!(wall(&state, root), Some(500));
    assert_eq!(spent(&state, root), Some(100));
    assert_eq!(wall(&state, child), Some(300));
    assert_eq!(spent(&state, child), Some(100));
    assert_eq!(
        state.expiring(400),
        vec![child],
        "the tick that would empty it"
    );
    assert_eq!(state.expiring(399), vec![]);

    step(&mut state, |s| tick(s, 400));
    let rec = state.agent(child).unwrap();
    assert_eq!(rec.status, Status::Aborted);
    assert_eq!(
        rec.spent.get(&DimKey::WallMs),
        Some(&400),
        "all of it, no more"
    );
    assert_eq!(rec.budget.get(&DimKey::Tokens), None, "returned");
    assert_eq!(state.result(child), Some(Outcome::Aborted));
    assert_eq!(
        wall(&state, root),
        Some(200),
        "no wall came back: it was spent"
    );
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::Tokens),
        Some(100),
        "the tokens did"
    );

    step(&mut state, |s| tick(s, 600));
    let rec = state.agent(root).unwrap();
    assert_eq!(rec.status, Status::Aborted);
    assert_eq!(rec.budget.get(&DimKey::WallMs), Some(0));
    assert_eq!(rec.spent.get(&DimKey::WallMs), Some(&600));
    assert!(state.is_drained());
}

#[test]
fn wall_is_carved_from_the_parents_remaining_time() {
    let (mut state, _, root) = booted_with(timed_root());
    step(&mut state, |s| tick(s, 900));
    assert_eq!(
        refuse(&state, &spawned_with(&state, root, timed(10, 1, 500))),
        Refusal::Budget(BudgetError::Insufficient {
            dim: DimKey::WallMs,
            available: 100,
            requested: 500
        }),
        "a child cannot be granted time its parent no longer has"
    );
}

#[test]
fn a_timed_parent_may_not_spawn_an_untimed_child() {
    let (state, _, root) = booted_with(timed_root());
    assert_eq!(
        refuse(&state, &spawned(&state, root, 10)),
        Refusal::Unbounded {
            parent: root,
            dim: DimKey::WallMs
        },
        "a limit cannot be escaped by spawning"
    );
}

#[test]
fn an_untimed_agent_is_measured_but_never_charged() {
    let (mut state, _, root) = booted();
    step(&mut state, |s| spawned(s, root, 10));
    step(&mut state, |s| tick(s, 500));
    step(&mut state, |s| tick(s, 700));
    for id in [root, agent(1)] {
        let rec = state.agent(id).unwrap();
        assert_eq!(rec.status, Status::Live, "{id}");
        assert_eq!(rec.budget.get(&DimKey::WallMs), None, "{id}");
        assert_eq!(
            rec.spent.get(&DimKey::WallMs),
            Some(&700),
            "{id}: the receipt"
        );
    }
    assert_eq!(state.expiring(u64::MAX), vec![]);
}

// -------------------------------------------------------------- property

/// Tier 2 property #1, at one point in a fold: over the whole tree, budgets
/// plus reservations plus spent equal the root's grant plus overdraft, along
/// every granted dimension but `depth` — which is a shape limit every child
/// inherits, not a resource that is handed down.
fn conserved(state: &State, grant: &Budget) {
    let mut sum: std::collections::BTreeMap<DimKey, u64> = Default::default();
    let mut overdraft: std::collections::BTreeMap<DimKey, u64> = Default::default();
    let add = |into: &mut std::collections::BTreeMap<DimKey, u64>, dim: &DimKey, v: u64| {
        *into.entry(dim.clone()).or_insert(0) += v;
    };
    for (_, a) in state.agents() {
        for (dim, v) in a.budget.iter() {
            add(&mut sum, dim, v);
        }
        for held in a.reserved.values() {
            for (dim, v) in held.iter() {
                add(&mut sum, dim, v);
            }
        }
        for (dim, v) in &a.spent {
            add(&mut sum, dim, *v);
        }
        for (dim, v) in &a.overdraft {
            add(&mut overdraft, dim, *v);
        }
    }
    for (dim, want) in grant.iter().filter(|(d, _)| **d != DimKey::Depth) {
        let over = overdraft.get(dim).copied().unwrap_or(0);
        assert_eq!(
            sum.get(dim).copied().unwrap_or(0),
            want + over,
            "`{dim}` after entry {}",
            state.len()
        );
    }
}

#[test]
fn budgets_reservations_and_spent_sum_to_the_root_grant_at_every_step() {
    let grant = timed_root();
    let (mut state, _, root) = booted_with(grant.clone());
    conserved(&state, &grant);
    let run = |state: &mut State, build: &dyn Fn(&State) -> Entry| {
        step(state, build);
        conserved(state, &grant);
    };
    let (a, g, b) = (agent(1), agent(2), agent(3));
    run(&mut state, &|s| spawned_with(s, root, timed(40, 4, 400)));
    run(&mut state, &|s| spawned_with(s, a, timed(10, 1, 100)));
    run(&mut state, &|s| sent(s, a)); // corr 0, CEILING held
    run(&mut state, &|s| tick(s, 50)); // wall charged to all three
    run(&mut state, &|s| replied(s, Corr::new(0), a, used(3)));
    run(&mut state, &|s| sent(s, g)); // corr 1
    run(&mut state, &|s| exited(s, a)); // g is now an orphan
    run(&mut state, &|s| replied(s, Corr::new(1), g, used(12))); // misreport: 2 over
    run(&mut state, &|s| spawned_with(s, root, timed(5, 1, 10)));
    run(&mut state, &|s| cancelled(s, Some(root), b, 20));
    run(&mut state, &|s| tick(s, 70)); // b: deadline and wall, both reached
    run(&mut state, &|s| exited(s, g)); // cascades past dead `a` to root
    run(&mut state, &|s| tick(s, 100));
    run(&mut state, &|s| claimed(s, b, Some(root)));
    run(&mut state, &|s| tick(s, 5_000)); // root's own wall runs out
    assert!(state.is_drained());
    assert_eq!(
        state.agent(g).unwrap().overdraft.get(&DimKey::Tokens),
        Some(&2)
    );
    assert_eq!(state.agent(a).unwrap().budget.get(&DimKey::Tokens), None);
    assert_eq!(state.agent(b).unwrap().status, Status::Aborted);
}

// ---------------------------------------------------------------- indexes

/// Every view the reducer keeps an index for, recomputed by sweeping the
/// records: the oracle the indexes must agree with after every entry.
fn views_agree_with_a_full_sweep(state: &State) {
    let is_live = |s: Status| matches!(s, Status::Live | Status::Cancelling);
    let ids: Vec<AgentId> = state.agents().map(|(id, _)| id).collect();
    let live: Vec<AgentId> = state
        .agents()
        .filter(|(_, a)| is_live(a.status))
        .map(|(id, _)| id)
        .collect();
    let at = state.len();

    assert_eq!(
        state.live_count(),
        live.len(),
        "live_count after entry {at}"
    );
    assert_eq!(
        state.is_drained(),
        !ids.is_empty() && live.is_empty(),
        "is_drained after entry {at}"
    );
    for id in &ids {
        let children_live = state
            .agents()
            .any(|(_, a)| a.parent == Some(*id) && is_live(a.status));
        assert_eq!(
            state.has_live_children(*id),
            children_live,
            "has_live_children({id}) after entry {at}"
        );
        let below: Vec<AgentId> = ids
            .iter()
            .copied()
            .filter(|x| x == id || state.is_descendant(*x, *id))
            .collect();
        assert_eq!(state.subtree(*id), below, "subtree({id}) after entry {at}");
        assert_eq!(
            state.result(*id),
            state
                .completed()
                .find(|c| c.agent == *id)
                .map(|c| c.outcome),
            "result({id}) after entry {at}"
        );
        assert_eq!(
            state.next_completed_child(*id),
            state.completed().find(|c| c.parent == Some(*id)),
            "next_completed_child({id}) after entry {at}"
        );
    }
    for now in [state.now(), state.now() + 10, state.now() + 100, u64::MAX] {
        let elapsed = now - state.now();
        let ending: Vec<AgentId> = ids
            .iter()
            .rev()
            .copied()
            .filter(|id| {
                let a = state.agent(*id).unwrap();
                is_live(a.status)
                    && ((a.status == Status::Cancelling && a.deadline.is_some_and(|d| d <= now))
                        || a.budget.get(&DimKey::WallMs).is_some_and(|w| w <= elapsed))
            })
            .collect();
        assert_eq!(
            state.expiring(now),
            ending,
            "expiring({now}) after entry {at}"
        );
    }
}

#[test]
fn the_indexed_views_agree_with_a_full_sweep_at_every_step() {
    let (mut state, _, root) = booted_with(timed_root());
    views_agree_with_a_full_sweep(&state);
    let run = |state: &mut State, build: &dyn Fn(&State) -> Entry| {
        step(state, build);
        views_agree_with_a_full_sweep(state);
    };
    // The same script as the conservation test: every way an agent can
    // finish — exit, deadline abort, wall abort, orphan cascade — plus claims
    // in an order that is not spawn order.
    let (a, g, b) = (agent(1), agent(2), agent(3));
    run(&mut state, &|s| spawned_with(s, root, timed(40, 4, 400)));
    run(&mut state, &|s| spawned_with(s, a, timed(10, 1, 100)));
    run(&mut state, &|s| sent(s, a));
    run(&mut state, &|s| tick(s, 50));
    run(&mut state, &|s| replied(s, Corr::new(0), a, used(3)));
    run(&mut state, &|s| sent(s, g));
    run(&mut state, &|s| exited(s, a));
    run(&mut state, &|s| replied(s, Corr::new(1), g, used(12)));
    run(&mut state, &|s| spawned_with(s, root, timed(5, 1, 10)));
    run(&mut state, &|s| cancelled(s, Some(root), b, 20));
    run(&mut state, &|s| tick(s, 70));
    run(&mut state, &|s| exited(s, g));
    run(&mut state, &|s| tick(s, 100));
    run(&mut state, &|s| claimed(s, b, Some(root)));
    run(&mut state, &|s| claimed(s, a, None));
    run(&mut state, &|s| tick(s, 5_000));
    run(&mut state, &|s| claimed(s, g, None));
    run(&mut state, &|s| claimed(s, root, None));
    assert!(state.is_drained());
    assert_eq!(state.completed().len(), 0);
}

#[test]
fn a_subtree_walks_through_a_dead_middle_node() {
    let (mut state, root, child, grandchild) = family();
    step(&mut state, |s| exited(s, child));

    // The record stays, so the dead child is still in every subtree it was
    // in, and its live orphan is still reachable below it.
    assert_eq!(state.subtree(root), vec![root, child, grandchild]);
    assert_eq!(state.subtree(child), vec![child, grandchild]);
    assert_eq!(state.subtree(grandchild), vec![grandchild]);
    assert!(state.has_live_children(child), "the orphan is still live");
    assert!(state.is_descendant(grandchild, root));

    // A harness cancel of the root reaches the orphan through the dead child.
    step(&mut state, |s| cancelled(s, None, root, 0));
    assert_eq!(state.agent(child).unwrap().status, Status::Exited);
    assert_eq!(state.agent(grandchild).unwrap().status, Status::Aborted);
    assert_eq!(state.agent(root).unwrap().status, Status::Aborted);
    assert!(!state.has_live_children(child));
    assert!(state.is_drained());
    views_agree_with_a_full_sweep(&state);
}
