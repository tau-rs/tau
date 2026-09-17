//! M3c: driver supervision, end to end (ADR-0014).
//!
//! A driver that answers is handled by its reply. These tests are about the
//! one that does not: its `handle` unwinds, or hangs past the bound it was
//! registered with, or the harness retires it. In every case the request
//! closes explicitly — a reply from the kernel, billed the ceiling if the
//! driver had taken it and nothing if it was still queued — the transition
//! is in the log, and the supervisor hears about it after the fact. Then
//! the part that is the point: the log refolds, by a reducer that holds no
//! health, to the live kernel's hash.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{
    AgentId, BlobRef, Budget, Consumption, Corr, DimKey, DownCause, DriverId, Endpoint, Msg,
    MsgKind, Name, Namespace, UnansweredCause,
};
use tau_kernel::driver::clock::VirtualClock;
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, DriverEvent, Kernel, KernelError};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{as_consumption, fold, Outcome, Refusal, State, Status};
use tau_kernel::syscall::{program, Match};
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

const RAISED: &str = "the driver raised instead of reporting";

/// A panic in a driver is what these tests are about; the default hook
/// would print each one to stderr as if it were the test's own. Every
/// other panic — an assertion in a program — still reports.
fn quiet_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let raised = payload.downcast_ref::<&str>().is_some_and(|m| *m == RAISED)
            || payload
                .downcast_ref::<String>()
                .is_some_and(|m| m == RAISED);
        if !raised {
            default(info);
        }
    }));
}

const FIXTURE: &str = "tests/fixtures/m3c-supervision.log";
const CEILING: u64 = 30;
const BOUND: u64 = 100;
const HALF_BOUND: u64 = 50;
const OVER: u64 = 40;

fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}

fn model_id() -> DriverId {
    DriverId::new(name("model"))
}

fn ceiling() -> Budget {
    Budget::from_dims([(DimKey::Tokens, CEILING)])
}

fn billed_ceiling() -> Consumption {
    as_consumption(&ceiling())
}

fn tokens(n: u64) -> Consumption {
    Consumption::from_dims([(DimKey::Tokens, n)])
}

/// Plenty for a handful of calls, and a wall grant the clock never empties.
fn root_grant() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 500),
        (DimKey::Calls, 20),
        (DimKey::WallMs, 1_000_000),
    ])
}

/// A driver whose behaviour is the payload's first word: `boom` unwinds,
/// `hang…` takes the request and never returns, `over` reports above the
/// ceiling, anything else is echoed under `tag` and billed one token per
/// byte. `taken` is notified when a `hang` request is taken.
#[derive(Clone)]
struct Scripted {
    tag: &'static str,
    taken: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

impl Scripted {
    fn new(tag: &'static str) -> Self {
        Self::sharing(tag, &Arc::default())
    }

    /// A fresh instance that reports `taken` on the same notify as another,
    /// so a test can follow a replacement's requests too.
    fn sharing(tag: &'static str, taken: &Arc<Notify>) -> Self {
        Self {
            tag,
            taken: Arc::clone(taken),
            dropped: Arc::default(),
        }
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl Driver for Scripted {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let payload = request.payload;
        if payload == b"boom" {
            panic!("{RAISED}");
        }
        if payload.starts_with(b"hang") {
            self.taken.notify_one();
            return Box::pin(std::future::pending());
        }
        if payload == b"over" {
            return Box::pin(async move { (b"re: over".to_vec(), tokens(OVER)) });
        }
        let mut answer = format!("{}: ", self.tag).into_bytes();
        answer.extend_from_slice(&payload);
        let cost = tokens(u64::try_from(payload.len()).unwrap());
        Box::pin(async move { (answer, cost) })
    }

    fn describe(&self) -> Option<ToolSchema> {
        Some(ToolSchema {
            description: "scripted".into(),
            input_schema: b"{}".to_vec(),
        })
    }

    fn abandon(&self, _corr: Corr) {}
}

/// What the supervisor did about an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    Nothing,
    Replaced,
    Retired,
}

/// The reference policy of ADR-0014 §6, as harness code: replace a crashed
/// driver up to `k` times and retire it on the `k+1`th; do nothing about a
/// first overdue request and replace on the second in a row; count
/// overdrafts and retire above `max_excess` tokens.
struct Policy {
    k: u32,
    max_excess: u64,
    crashes: BTreeMap<DriverId, u32>,
    overdue_in_a_row: BTreeMap<DriverId, u32>,
    excess: BTreeMap<DriverId, u64>,
    seen: Vec<DriverEvent>,
}

impl Policy {
    fn new(k: u32, max_excess: u64) -> Self {
        Self {
            k,
            max_excess,
            crashes: BTreeMap::new(),
            overdue_in_a_row: BTreeMap::new(),
            excess: BTreeMap::new(),
            seen: Vec::new(),
        }
    }

