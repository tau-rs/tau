//! ADR-0015: wall-exhaustion grace as a policy of the clock source.
//!
//! The clock knows the next reading before it publishes it. With a grace `g`
//! set on it, before each tick it cancels — through the harness's ordinary
//! `cancel` — every live, timed agent whose remaining wall would be at most
//! `g` after that tick, with a grace equal to the agent's remaining wall as
//! of the last reading. The deadline is therefore exactly the reading that
//! would have aborted the agent anyway: the notice moves earlier, the
//! deadline never moves. A child that acts on the notice exits with what it
//! has and its parent's `wait` sees `Exited`; one that does not is aborted on
//! the same tick as without the policy. A parent that enters its window takes
//! its subtree into the freeze with it (§4). A grace of zero is no policy at
//! all and reproduces the hard path `m1b-wall.log` pins.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{
    AgentId, Budget, Consumption, Corr, DimKey, DriverId, Endpoint, Msg, MsgKind, Name, Namespace,
};
use tau_kernel::driver::clock::{Grace, VirtualClock};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{fold, Outcome, State, Status};
use tau_kernel::syscall::{program, Match, WaitFor};
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn contents(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

/// A driver that never answers.
#[derive(Clone, Default)]
struct BlackHole;

impl Driver for BlackHole {
    fn handle(&self, _request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        Box::pin(std::future::pending())
    }
}

const FIXTURE: &str = "tests/fixtures/wall-grace.log";
const CEILING: u64 = 5;
const ROOT_WALL: u64 = 1_000;
const CHILD_WALL: u64 = 100;
const PERIOD: u64 = 10;
const GRACE: u64 = 30;
const HALF_CHILD: u64 = 50;
/// `CHILD_WALL / PERIOD`.
const TICKS_TO_EXHAUSTION: usize = 10;
const REASON: &[u8] = b"wall";

fn hole_id() -> DriverId {
    DriverId::new(Name::new("blackhole").unwrap())
}

fn root_grant() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 100),
        (DimKey::Calls, 2),
        (DimKey::Depth, 1),
        (DimKey::WallMs, ROOT_WALL),
    ])
}

fn child_grant(wall: u64) -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 10),
        (DimKey::Calls, 1),
        (DimKey::WallMs, wall),
    ])
}

/// Budgets plus reservations plus spent, over every record, per dimension.
fn tree_total(state: &State) -> BTreeMap<DimKey, u64> {
    let mut sum: BTreeMap<DimKey, u64> = BTreeMap::new();
    let mut add = |dim: &DimKey, v: u64| *sum.entry(dim.clone()).or_insert(0) += v;
    for (_, a) in state.agents() {
        for (dim, v) in a.budget.iter() {
            add(dim, v);
        }
        for held in a.reserved.values() {
            for (dim, v) in held.iter() {
                add(dim, v);
            }
        }
        for (dim, v) in &a.spent {
            add(dim, *v);
        }
    }
    sum
}

fn ticks(entries: &[Entry]) -> usize {
    entries
        .iter()
        .filter(|e| matches!(e, Entry::Tick { .. }))
        .count()
}

fn cancels(entries: &[Entry]) -> Vec<(AgentId, u64)> {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::Cancelled {
                by: None,
                agent,
                grace,
                ..
            } => Some((*agent, *grace)),
            _ => None,
        })
        .collect()
}

/// The log refolds to the live state, through the bytes on disk too.
fn assert_refolds(kernel: &Kernel, sink: &SharedBuf) -> Vec<u8> {
    let entries = kernel.entries();
    let state = kernel.state();
    let live = kernel.state_hash();
    let once = fold(&entries).unwrap();
    assert_eq!(once, state);
    assert_eq!(once.hash(), live);
    let bytes = sink.contents();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(reread.entries(), entries.as_slice());
    assert_eq!(fold(reread.entries()).unwrap().hash(), live);
    bytes
}

