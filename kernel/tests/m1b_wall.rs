//! M1b: wall time, end to end, from both clocks.
//!
//! A root with a wall grant spawns a child with a smaller one. The child sends
//! a request a driver will never answer and waits for the reply. A virtual
//! clock ticks: each tick moves elapsed time out of both agents' wall grants,
//! and the tick that empties the child's aborts it — inside the apply of the
//! tick, with no entry of its own. The parent's `wait` returns `Aborted`, the
//! books balance along every dimension, no reservation leaks, and the log
//! refolds to the live state. Then the wall clock proper: real time, on a
//! short interval, appends at least one tick before a bounded deadline.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tau_kernel::abi::{Budget, Consumption, Corr, DimKey, DriverId, Name, Namespace};
use tau_kernel::driver::clock::{Sleep, VirtualClock, WallClock};
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

const FIXTURE: &str = "tests/fixtures/m1b-wall.log";
const CEILING: u64 = 5;
const ROOT_WALL: u64 = 1_000;
const CHILD_WALL: u64 = 100;
const HALF: u64 = 50;

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

#[tokio::test]
async fn a_child_is_ended_by_the_tick_that_exhausts_its_wall_grant() {
    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot(log, tokio_spawner);
    let clock = VirtualClock::new(Arc::clone(&kernel));

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
                        Budget::from_dims([
                            (DimKey::Tokens, 10),
                            (DimKey::Calls, 1),
                            (DimKey::WallMs, CHILD_WALL),
                        ]),
                    )
                    .unwrap();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.agent, child);
                assert_eq!(done.outcome, Outcome::Aborted);
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();

    sent.notified().await;
    let state = kernel.state();
    let child = state
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    let corr = Corr::new(0);
    assert_eq!(state.owner(corr), Some(child));
    assert_eq!(
        state.agent(root).unwrap().budget.get(&DimKey::WallMs),
        Some(ROOT_WALL - CHILD_WALL),
        "carved"
    );

    // --- the clock spends wall on both; the tick that empties the child's
    //     ends it, and only it
    clock.advance(HALF).unwrap();
    let state = kernel.state();
    assert_eq!(
        state.agent(child).unwrap().budget.get(&DimKey::WallMs),
        Some(CHILD_WALL - HALF)
    );
    assert_eq!(state.agent(child).unwrap().status, Status::Live);
    clock.advance(HALF).unwrap();
    assert_eq!(kernel.state().agent(child).unwrap().status, Status::Aborted);

    // The root's `wait` resolves to the abort and it exits on its own.
    kernel.drained().await.unwrap();
    kernel.shutdown();

    // --- the books
    let state = kernel.state();
    let root_rec = state.agent(root).unwrap();
    let child_rec = state.agent(child).unwrap();
    assert_eq!(root_rec.status, Status::Exited);
    assert_eq!(child_rec.status, Status::Aborted);
    assert_eq!(child_rec.spent.get(&DimKey::WallMs), Some(&CHILD_WALL));
    assert_eq!(root_rec.spent.get(&DimKey::WallMs), Some(&CHILD_WALL));
    assert_eq!(
        root_rec.budget.get(&DimKey::WallMs),
        Some(ROOT_WALL - 2 * CHILD_WALL),
        "the child's wall was spent, not returned; the root's own passed too"
    );
    assert_eq!(
        root_rec.budget.get(&DimKey::Tokens),
        Some(100),
        "the tokens came back, reservation included"
    );
    assert!(child_rec.reserved.is_empty(), "no leaked reservation");
    assert_eq!(state.owner(corr), None);
    assert!(state.is_drained());
    let total = tree_total(&state);
    for (dim, want) in root_grant().iter().filter(|(d, _)| **d != DimKey::Depth) {
        assert_eq!(total.get(dim), Some(&want), "`{dim}` conserved");
    }
    assert_eq!(
        kernel.claim(root).unwrap(),
        Outcome::Exited(tau_kernel::abi::BlobRef::EMPTY)
    );

    // The abort is not an entry; it is what the second tick did.
    let entries = kernel.entries();
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Tick { .. }))
            .count(),
        2
    );
    assert!(!entries.iter().any(|e| matches!(e, Entry::Cancelled { .. })));

    // --- the log is the kernel
    let state = kernel.state();
    let live = kernel.state_hash();
    let once = fold(&entries).unwrap();
    assert_eq!(once, state);
    assert_eq!(once.hash(), live);
    assert_eq!(fold(&entries).unwrap().hash(), live);
    let bytes = sink.contents();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(reread.entries(), entries.as_slice());
    assert_eq!(fold(reread.entries()).unwrap().hash(), live);

    if std::env::var_os("TAU_UPDATE_FIXTURES").is_some() {
        std::fs::write(FIXTURE, &bytes).unwrap();
    }
}

#[test]
fn the_fixture_refolds_to_the_pinned_state_hash() {
    let bytes = include_bytes!("fixtures/m1b-wall.log");
    let log = Log::read_from(&bytes[..]).unwrap();
    let state = fold(log.entries()).unwrap();
    assert!(state.is_drained());
    assert_eq!(state.now(), CHILD_WALL);
    insta::assert_snapshot!(state.hash().to_string());
}

/// The wall clock reads real time and appends it. One tick within a bounded
/// wait is the whole claim: everything downstream of a tick is covered by
/// the virtual-clock tests, which are exact.
#[tokio::test]
async fn the_wall_clock_appends_a_tick_from_real_time() {
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let clock = Arc::new(WallClock::new(
        Arc::clone(&kernel),
        Duration::from_millis(5),
    ));
    let sleep: Sleep = Arc::new(|d| Box::pin(tokio::time::sleep(d)));
    let loop_task = tokio::spawn(Arc::clone(&clock).run(sleep));

    let ticked = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if kernel
                .entries()
                .iter()
                .any(|e| matches!(e, Entry::Tick { .. }))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    assert!(ticked.is_ok(), "no tick within two seconds");
    let now = kernel.state().now();
    assert!(now >= 5, "a reading after at least one period, got {now}");
    assert!(clock.now() >= now, "the clock only goes forward");

    // Shutdown ends the loop: the next tick is refused and the task returns.
    kernel.shutdown();
    tokio::time::timeout(Duration::from_secs(2), loop_task)
        .await
        .expect("the loop stops once the kernel is closed")
        .unwrap();
}