    fn on(
        &mut self,
        kernel: &Arc<Kernel>,
        event: DriverEvent,
        fresh: &dyn Fn() -> Scripted,
    ) -> Verb {
        self.seen.push(event.clone());
        match event {
            DriverEvent::Crashed { driver } => {
                let n = self.crashes.entry(driver.clone()).or_insert(0);
                *n += 1;
                if *n <= self.k {
                    self.overdue_in_a_row.remove(&driver);
                    kernel.replace_driver(&driver, fresh()).unwrap();
                    Verb::Replaced
                } else {
                    kernel.retire_driver(&driver).unwrap();
                    Verb::Retired
                }
            }
            DriverEvent::Overdue { driver, .. } => {
                let n = self.overdue_in_a_row.entry(driver.clone()).or_insert(0);
                *n += 1;
                if *n >= 2 {
                    self.overdue_in_a_row.remove(&driver);
                    kernel.replace_driver(&driver, fresh()).unwrap();
                    Verb::Replaced
                } else {
                    Verb::Nothing
                }
            }
            DriverEvent::Overdrew { driver, excess, .. } => {
                let total = self.excess.entry(driver.clone()).or_insert(0);
                *total += excess.get(&DimKey::Tokens).unwrap_or(0);
                if *total > self.max_excess {
                    kernel.retire_driver(&driver).unwrap();
                    Verb::Retired
                } else {
                    Verb::Nothing
                }
            }
            _ => Verb::Nothing,
        }
    }
}

/// Runs `policy` over the kernel's events until the kernel closes, telling
/// `acted` after every verb that changed a driver. Returns the policy.
fn supervise(
    kernel: &Arc<Kernel>,
    mut policy: Policy,
    acted: Arc<Notify>,
    fresh: impl Fn() -> Scripted + Send + 'static,
) -> tokio::task::JoinHandle<Policy> {
    let kernel = Arc::clone(kernel);
    tokio::spawn(async move {
        while let Ok(event) = kernel.supervise().await {
            if policy.on(&kernel, event, &fresh) != Verb::Nothing {
                acted.notify_one();
            }
        }
        policy
    })
}

fn health(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::DriverDown { cause, .. } => Some(format!("down:{cause:?}")),
            Entry::DriverUp { .. } => Some("up".into()),
            Entry::Unanswered { msg, cause, .. } => Some(format!(
                "unanswered:{cause:?}:{}",
                msg.consumed.as_ref().map_or("queued", |_| "taken")
            )),
            _ => None,
        })
        .collect()
}

fn unanswered_for(entries: &[Entry], corr: Corr) -> Option<(UnansweredCause, Option<Consumption>)> {
    entries.iter().find_map(|e| match e {
        Entry::Unanswered { msg, cause, .. } if msg.corr == Some(corr) => {
            Some((*cause, msg.consumed.clone()))
        }
        _ => None,
    })
}

/// A reply from the kernel: on the correlation, `Reply`, empty.
fn from_kernel(msg: &Msg, corr: Corr) {
    assert_eq!(msg.from, Endpoint::Kernel, "{msg:?}");
    assert_eq!(msg.kind, MsgKind::Reply);
    assert_eq!(msg.corr, Some(corr));
    assert_eq!(msg.payload, BlobRef::EMPTY);
}

/// Budgets plus reservations plus spent, per dimension, over every record.
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

fn conserved(state: &State, grant: &Budget) {
    let total = tree_total(state);
    for (dim, want) in grant.iter().filter(|(d, _)| **d != DimKey::Depth) {
        let over: u64 = state
            .agents()
            .map(|(_, a)| a.overdraft.get(dim).copied().unwrap_or(0))
            .sum();
        assert_eq!(
            total.get(dim).copied().unwrap_or(0),
            want + over,
            "`{dim}` conserved"
        );
    }
}