/// ADR-0015 §6, first two bullets: one root, two children with the same,
/// smaller grant. Child A acts on the notice and exits inside its window;
/// child B never does anything and is aborted at exhaustion, on the same tick
/// it would have been without the policy.
#[tokio::test]
async fn a_child_is_warned_one_tick_before_its_last_grace_units_and_may_exit() {
    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot(log, tokio_spawner);
    let clock =
        VirtualClock::new(Arc::clone(&kernel)).with_grace(Grace::flat(GRACE).reason(REASON));

    let cap = kernel
        .register_driver(
            hole_id(),
            BlackHole,
            Budget::from_dims([(DimKey::Tokens, CEILING)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([cap]);

    let sent = Arc::new(Notify::new());
    let a_done = Arc::new(Notify::new());
    let notice: Arc<Mutex<Option<Msg>>> = Arc::default();
    let outcomes: Arc<Mutex<Vec<(AgentId, Outcome)>>> = Arc::default();

    let a_ns = ns.clone();
    let b_ns = ns.clone();
    let sent_c = Arc::clone(&sent);
    let a_done_c = Arc::clone(&a_done);
    let notice_c = Arc::clone(&notice);
    let outcomes_c = Arc::clone(&outcomes);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                // A: sends, waits for either the reply or a notice, and on
                // the notice exits with what it has.
                let a = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(cap, b"never answered").unwrap();
                            sent_c.notify_one();
                            let msg = child
                                .recv(Match::Or(vec![
                                    Match::Corr(corr),
                                    Match::Kind(MsgKind::Notice),
                                ]))
                                .await
                                .unwrap();
                            *notice_c.lock().unwrap() = Some(msg);
                            child.exit(b"partial")
                        }),
                        a_ns,
                        child_grant(CHILD_WALL),
                    )
                    .unwrap();
                // B: never acts. The notice sits in its mailbox unread.
                let b = root
                    .spawn(
                        program(|_child| std::future::pending()),
                        b_ns,
                        child_grant(CHILD_WALL),
                    )
                    .unwrap();
                let done = root.wait(WaitFor::Child(a)).await.unwrap();
                outcomes_c.lock().unwrap().push((done.agent, done.outcome));
                a_done_c.notify_one();
                let done = root.wait(WaitFor::Child(b)).await.unwrap();
                outcomes_c.lock().unwrap().push((done.agent, done.outcome));
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();

    sent.notified().await;
    let state = kernel.state();
    let mut children: Vec<AgentId> = state
        .agents()
        .filter_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .collect();
    children.sort_unstable();
    let [a, b] = children[..] else {
        panic!("two children, got {children:?}");
    };
    assert_eq!(state.owner(Corr::new(0)), Some(a));

    // --- up to the tick before the window: nothing but ticks
    let mut now = 0;
    while now + PERIOD < CHILD_WALL - GRACE {
        now = clock.advance(PERIOD).unwrap();
    }
    assert_eq!(
        now,
        CHILD_WALL - GRACE - PERIOD,
        "one tick before the window"
    );
    let state = kernel.state();
    for id in [a, b] {
        let rec = state.agent(id).unwrap();
        assert_eq!(rec.status, Status::Live);
        assert_eq!(rec.deadline, None);
        assert_eq!(rec.budget.get(&DimKey::WallMs), Some(GRACE + PERIOD));
    }
    assert!(cancels(&kernel.entries()).is_empty());

    // --- the tick that opens the window: both children are cancelled
    //     before it is published, each with its remaining wall as grace, so
    //     each deadline is exactly its exhaustion reading.
    let remaining = GRACE + PERIOD;
    let opened = clock.advance(PERIOD).unwrap();
    assert_eq!(opened, CHILD_WALL - GRACE);
    let state = kernel.state();
    for id in [a, b] {
        let rec = state.agent(id).unwrap();
        assert_eq!(rec.status, Status::Cancelling);
        assert_eq!(rec.deadline, Some(CHILD_WALL), "deadline = exhaustion");
        assert_eq!(rec.budget.get(&DimKey::WallMs), Some(GRACE));
    }
    assert_eq!(
        state.agent(root).unwrap().status,
        Status::Live,
        "the root is not near its own"
    );
    let entries = kernel.entries();
    assert_eq!(cancels(&entries), vec![(a, remaining), (b, remaining)]);
    let tail = entries.get(entries.len().saturating_sub(3)..).unwrap();
    assert!(
        matches!(
            tail,
            [Entry::Cancelled { agent: x, .. }, Entry::Cancelled { agent: y, .. }, Entry::Tick { now, .. }]
                if *x == a && *y == b && *now == opened
        ),
        "the cancels precede the tick that opens the window: {tail:?}"
    );

    // --- A sees the notice, from the harness, and exits inside the window
    a_done.notified().await;
    let msg = notice.lock().unwrap().take().unwrap();
    assert_eq!(msg.kind, MsgKind::Notice);
    assert_eq!(msg.from, Endpoint::Harness);
    assert_eq!(msg.corr, None);
    assert_eq!(kernel.read(msg.payload).unwrap(), REASON);
    assert_eq!(
        outcomes.lock().unwrap().as_slice(),
        &[(a, Outcome::Exited(tau_kernel::blob::digest(b"partial")))]
    );
    let state = kernel.state();
    assert_eq!(state.agent(a).unwrap().status, Status::Exited);
    assert_eq!(
        state.owner(Corr::new(0)),
        None,
        "the abandoned request is closed"
    );

    // --- B does nothing; the tick that would have aborted it today does
    let mut now = opened;
    while now + PERIOD < CHILD_WALL {
        now = clock.advance(PERIOD).unwrap();
        assert_eq!(kernel.state().agent(b).unwrap().status, Status::Cancelling);
    }
    assert_eq!(clock.advance(PERIOD).unwrap(), CHILD_WALL);
    assert_eq!(kernel.state().agent(b).unwrap().status, Status::Aborted);

    kernel.drained().await.unwrap();
    kernel.shutdown();

    // --- the books
    assert_eq!(
        outcomes.lock().unwrap().as_slice(),
        &[
            (a, Outcome::Exited(tau_kernel::blob::digest(b"partial"))),
            (b, Outcome::Aborted)
        ]
    );
    let state = kernel.state();
    let root_rec = state.agent(root).unwrap();
    let a_rec = state.agent(a).unwrap();
    let b_rec = state.agent(b).unwrap();
    assert_eq!(root_rec.status, Status::Exited);
    assert_eq!(
        a_rec.spent.get(&DimKey::WallMs),
        Some(&opened),
        "A paid for the time it used"
    );
    assert_eq!(
        b_rec.spent.get(&DimKey::WallMs),
        Some(&CHILD_WALL),
        "B paid for all of it"
    );
    assert!(
        a_rec.reserved.is_empty() && b_rec.reserved.is_empty(),
        "no leaked reservation"
    );
    assert!(state.is_drained());
    let total = tree_total(&state);
    for (dim, want) in root_grant().iter().filter(|(d, _)| **d != DimKey::Depth) {
        assert_eq!(total.get(dim), Some(&want), "`{dim}` conserved");
    }
    assert_eq!(
        kernel.claim(root).unwrap(),
        Outcome::Exited(tau_kernel::abi::BlobRef::EMPTY)
    );

    let entries = kernel.entries();
    assert_eq!(ticks(&entries), TICKS_TO_EXHAUSTION);
    assert_eq!(
        cancels(&entries).len(),
        2,
        "one cancel per child, none for the root"
    );

    let bytes = assert_refolds(&kernel, &sink);
    if std::env::var_os("TAU_UPDATE_FIXTURES").is_some() {
        std::fs::write(FIXTURE, &bytes).unwrap();
    }
}

