//! M1a: `cancel`, end to end, both phases.
//!
//! A child blocks in `recv` on a request a driver will never answer. Its
//! parent cancels it with a grace period. The child sees the `CancelNotice`
//! and — misbehaving on purpose — keeps waiting for its reply. The driver is
//! told to abandon the request. A virtual clock ticks up to the deadline, the
//! reducer aborts the child, the parent's `wait` returns `Aborted`, and the
//! books balance: the child's whole grant is back with the parent, no
//! correlation is open, and the log refolds to the live state.
//!
//! This is Tier 2 property #1 — "cancel leaves no live descendants and no
//! leaked reservations" — written as one concrete run before the property
//! machinery exists.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{
    Budget, Consumption, Corr, DimKey, DriverId, Endpoint, Msg, MsgKind, Name, Namespace,
};
use tau_kernel::driver::clock::VirtualClock;
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{fold, Outcome, Status};
use tau_kernel::syscall::{program, CancelMode, Match, WaitFor};
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

/// A driver that never answers, and remembers what it was told to abandon.
#[derive(Clone, Default)]
struct BlackHole {
    abandoned: Arc<Mutex<Vec<Corr>>>,
}

impl Driver for BlackHole {
    fn handle(&self, _request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        Box::pin(std::future::pending())
    }

    fn abandon(&self, corr: Corr) {
        self.abandoned.lock().unwrap().push(corr);
    }
}

const FIXTURE: &str = "tests/fixtures/m1a-cancel.log";
const REASON: &[u8] = b"budget review";
const GRACE: u64 = 10;

fn hole_id() -> DriverId {
    DriverId::new(Name::new("blackhole").unwrap())
}

#[tokio::test]
async fn a_cancelled_child_is_notified_abandoned_and_aborted_at_the_deadline() {
    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot(log, tokio_spawner);
    let clock = VirtualClock::new(Arc::clone(&kernel));

    let hole = BlackHole::default();
    let cap = kernel.register_driver(hole_id(), hole.clone()).unwrap();
    let ns = Namespace::from_caps([cap]);

    // Plain memory shared between programs and the harness, so the harness
    // acts only once the run has reached the point being tested. Nothing here
    // is held across an await as a guard.
    let seen: Arc<Mutex<Vec<Msg>>> = Arc::default();
    let sent = Arc::new(Notify::new());
    let noticed = Arc::new(Notify::new());
    let cancelled = Arc::new(Notify::new());

    let child_ns = ns.clone();
    let (seen_c, sent_c, noticed_c, cancelled_c) = (
        Arc::clone(&seen),
        Arc::clone(&sent),
        Arc::clone(&noticed),
        Arc::clone(&cancelled),
    );
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let sent_root = Arc::clone(&sent_c);
                let child = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(cap, b"never answered").unwrap();
                            sent_c.notify_one();
                            // Either the reply or a cancel notice, whichever comes.
                            let msg = child
                                .recv(Match::Or(vec![
                                    Match::Corr(corr),
                                    Match::Kind(MsgKind::Notice),
                                ]))
                                .await
                                .unwrap();
                            seen_c.lock().unwrap().push(msg);
                            noticed_c.notify_one();
                            // Misbehave: ignore the notice and hold out for
                            // the reply. The deadline is not negotiable.
                            let _ = child.recv(Match::Corr(corr)).await;
                            child.exit(b"unreachable")
                        }),
                        child_ns,
                        Budget::from_dims([(DimKey::Tokens, 100)]),
                    )
                    .unwrap();
                sent_root.notified().await;
                root.cancel(child, CancelMode::grace(GRACE).with_reason(REASON))
                    .unwrap();
                cancelled_c.notify_one();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.agent, child);
                assert_eq!(done.outcome, Outcome::Aborted);
                root.exit(b"")
            }),
            ns,
            Budget::from_dims([(DimKey::Tokens, 1_000)]),
        )
        .unwrap();

    // --- phase one: freeze + notice + abandon
    cancelled.notified().await;
    noticed.notified().await;
    let state = kernel.state();
    let child = state
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    let rec = state.agent(child).unwrap();
    assert_eq!(rec.status, Status::Cancelling);
    assert_eq!(rec.deadline, Some(GRACE));
    let corr = Corr::new(0);
    assert_eq!(state.owner(corr), Some(child), "still open during grace");
    assert_eq!(*hole.abandoned.lock().unwrap(), vec![corr]);
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let notice = seen.first().unwrap();
        assert_eq!(notice.kind, MsgKind::Notice);
        assert_eq!(notice.from, Endpoint::Agent { id: root });
        assert_eq!(notice.corr, None);
        assert_eq!(kernel.read(notice.payload).unwrap(), REASON);
    }

    // --- phase two: the deadline, enforced by the reducer on tick
    clock.advance(GRACE - 1).unwrap();
    assert_eq!(
        kernel.state().agent(child).unwrap().status,
        Status::Cancelling
    );
    clock.advance(1).unwrap();
    assert_eq!(kernel.state().agent(child).unwrap().status, Status::Aborted);

    kernel.drained().await.unwrap();
    kernel.shutdown();

    // --- the books
    let state = kernel.state();
    let root_rec = state.agent(root).unwrap();
    let child_rec = state.agent(child).unwrap();
    assert_eq!(root_rec.status, Status::Exited);
    assert_eq!(child_rec.status, Status::Aborted);
    assert_eq!(
        root_rec.budget.get(&DimKey::Tokens),
        Some(1_000),
        "the child's whole grant came back: nothing was ever billed"
    );
    assert_eq!(child_rec.budget.get(&DimKey::Tokens), None);
    assert!(child_rec.mailbox.is_empty());
    assert_eq!(state.owner(corr), None, "no correlation left open");
    assert_eq!(state.live_count(), 0, "no live descendants");
    assert!(state.is_drained());
    assert_eq!(
        kernel.claim(root).unwrap(),
        Outcome::Exited(tau_kernel::abi::BlobRef::EMPTY)
    );

    // The child's abort is not an entry; it is what the tick did.
    let entries = kernel.entries();
    assert!(entries.iter().any(
        |e| matches!(e, Entry::Cancelled { agent, grace, .. } if *agent == child && *grace == GRACE)
    ));
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Tick { .. }))
            .count(),
        2
    );

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
    let bytes = include_bytes!("fixtures/m1a-cancel.log");
    let log = Log::read_from(&bytes[..]).unwrap();
    let state = fold(log.entries()).unwrap();
    assert!(state.is_drained());
    assert_eq!(state.now(), GRACE);
    insta::assert_snapshot!(state.hash().to_string());
}