/// The log is the kernel: a fold from zero, and a fold of the bytes on
/// disk, both hash to the live state.
fn refolds(kernel: &Kernel, sink: &SharedBuf) -> Vec<u8> {
    let entries = kernel.entries();
    let live = kernel.state_hash();
    assert_eq!(fold(&entries).unwrap().hash(), live);
    let bytes = sink.contents();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(reread.entries(), entries.as_slice());
    assert_eq!(fold(reread.entries()).unwrap().hash(), live);
    bytes
}

/// A booted kernel with `model` registered under `bound`, a clock, and
/// the sink the log is written to.
fn boot(driver: Scripted, bound: Option<u64>) -> (Arc<Kernel>, SharedBuf, VirtualClock, Namespace) {
    let sink = SharedBuf::default();
    let kernel = Kernel::boot(Log::with_sink(sink.clone()).unwrap(), tokio_spawner);
    let cap = kernel
        .register_driver_with(model_id(), driver, ceiling(), bound)
        .unwrap();
    let clock = VirtualClock::new(Arc::clone(&kernel));
    (kernel, sink, clock, Namespace::from_caps([cap]))
}

fn model_cap(ns: &Namespace) -> tau_kernel::abi::Capability {
    ns.iter().next().unwrap()
}

// ----------------------------------------------------------------- crashed

#[tokio::test]
async fn a_crashed_driver_fails_its_taken_request_and_the_requester_reads_a_reply_from_the_kernel()
{
    quiet_panics();
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(model, b"boom").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                from_kernel(&reply, corr);
                assert_eq!(reply.consumed, Some(billed_ceiling()), "taken: the ceiling");
                assert_eq!(
                    root.read(reply.payload).as_deref(),
                    Some(&b""[..]),
                    "empty, not missing: the kernel authored no bytes"
                );
                root.exit(b"read it")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    let event = kernel.supervise().await.unwrap();
    assert_eq!(event, DriverEvent::Crashed { driver: model_id() });
    kernel.shutdown();

    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        ["down:Crashed", "unanswered:Crashed:taken"]
    );
    let state = kernel.state();
    let a = state.agent(root).unwrap();
    assert_eq!(a.status, Status::Exited);
    assert_eq!(
        a.spent.get(&DimKey::Tokens),
        Some(&CEILING),
        "billed the ceiling"
    );
    assert!(a.overdraft.is_empty());
    assert!(a.reserved.is_empty());
    assert_eq!(state.owner(Corr::new(0)), None);
    conserved(&state, &root_grant());
    assert!(matches!(
        kernel.claim(root).unwrap(),
        Outcome::Exited(blob) if kernel.read(blob).unwrap() == b"read it"
    ));
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn a_request_still_queued_at_the_crash_is_delivered_to_the_replacement() {
    quiet_panics();
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let supervisor = supervise(&kernel, Policy::new(3, 100), Arc::clone(&acted), || {
        Scripted::new("v2")
    });
    kernel
        .spawn_root(
            program(move |root| async move {
                // Both queued before the loop takes either: the first is
                // taken and crashes the driver, the second waits.
                let first = root.send(model, b"boom").unwrap();
                let second = root.send(model, b"hello").unwrap();
                let reply = root.recv(Match::Corr(first)).await.unwrap();
                from_kernel(&reply, first);
                let reply = root.recv(Match::Corr(second)).await.unwrap();
                assert_eq!(reply.from, Endpoint::Driver { id: model_id() });
                assert_eq!(root.read(reply.payload).unwrap(), b"v2: hello");
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();
    assert_eq!(policy.seen, [DriverEvent::Crashed { driver: model_id() }]);

    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        ["down:Crashed", "unanswered:Crashed:taken", "up"],
        "the queued request was never unanswered"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Replied { .. }))
            .count(),
        1
    );
    conserved(&kernel.state(), &root_grant());
    refolds(&kernel, &sink);
}

// ----------------------------------------------------------------- overdue