#[test]
fn the_fixture_refolds_to_the_pinned_state_hash() {
    let bytes = include_bytes!("fixtures/wall-grace.log");
    let log = Log::read_from(&bytes[..]).unwrap();
    let state = fold(log.entries()).unwrap();
    assert!(state.is_drained());
    assert_eq!(state.now(), CHILD_WALL);
    insta::assert_snapshot!(state.hash().to_string());
}

/// ADR-0015 §4: the parent's own wall runs out first. Its cancel freezes the
/// child with it, at the parent's deadline, and the tick that exhausts the
/// parent aborts both — where today the child would have run on as an orphan.
#[tokio::test]
async fn a_parent_entering_its_window_freezes_its_subtree_with_it() {
    const PARENT_WALL: u64 = 200;
    const CHILD: u64 = 150;
    const PARENT_OWN: u64 = PARENT_WALL - CHILD;

    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot(log, tokio_spawner);
    let clock = VirtualClock::new(Arc::clone(&kernel)).with_grace(Grace::flat(GRACE));

    let cap = kernel
        .register_driver(
            hole_id(),
            BlackHole,
            Budget::from_dims([(DimKey::Tokens, CEILING)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([cap]);
    let spawned = Arc::new(Notify::new());
    let spawned_c = Arc::clone(&spawned);
    let child_ns = ns.clone();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                root.spawn(
                    program(|_child| std::future::pending()),
                    child_ns,
                    child_grant(CHILD),
                )
                .unwrap();
                spawned_c.notify_one();
                std::future::pending().await
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 100),
                (DimKey::Calls, 2),
                (DimKey::Depth, 1),
                (DimKey::WallMs, PARENT_WALL),
            ]),
        )
        .unwrap();
    spawned.notified().await;
    let state = kernel.state();
    let child = state
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::WallMs),
        Some(PARENT_OWN)
    );

    // The parent's window opens at PARENT_OWN - GRACE; the child, with far
    // more wall left, is not a candidate of its own.
    let mut now = 0;
    while now + PERIOD < PARENT_OWN - GRACE {
        now = clock.advance(PERIOD).unwrap();
    }
    assert!(cancels(&kernel.entries()).is_empty());
    let opened = clock.advance(PERIOD).unwrap();
    assert_eq!(opened, PARENT_OWN - GRACE);
    let state = kernel.state();
    let root_rec = state.agent(root).unwrap();
    let child_rec = state.agent(child).unwrap();
    assert_eq!(root_rec.status, Status::Cancelling);
    assert_eq!(root_rec.deadline, Some(PARENT_OWN));
    assert_eq!(
        child_rec.status,
        Status::Cancelling,
        "frozen with its parent"
    );
    assert_eq!(
        child_rec.deadline,
        Some(PARENT_OWN),
        "at the parent's deadline"
    );
    assert_eq!(
        child_rec.budget.get(&DimKey::WallMs),
        Some(CHILD - opened),
        "not near its own"
    );
    let notice = child_rec.mailbox.last().unwrap();
    assert_eq!(notice.kind, MsgKind::Notice);
    assert_eq!(notice.from, Endpoint::Harness);
    assert_eq!(
        cancels(&kernel.entries()),
        vec![(root, PARENT_OWN - now)],
        "one cancel, the root's"
    );

    let mut now = opened;
    while now + PERIOD < PARENT_OWN {
        now = clock.advance(PERIOD).unwrap();
        let state = kernel.state();
        assert_eq!(state.agent(root).unwrap().status, Status::Cancelling);
        assert_eq!(state.agent(child).unwrap().status, Status::Cancelling);
    }
    assert_eq!(clock.advance(PERIOD).unwrap(), PARENT_OWN);
    let state = kernel.state();
    assert_eq!(state.agent(root).unwrap().status, Status::Aborted);
    assert_eq!(
        state.agent(child).unwrap().status,
        Status::Aborted,
        "no orphan"
    );
    assert!(state.is_drained());
    assert_eq!(cancels(&kernel.entries()).len(), 1);
    kernel.shutdown();
    assert_eq!(kernel.claim(root).unwrap(), Outcome::Aborted);
    assert_refolds(&kernel, &sink);
}

