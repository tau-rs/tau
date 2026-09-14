//! M0: the walking skeleton, end to end.
//!
//! Harness boots → root spawns a child → child sends to the echo driver and
//! receives the reply → child exits with it → the root `wait`s for the child
//! and exits with the same bytes → the harness claims the root's result. Then
//! the part that is the actual point: fold the log twice and compare the
//! state hashes to the live kernel's. The log is the kernel.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{Budget, DimKey, DriverId, Name, Namespace};
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{AbortHandle, BoxFuture, Kernel};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{fold, Outcome, Status};
use tau_kernel::syscall::{program, Match, WaitFor};

/// A `Write` the test can read back after the kernel is done with it.
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

/// tokio as the body (ADR-0003): spawn, and hand back the abort.
fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

const FIXTURE: &str = "tests/fixtures/m0-walking-skeleton.log";
const PAYLOAD: &[u8] = b"hello, kernel";
const CEILING: u64 = 64;

fn tokens(n: u64) -> Budget {
    Budget::from_dims([(DimKey::Tokens, n)])
}

fn echo_id() -> DriverId {
    DriverId::new(Name::new("echo").unwrap())
}

#[tokio::test]
async fn the_loop_runs_and_its_log_refolds_to_the_same_state() {
    let sink = SharedBuf::default();
    let log = Log::with_sink(sink.clone()).unwrap();
    let kernel = Kernel::boot(log, tokio_spawner);

    // Boot: drivers first, so the root's namespace can name them.
    // The ceiling is the harness's word on what one echo may cost at most;
    // every send reserves it before delivery.
    let echo = kernel
        .register_driver(echo_id(), EchoDriver::new(), tokens(CEILING))
        .unwrap();
    let ns = Namespace::from_caps([echo]);

    let child_ns = ns.clone();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            let corr = child.send(echo, PAYLOAD).unwrap();
                            let reply = child.recv(Match::Corr(corr)).await.unwrap();
                            let echoed = child.read(reply.payload).unwrap();
                            child.exit(&echoed)
                        }),
                        child_ns,
                        Budget::from_dims([(DimKey::Tokens, 100), (DimKey::Calls, 1)]),
                    )
                    .unwrap();
                // Syscall 3: the parent claims its child's result.
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.agent, child);
                let bytes = root.read(done.result().unwrap()).unwrap();
                root.exit(&bytes)
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 1_000),
                (DimKey::Calls, 10),
                (DimKey::Depth, 1),
            ]),
        )
        .unwrap();

    kernel.drained().await.unwrap();
    kernel.shutdown();

    // --- the root's result is the child's, claimed through `wait`; the
    //     harness claims only what the tree left behind, and that is logged
    let Outcome::Exited(blob) = kernel.claim(root).unwrap() else {
        panic!("the root exited on its own");
    };
    assert_eq!(kernel.read(blob).unwrap(), PAYLOAD);
    assert!(kernel.claim(root).is_err(), "a result can be claimed once");

    let state = kernel.state();
    let child = state
        .agents()
        .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
        .unwrap();
    assert!(
        kernel.claim(child).is_err(),
        "the parent already claimed the child"
    );
    let entries = kernel.entries();
    assert!(entries.iter().any(|e| matches!(
        e,
        Entry::Claimed { agent, by: Some(by), .. } if *agent == child && *by == root
    )));

    // --- accounting: 1000 → root, 100 carved to the child, 64 held for the
    //     send, 13 spent, 51 refunded, 87 back; the one call charged
    let root_rec = state.agent(root).unwrap();
    let child_rec = state.agent(child).unwrap();
    assert_eq!(root_rec.status, Status::Exited);
    assert_eq!(child_rec.status, Status::Exited);
    assert_eq!(root_rec.budget.get(&DimKey::Tokens), Some(987));
    assert_eq!(root_rec.budget.get(&DimKey::Calls), Some(9));
    assert_eq!(
        child_rec.budget.get(&DimKey::Tokens),
        None,
        "returned at exit"
    );
    assert_eq!(child_rec.spent.get(&DimKey::Tokens), Some(&13));
    assert_eq!(child_rec.spent.get(&DimKey::Calls), Some(&1));
    assert!(child_rec.reserved.is_empty(), "settled at the reply");
    assert!(
        child_rec.overdraft.is_empty(),
        "the echo kept to its ceiling"
    );
    assert!(child_rec.mailbox.is_empty(), "the reply was resolved");
    assert!(state.is_drained());
    assert!(state.completed().is_empty(), "everything was claimed");

    // --- the log is the kernel: two folds, one hash, equal to the live state
    let live = kernel.state_hash();
    let once = fold(&entries).unwrap();
    let twice = fold(&entries).unwrap();
    assert_eq!(once, state);
    assert_eq!(once.hash(), live);
    assert_eq!(twice.hash(), live);

    // --- and the log survives the trip through bytes
    let bytes = sink.contents();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(reread.entries(), entries.as_slice());
    assert_eq!(fold(reread.entries()).unwrap().hash(), live);

    // The first fixture of the Tier 3 determinism corpus. Regenerate with
    // `TAU_UPDATE_FIXTURES=1 cargo test -p tau-kernel --test m0_walking_skeleton`
    // — and read the diff, because a changed fixture means the entry format
    // or the reducer moved.
    if std::env::var_os("TAU_UPDATE_FIXTURES").is_some() {
        std::fs::write(FIXTURE, &bytes).unwrap();
    }
}

#[test]
fn the_fixture_refolds_to_the_pinned_state_hash() {
    // The determinism sentinel at its smallest: yesterday's log, today's
    // reducer, the same hash. If this fails, the reducer's behaviour on an
    // existing log changed — which is the one thing it must never do quietly.
    let bytes = include_bytes!("fixtures/m0-walking-skeleton.log");
    let log = Log::read_from(&bytes[..]).unwrap();
    let state = fold(log.entries()).unwrap();
    assert!(state.is_drained());
    insta::assert_snapshot!(state.hash().to_string());
}