#[tokio::test]
async fn an_overdue_request_fails_at_the_tick_that_passes_its_bound() {
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let (kernel, sink, clock, ns) = boot(driver, Some(BOUND));
    let model = model_cap(&ns);
    let done = Arc::new(Notify::new());
    let done_root = Arc::clone(&done);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(model, b"hang").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                from_kernel(&reply, corr);
                assert_eq!(reply.consumed, Some(billed_ceiling()));
                done_root.notify_one();
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    taken.notified().await;

    // Short of the bound: still open, still reserved.
    clock.advance(BOUND - 1).unwrap();
    let state = kernel.state();
    assert_eq!(state.open_corrs(root).count(), 1);
    assert!(kernel
        .entries()
        .iter()
        .all(|e| !matches!(e, Entry::Unanswered { .. })));
    // The tick that reaches it closes it, inside the same `tick`.
    clock.advance(1).unwrap();
    let state = kernel.state();
    assert_eq!(state.open_corrs(root).count(), 0);
    assert_eq!(
        unanswered_for(&kernel.entries(), Corr::new(0)),
        Some((UnansweredCause::Overdue, Some(billed_ceiling())))
    );
    let event = kernel.supervise().await.unwrap();
    assert_eq!(
        event,
        DriverEvent::Overdue {
            driver: model_id(),
            corr: Corr::new(0),
            agent: root,
        }
    );

    done.notified().await;
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        ["unanswered:Overdue:taken"],
        "not declared down: slow and dead look the same from outside"
    );
    conserved(&kernel.state(), &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn a_request_queued_behind_a_hung_handle_goes_overdue_too() {
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let (kernel, sink, clock, ns) = boot(driver, Some(BOUND));
    let model = model_cap(&ns);
    let sent_second = Arc::new(Notify::new());
    let sent_second_root = Arc::clone(&sent_second);
    let half = Arc::new(Notify::new());
    let half_root = Arc::clone(&half);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let first = root.send(model, b"hang").unwrap();
                half_root.notified().await;
                let second = root.send(model, b"queued behind").unwrap();
                sent_second_root.notify_one();
                let reply = root.recv(Match::Corr(first)).await.unwrap();
                from_kernel(&reply, first);
                assert_eq!(reply.consumed, Some(billed_ceiling()), "taken");
                let reply = root.recv(Match::Corr(second)).await.unwrap();
                from_kernel(&reply, second);
                assert_eq!(reply.consumed, None, "queued: nothing ran");
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    taken.notified().await;
    clock.advance(HALF_BOUND).unwrap();
    half.notify_one();
    sent_second.notified().await;
    // The first's bound, not the second's.
    clock.advance(HALF_BOUND).unwrap();
    let entries = kernel.entries();
    assert_eq!(
        unanswered_for(&entries, Corr::new(0)),
        Some((UnansweredCause::Overdue, Some(billed_ceiling())))
    );
    assert_eq!(unanswered_for(&entries, Corr::new(1)), None);
    assert_eq!(kernel.state().open_corrs(root).count(), 1);
    // Then the second's, counted from its own `Sent`.
    clock.advance(HALF_BOUND).unwrap();
    assert_eq!(
        unanswered_for(&kernel.entries(), Corr::new(1)),
        Some((UnansweredCause::Overdue, None))
    );

    kernel.drained().await.unwrap();
    kernel.shutdown();
    let state = kernel.state();
    let a = state.agent(root).unwrap();
    assert_eq!(
        a.spent.get(&DimKey::Tokens),
        Some(&CEILING),
        "one ceiling, not two"
    );
    assert_eq!(a.budget.get(&DimKey::Tokens), Some(500 - CEILING));
    conserved(&state, &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn a_late_reply_to_an_unanswered_request_is_dead_letter() {
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let (kernel, sink, clock, ns) = boot(driver, Some(BOUND));
    let model = model_cap(&ns);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(model, b"hang").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                from_kernel(&reply, corr);
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    taken.notified().await;
    clock.advance(BOUND).unwrap();
    kernel.drained().await.unwrap();

    // The driver finally answers, through the same door its loop uses.
    let err = kernel
        .reply(&model_id(), Corr::new(0), b"so sorry", tokens(5))
        .unwrap_err();
    assert!(
        matches!(err, KernelError::Refused(Refusal::UnknownCorr(Some(c))) if c == Corr::new(0)),
        "got {err:?}"
    );
    kernel.shutdown();
    let entries = kernel.entries();
    assert!(!entries.iter().any(|e| matches!(e, Entry::Replied { .. })));
    let state = kernel.state();
    assert_eq!(
        state.agent(root).unwrap().spent.get(&DimKey::Tokens),
        Some(&CEILING),
        "no second bill"
    );
    conserved(&state, &root_grant());
    refolds(&kernel, &sink);
}

// ----------------------------------------------------------------- retired

#[tokio::test]
async fn a_retired_driver_fails_its_queue_and_its_capability_is_unroutable() {
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let dropped = Arc::clone(&driver.dropped);
    let (kernel, sink, _, ns) = boot(driver, None);
    let model = model_cap(&ns);
    let retired = Arc::new(Notify::new());
    let retired_root = Arc::clone(&retired);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                assert!(root.describe(model).unwrap().is_some(), "a tool while up");
                let first = root.send(model, b"hang").unwrap();
                let second = root.send(model, b"queued").unwrap();
                retired_root.notified().await;
                let reply = root.recv(Match::Corr(first)).await.unwrap();
                from_kernel(&reply, first);
                assert_eq!(reply.consumed, Some(billed_ceiling()), "taken");
                let reply = root.recv(Match::Corr(second)).await.unwrap();
                from_kernel(&reply, second);
                assert_eq!(reply.consumed, None, "queued");
                let err = root.send(model, b"anyone there").unwrap_err();
                assert!(
                    matches!(err, KernelError::Refused(Refusal::Unroutable(cap)) if cap == model),
                    "got {err:?}"
                );
                assert_eq!(root.describe(model).unwrap(), None, "no tool behind it");
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    taken.notified().await;
    kernel.retire_driver(&model_id()).unwrap();
    // A second retire is a no-op, not a second `DriverDown`.
    kernel.retire_driver(&model_id()).unwrap();
    for _ in 0..64 {
        if dropped.load(Ordering::SeqCst) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "the loop was aborted and let go of the instance"
    );
    retired.notify_one();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        [
            "down:Retired",
            "unanswered:Retired:taken",
            "unanswered:Retired:queued",
        ]
    );
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Sent { .. }))
            .count(),
        2,
        "the refused send never happened"
    );
    let state = kernel.state();
    let a = state.agent(root).unwrap();
    assert_eq!(a.spent.get(&DimKey::Tokens), Some(&CEILING));
    assert_eq!(a.spent.get(&DimKey::Calls), Some(&2));
    conserved(&state, &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn a_replaced_driver_answers_under_the_same_capability() {
    quiet_panics();
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let acted_root = Arc::clone(&acted);
    let supervisor = supervise(&kernel, Policy::new(3, 100), Arc::clone(&acted), || {
        Scripted::new("v2")
    });
    kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(model, b"boom").unwrap();
                from_kernel(&root.recv(Match::Corr(corr)).await.unwrap(), corr);
                acted_root.notified().await;
                // The same capability, the same namespace, a fresh instance.
                let corr = root.send(model, b"again").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(reply.from, Endpoint::Driver { id: model_id() });
                assert_eq!(root.read(reply.payload).unwrap(), b"v2: again");
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    supervisor.await.unwrap();

    let entries = kernel.entries();
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::DriverRegistered { .. }))
            .count(),
        1,
        "no second registration"
    );
    assert_eq!(
        health(&entries),
        ["down:Crashed", "unanswered:Crashed:taken", "up"]
    );
    assert_eq!(kernel.state().driver_cap(&model_id()), Some(model));
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn replacing_a_hung_driver_aborts_its_loop_and_fails_the_taken_request() {
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let dropped = Arc::clone(&driver.dropped);
    let (kernel, sink, _, ns) = boot(driver, None);
    let model = model_cap(&ns);
    kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(model, b"hang").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                from_kernel(&reply, corr);
                assert_eq!(reply.consumed, Some(billed_ceiling()));
                let corr = root.send(model, b"after").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(root.read(reply.payload).unwrap(), b"v2: after");
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    taken.notified().await;
    kernel
        .replace_driver(&model_id(), Scripted::new("v2"))
        .unwrap();
    for _ in 0..64 {
        if dropped.load(Ordering::SeqCst) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(dropped.load(Ordering::SeqCst), "the old loop was aborted");
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        ["down:Retired", "unanswered:Retired:taken", "up"]
    );
    assert!(
        kernel.supervise().await.is_err(),
        "a replacement by the harness is not an event: it already knows"
    );
    conserved(&kernel.state(), &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn shutdown_writes_no_health_entry() {
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let (kernel, sink, _, ns) = boot(driver, Some(BOUND));
    let model = model_cap(&ns);
    kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(model, b"hang").unwrap();
                let _ = root.recv(Match::Corr(corr)).await;
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    taken.notified().await;
    // The run is over with a request open on a hung driver: not a health
    // event, nothing written.
    kernel.shutdown();
    let err = kernel.supervise().await.unwrap_err();
    assert!(matches!(err, KernelError::Closed), "got {err:?}");
    let entries = kernel.entries();
    assert!(health(&entries).is_empty(), "{:?}", health(&entries));
    assert!(matches!(entries.last(), Some(Entry::Sent { .. })));
    refolds(&kernel, &sink);
}

// -------------------------------------------------------------- supervisor

#[tokio::test]
async fn the_supervisor_sees_crashed_overdue_and_overdrew_in_order() {
    // The whole story, and the fixture: a crash replaced, a hang that goes
    // overdue twice — the taken request and the one queued behind it — and
    // is replaced, a report above the ceiling, then a retirement once the
    // tree is done. Every transition is in the log; the fold needs none of
    // the health to reproduce the hash.
    quiet_panics();
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let (kernel, sink, clock, ns) = boot(driver, Some(BOUND));
    let model = model_cap(&ns);
    let clock = Arc::new(clock);
    let acted = Arc::new(Notify::new());
    let acted_root = Arc::clone(&acted);
    let taken_v2 = Arc::clone(&taken);
    let supervisor = supervise(
        &kernel,
        Policy::new(3, 100),
        Arc::clone(&acted),
        move || Scripted::sharing("v2", &taken_v2),
    );
    let root_clock = Arc::clone(&clock);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                // 1. crashed: the taken request fails at the ceiling.
                let corr = root.send(model, b"boom").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                from_kernel(&reply, corr);
                assert_eq!(reply.consumed, Some(billed_ceiling()));
                acted_root.notified().await;
                // 2. the replacement answers under the same capability.
                let corr = root.send(model, b"hello").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(root.read(reply.payload).unwrap(), b"v2: hello");
                // 3. overdue: one taken, one queued behind it, at the bound.
                let first = root.send(model, b"hang").unwrap();
                taken.notified().await;
                let second = root.send(model, b"hang too").unwrap();
                root_clock.advance(BOUND).unwrap();
                let reply = root.recv(Match::Corr(first)).await.unwrap();
                from_kernel(&reply, first);
                assert_eq!(reply.consumed, Some(billed_ceiling()));
                let reply = root.recv(Match::Corr(second)).await.unwrap();
                from_kernel(&reply, second);
                assert_eq!(reply.consumed, None);
                acted_root.notified().await;
                // 4. overdrew: answered, above the ceiling.
                let corr = root.send(model, b"over").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(reply.consumed, Some(tokens(OVER)));
                root.exit(b"supervised")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.retire_driver(&model_id()).unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();
    assert_eq!(
        policy.seen,
        [
            DriverEvent::Crashed { driver: model_id() },
            DriverEvent::Overdue {
                driver: model_id(),
                corr: Corr::new(2),
                agent: root,
            },
            DriverEvent::Overdue {
                driver: model_id(),
                corr: Corr::new(3),
                agent: root,
            },
            DriverEvent::Overdrew {
                driver: model_id(),
                corr: Corr::new(4),
                agent: root,
                excess: tokens(OVER - CEILING),
            },
        ]
    );

    // --- the log
    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        [
            "down:Crashed",
            "unanswered:Crashed:taken",
            "up",
            "unanswered:Overdue:taken",
            "unanswered:Overdue:queued",
            "down:Retired",
            "up",
            "down:Retired",
        ]
    );
    let registered = entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                Entry::DriverRegistered {
                    reply_within: Some(BOUND),
                    ..
                }
            )
        })
        .count();
    assert_eq!(registered, 1, "the bound is on the wire");
    assert!(
        matches!(
            entries.last(),
            Some(Entry::DriverDown {
                cause: DownCause::Retired,
                ..
            })
        ),
        "the retirement after the tree drained is the last line"
    );

    // --- the books: three ceilings, one refund, one overdraft
    let state = kernel.state();
    let a = state.agent(root).unwrap();
    assert_eq!(a.status, Status::Exited);
    assert_eq!(
        a.spent.get(&DimKey::Tokens),
        Some(&(CEILING + 5 + CEILING + OVER)),
        "boom, hello, hang, over; nothing for the queued one"
    );
    assert_eq!(a.overdraft.get(&DimKey::Tokens), Some(&(OVER - CEILING)));
    assert!(a.reserved.is_empty());
    assert!(state.is_drained());
    conserved(&state, &root_grant());
    assert!(matches!(
        kernel.claim(root).unwrap(),
        Outcome::Exited(blob) if kernel.read(blob).unwrap() == b"supervised"
    ));

    // --- the log is the kernel
    let bytes = refolds(&kernel, &sink);

    // Regenerate with `TAU_UPDATE_FIXTURES=1 cargo test -p tau-kernel --test
    // m3c_supervision`, and read the diff.
    if std::env::var_os("TAU_UPDATE_FIXTURES").is_some() {
        std::fs::write(FIXTURE, &bytes).unwrap();
    }
}