/// ADR-0015 §2 and §6: a grace of zero is no policy. The M1b tree under a
/// zero-grace clock writes the same entries `m1b-wall.log` pins: no cancel,
/// two ticks, the child aborted by the second.
#[tokio::test]
async fn a_grace_of_zero_reproduces_the_m1b_wall_log() {
    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot(log, tokio_spawner);
    let clock = VirtualClock::new(Arc::clone(&kernel)).with_grace(Grace::flat(0));

    let cap = kernel
        .register_driver(
            hole_id(),
            BlackHole,
            Budget::from_dims([(DimKey::Tokens, CEILING)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([cap]);
    let sent = Arc::new(Notify::new());
    let child_ns = ns.clone();
    let sent_c = Arc::clone(&sent);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(cap, b"never answered").unwrap();
                            sent_c.notify_one();
                            let _ = child.recv(Match::Corr(corr)).await;
                            child.exit(b"unreachable")
                        }),
                        child_ns,
                        child_grant(CHILD_WALL),
                    )
                    .unwrap();
                let _ = root.wait(WaitFor::Child(child)).await.unwrap();
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    sent.notified().await;
    clock.advance(HALF_CHILD).unwrap();
    clock.advance(HALF_CHILD).unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    kernel.claim(root).unwrap();

    // The same entries as the pin, and the same state. The one thing that
    // may differ is the `abi` stamp on each envelope: it names the writer's
    // build, which has moved since the fixture was recorded, and the fold
    // never reads it.
    let pinned = Log::read_from(&include_bytes!("fixtures/m1b-wall.log")[..]).unwrap();
    let entries = kernel.entries();
    assert!(cancels(&entries).is_empty());
    assert_eq!(unstamped(&entries), unstamped(pinned.entries()));
    assert_eq!(kernel.state_hash(), fold(pinned.entries()).unwrap().hash());
    assert_eq!(
        kernel.state_hash().to_string(),
        "87610ce202a8c28b67abb14778094ac8c899b2f889141d41fbd848c36a003c50",
        "the hash m1b_wall pins"
    );
}

/// The entries with every envelope's `abi` stamp cleared.
fn unstamped(entries: &[Entry]) -> Vec<Entry> {
    entries
        .iter()
        .cloned()
        .map(|mut e| {
            match &mut e {
                Entry::Sent { msg, .. }
                | Entry::Replied { msg, .. }
                | Entry::Emitted { msg, .. }
                | Entry::Unanswered { msg, .. } => msg.abi = 0,
                _ => {}
            }
            e
        })
        .collect()
}

/// A per-agent policy: `None` leaves an agent on the hard path.
#[tokio::test]
async fn a_policy_may_leave_an_agent_on_the_hard_path() {
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    // Only agents granted at least CHILD_WALL get a lead time. The record
    // holds what remains; the grant is that plus what the clock has spent.
    let clock = VirtualClock::new(Arc::clone(&kernel)).with_grace(Grace::policy(|_, agent| {
        let remaining = agent.budget.get(&DimKey::WallMs)?;
        let spent = agent.spent.get(&DimKey::WallMs).copied().unwrap_or(0);
        (remaining + spent >= CHILD_WALL).then_some(GRACE)
    }));
    let ns = Namespace::from_caps(std::iter::empty());
    let spawned = Arc::new(Notify::new());
    let spawned_c = Arc::clone(&spawned);
    let (big_ns, small_ns) = (ns.clone(), ns.clone());
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                root.spawn(
                    program(|_c| std::future::pending()),
                    big_ns,
                    child_grant(CHILD_WALL),
                )
                .unwrap();
                root.spawn(
                    program(|_c| std::future::pending()),
                    small_ns,
                    child_grant(HALF_CHILD),
                )
                .unwrap();
                spawned_c.notify_one();
                std::future::pending().await
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    spawned.notified().await;
    let state = kernel.state();
    let mut children: Vec<AgentId> = state
        .agents()
        .filter_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .collect();
    children.sort_unstable();
    let [big, small] = children[..] else {
        panic!("two children, got {children:?}");
    };

    // The small child's window would open at 20; the policy skips it, and
    // it is aborted at 50 with no notice. The big one is warned at 70.
    let mut now = 0;
    while now < HALF_CHILD {
        now = clock.advance(PERIOD).unwrap();
    }
    let state = kernel.state();
    assert_eq!(state.agent(small).unwrap().status, Status::Aborted);
    assert!(state.agent(small).unwrap().mailbox.is_empty(), "no notice");
    assert_eq!(state.agent(big).unwrap().status, Status::Live);
    while now < CHILD_WALL - GRACE {
        now = clock.advance(PERIOD).unwrap();
    }
    let rec = kernel.state();
    let big_rec = rec.agent(big).unwrap();
    assert_eq!(big_rec.status, Status::Cancelling);
    assert_eq!(big_rec.deadline, Some(CHILD_WALL));
    assert_eq!(cancels(&kernel.entries()), vec![(big, GRACE + PERIOD)]);
    kernel.shutdown();
}