#[tokio::test]
async fn the_reference_policy_retires_after_k_crashes() {
    quiet_panics();
    const K: u32 = 3;
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let acted_root = Arc::clone(&acted);
    let supervisor = supervise(&kernel, Policy::new(K, 100), Arc::clone(&acted), || {
        Scripted::new("again")
    });
    kernel
        .spawn_root(
            program(move |root| async move {
                // K replacements, then the K+1th crash retires it.
                for _ in 0..=K {
                    let corr = root.send(model, b"boom").unwrap();
                    from_kernel(&root.recv(Match::Corr(corr)).await.unwrap(), corr);
                    acted_root.notified().await;
                }
                let err = root.send(model, b"boom").unwrap_err();
                assert!(
                    matches!(err, KernelError::Refused(Refusal::Unroutable(cap)) if cap == model),
                    "got {err:?}"
                );
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();
    assert_eq!(policy.seen.len() as u32, K + 1);
    assert_eq!(policy.crashes.get(&model_id()), Some(&(K + 1)));

    let entries = kernel.entries();
    let seen = health(&entries);
    assert_eq!(
        seen.iter().filter(|h| *h == "down:Crashed").count() as u32,
        K + 1
    );
    assert_eq!(seen.iter().filter(|h| *h == "up").count() as u32, K);
    assert_eq!(
        seen.iter().filter(|h| *h == "down:Retired").count(),
        0,
        "retiring a driver that is already down writes no second transition"
    );
    conserved(&kernel.state(), &root_grant());
    refolds(&kernel, &sink);
}

#[test]
fn the_fixture_refolds_to_the_pinned_state_hash() {
    // This binary has no driver, no clock and no supervisor. The fold
    // confirms the health entries and applies them as nothing, settles the
    // unanswered ones as it settles a reply, and reproduces the hash.
    let bytes = include_bytes!("fixtures/m3c-supervision.log");
    let log = Log::read_from(&bytes[..]).unwrap();
    assert_eq!(log.header().abi, 3);
    let state = fold(log.entries()).unwrap();
    assert!(state.is_drained());
    assert_eq!(state.now(), BOUND);
    let kinds = health(log.entries());
    assert!(kinds.iter().any(|h| h.starts_with("down:Crashed")));
    assert!(kinds.iter().any(|h| h.starts_with("down:Retired")));
    assert!(kinds.iter().any(|h| h == "up"));
    assert!(kinds.iter().any(|h| h == "unanswered:Overdue:queued"));
    assert!(kinds.iter().any(|h| h == "unanswered:Crashed:taken"));
    let root = AgentId::new(0);
    assert_eq!(
        state.agent(root).unwrap().overdraft.get(&DimKey::Tokens),
        Some(&(OVER - CEILING))
    );
    insta::assert_snapshot!(state.hash().to_string());
}
