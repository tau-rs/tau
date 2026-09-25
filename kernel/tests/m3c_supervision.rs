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

use std::collections::{BTreeMap, BTreeSet};
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
/// How many times its ceiling one report may be before the reference policy
/// calls it a blowout (ADR-0014 amendment 1).
const TIMES: u64 = 3;
/// A report far enough above the ceiling to be a blowout at `TIMES`.
const BLOWOUT: u64 = 4 * CEILING;
/// A charge on a dimension no ceiling here names: all of it lands as
/// overdraft, and the policy ignores every bit of it.
const UNCAPPED: u64 = 10_000;

fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}

fn model_id() -> DriverId {
    DriverId::new(name("model"))
}

fn ceiling() -> Budget {
    Budget::from_dims([(DimKey::Tokens, CEILING)])
}

/// A ceiling that caps compute instead of tokens, for the sandbox's shape.
fn cpu_ceiling() -> Budget {
    Budget::from_dims([(DimKey::ComputeMs, CEILING)])
}

/// A multiplier for every dimension these tests cap, as A3 requires.
fn blowout_multipliers() -> BTreeMap<DimKey, u64> {
    BTreeMap::from([(DimKey::Tokens, TIMES), (DimKey::ComputeMs, TIMES)])
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

/// [`root_grant`] plus the compute a `cpu_ceiling` driver spends.
fn cpu_grant() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 500),
        (DimKey::Calls, 20),
        (DimKey::WallMs, 1_000_000),
        (DimKey::ComputeMs, 500),
    ])
}

/// A driver whose behaviour is the payload's first word: `boom` unwinds,
/// `hang…` takes the request and never returns, `slow…` takes it and
/// answers once `release` is notified, `over` reports a little above the
/// ceiling, `blowout` reports far above it, `cpu` reports far above a
/// compute ceiling, `uncapped` bills a dimension no ceiling here names, and
/// anything else is echoed under `tag` and billed one token per byte.
/// `taken` is notified when a `hang` or `slow` request is taken.
#[derive(Clone)]
struct Scripted {
    tag: &'static str,
    taken: Arc<Notify>,
    release: Arc<Notify>,
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
            release: Arc::default(),
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
        if payload.starts_with(b"slow") {
            self.taken.notify_one();
            let release = Arc::clone(&self.release);
            let mut answer = format!("{}: ", self.tag).into_bytes();
            answer.extend_from_slice(&payload);
            let cost = tokens(u64::try_from(payload.len()).unwrap());
            return Box::pin(async move {
                release.notified().await;
                (answer, cost)
            });
        }
        if payload == b"over" {
            return Box::pin(async move { (b"re: over".to_vec(), tokens(OVER)) });
        }
        if payload == b"blowout" {
            return Box::pin(async move { (b"re: blowout".to_vec(), tokens(BLOWOUT)) });
        }
        if payload == b"cpu" {
            return Box::pin(async move {
                (
                    b"re: cpu".to_vec(),
                    Consumption::from_dims([(DimKey::ComputeMs, BLOWOUT)]),
                )
            });
        }
        if payload == b"uncapped" {
            return Box::pin(async move {
                (
                    b"re: uncapped".to_vec(),
                    Consumption::from_dims([(DimKey::CostMicroUsd, UNCAPPED)]),
                )
            });
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

/// The reference policy of ADR-0014 §6 and its amendments, as harness
/// code: replace a crashed driver up to `k` times and retire it on the
/// `k+1`th; do nothing about a first overdue request and replace on the
/// second on the same driver; and, for a report above the ceiling, look at
/// every dimension the harness capped, call one report above
/// `blowout[dim]` times its cap a blowout, let the first blowout go, and
/// retire on the second.
///
/// What it deliberately does not do is accumulate. Both drivers in this
/// workspace overshoot a soft fence by design (ADR-0009 §3, ADR-0013 §7),
/// so a running total fires on traffic rather than on misbehaviour.
///
/// What it cannot do is see a streak. Every event the kernel raises is a
/// fault; none reports a request that came back on time, so "the second
/// overdue in a row" is not a rule a supervisor can implement — it would
/// be "the second overdue, ever", wearing a rationale it does not earn
/// (amended 2026-09-25, #176). The policy says what it does instead: one
/// bit per driver, set by the first `Overdue`, acted on by the next.
struct Policy {
    k: u32,
    blowout: BTreeMap<DimKey, u64>,
    crashes: BTreeMap<DriverId, u32>,
    overdue: BTreeSet<DriverId>,
    blown: BTreeSet<DriverId>,
    seen: Vec<DriverEvent>,
}

impl Policy {
    /// `blowout` must carry a multiplier for every dimension of every named
    /// driver's ceiling: capping a dimension is the harness saying it cares
    /// about that dimension, so leaving its multiplier out is a harness bug
    /// and not a silent pass. A dimension no ceiling names is another
    /// matter — the kernel reports all of it as overdraft, and the policy
    /// ignores it, because the harness never claimed to be policing it.
    ///
    /// # Errors
    ///
    /// The first capped dimension with no multiplier.
    fn new(
        kernel: &Kernel,
        k: u32,
        blowout: BTreeMap<DimKey, u64>,
        drivers: &[DriverId],
    ) -> Result<Self, DimKey> {
        let state = kernel.state();
        for id in drivers {
            for (dim, _) in state.ceiling(id).into_iter().flat_map(Budget::iter) {
                if !blowout.contains_key(dim) {
                    return Err(dim.clone());
                }
            }
        }
        Ok(Self {
            k,
            blowout,
            crashes: BTreeMap::new(),
            overdue: BTreeSet::new(),
            blown: BTreeSet::new(),
            seen: Vec::new(),
        })
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
                    // The overdue bit belongs to the instance, and a crash
                    // is the one recovery the events do report: the
                    // replacement is a fresh loop, and its predecessor's
                    // slow request says nothing about it.
                    self.overdue.remove(&driver);
                    kernel.replace_driver(&driver, fresh()).unwrap();
                    Verb::Replaced
                } else {
                    kernel.retire_driver(&driver).unwrap();
                    Verb::Retired
                }
            }
            DriverEvent::Overdue { driver, .. } => {
                if self.overdue.insert(driver.clone()) {
                    // The first: already unanswered, and slow and dead look
                    // the same from outside. Remembered as one bit, not a
                    // counter of consecutive overdues — nothing could
                    // break such a streak, so it would count every one.
                    Verb::Nothing
                } else {
                    // The second on the same instance. This may be a hung
                    // loop or two unrelated slow requests, and the
                    // supervisor cannot tell which: it takes the blunter
                    // reading because a replacement costs one instance
                    // and a missed hang costs every request behind it.
                    // The fresh instance starts with its bit clear.
                    self.overdue.remove(&driver);
                    kernel.replace_driver(&driver, fresh()).unwrap();
                    Verb::Replaced
                }
            }
            DriverEvent::Overdrew { driver, excess, .. } => {
                let state = kernel.state();
                // Only the dimensions this driver was capped on: a charge
                // on any other is reported as overdraft in full, and was
                // never the harness's to police.
                let blowout = state
                    .ceiling(&driver)
                    .into_iter()
                    .flat_map(Budget::iter)
                    .any(|(dim, cap)| {
                        self.blowout.get(dim).is_some_and(|times| {
                            // A report is a blowout above `times` its cap.
                            // The report is cap + excess, so that is excess
                            // above (times - 1) × cap — written as a
                            // product because division is denied here, and
                            // leaving a cap of zero blown by any excess at
                            // all, which is what capping at zero meant.
                            excess.get(dim).unwrap_or(0)
                                > cap.saturating_mul(times.saturating_sub(1))
                        })
                    });
                if !blowout {
                    Verb::Nothing
                } else if self.blown.insert(driver.clone()) {
                    // Strike one is free: a limiter that is not applying
                    // and a one-off workload that escaped a soft fence look
                    // identical in one report and nothing alike across two.
                    Verb::Nothing
                } else {
                    kernel.retire_driver(&driver).unwrap();
                    Verb::Retired
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
    boot_with(driver, bound, ceiling())
}

/// [`boot`] under a ceiling of the harness's choosing, for the tests whose
/// driver is capped on a dimension other than `tokens`.
fn boot_with(
    driver: Scripted,
    bound: Option<u64>,
    ceiling: Budget,
) -> (Arc<Kernel>, SharedBuf, VirtualClock, Namespace) {
    let sink = SharedBuf::default();
    let kernel = Kernel::boot(Log::with_sink(sink.clone()).unwrap(), tokio_spawner);
    let cap = kernel
        .register_driver_with(model_id(), driver, ceiling, bound)
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
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        Arc::clone(&acted),
        || Scripted::new("v2"),
    );
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

#[tokio::test]
async fn a_second_overdue_on_the_same_driver_replaces_it_and_nothing_between_could_have_reset_the_first(
) {
    // ADR-0014 §6, the `Overdue` row as amended 2026-09-25 (#176): the
    // first overdue is remembered, the second on the same driver replaces
    // it — and "the same driver" is the whole rule, because no event
    // reports a request that came back. Here v1 goes overdue, is released
    // and answers `hello` cleanly, then goes overdue again: the clean reply
    // is in the log and never reached the supervisor, which saw exactly two
    // events and replaced v1 on the second.
    let driver = Scripted::new("v1");
    let taken = Arc::clone(&driver.taken);
    let release = Arc::clone(&driver.release);
    let (kernel, sink, clock, ns) = boot(driver, Some(BOUND));
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let acted_root = Arc::clone(&acted);
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        Arc::clone(&acted),
        || Scripted::new("v2"),
    );
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                // 1. overdue: taken, then past the bound. The supervisor
                //    does nothing; v1 stays up.
                let first = root.send(model, b"slow").unwrap();
                let reply = root.recv(Match::Corr(first)).await.unwrap();
                from_kernel(&reply, first);
                assert_eq!(reply.consumed, Some(billed_ceiling()));
                // 2. the same instance answers cleanly. Not an event.
                let corr = root.send(model, b"hello").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(root.read(reply.payload).unwrap(), b"v1: hello");
                // 3. overdue again on v1: replaced.
                let second = root.send(model, b"slow again").unwrap();
                let reply = root.recv(Match::Corr(second)).await.unwrap();
                from_kernel(&reply, second);
                assert_eq!(reply.consumed, Some(billed_ceiling()));
                acted_root.notified().await;
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
    clock.advance(BOUND).unwrap();
    // v1's late answer is dead letter; its loop is free for `hello`.
    release.notify_one();
    taken.notified().await;
    clock.advance(BOUND).unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();

    assert_eq!(
        policy.seen,
        [
            DriverEvent::Overdue {
                driver: model_id(),
                corr: Corr::new(0),
                agent: root,
            },
            DriverEvent::Overdue {
                driver: model_id(),
                corr: Corr::new(2),
                agent: root,
            },
        ],
        "two events, both faults: the clean reply between them is not one"
    );
    assert!(policy.overdue.is_empty(), "the replacement starts clear");
    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        [
            "unanswered:Overdue:taken",
            "unanswered:Overdue:taken",
            "down:Retired",
            "up",
        ]
    );
    assert!(
        entries.iter().any(|e| matches!(
            e,
            Entry::Replied { msg, .. } if msg.corr == Some(Corr::new(1))
        )),
        "the clean reply is in the log, where a harness with its own means could see it"
    );
    conserved(&kernel.state(), &root_grant());
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
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        Arc::clone(&acted),
        || Scripted::new("v2"),
    );
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
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
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
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, K, blowout_multipliers(), &[model_id()]).unwrap(),
        Arc::clone(&acted),
        || Scripted::new("again"),
    );
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

// --------------------------------------------------------------- overdrew

#[tokio::test]
async fn one_blowout_is_free_and_the_second_retires_the_driver() {
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let acted_root = Arc::clone(&acted);
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        Arc::clone(&acted),
        || Scripted::new("v2"),
    );
    kernel
        .spawn_root(
            program(move |root| async move {
                for _ in 0..2 {
                    let corr = root.send(model, b"blowout").unwrap();
                    let reply = root.recv(Match::Corr(corr)).await.unwrap();
                    assert_eq!(reply.consumed, Some(tokens(BLOWOUT)));
                }
                acted_root.notified().await;
                let err = root.send(model, b"anyone there").unwrap_err();
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
    assert_eq!(
        policy.seen.len(),
        2,
        "one event per report above the ceiling"
    );
    assert!(policy.blown.contains(&model_id()));

    let entries = kernel.entries();
    assert_eq!(
        health(&entries),
        ["down:Retired"],
        "the second blowout, and nothing else"
    );
    let state = kernel.state();
    let a = state.agent(AgentId::new(0)).unwrap();
    assert_eq!(
        a.overdraft.get(&DimKey::Tokens),
        Some(&(2 * (BLOWOUT - CEILING))),
        "both blowouts are on the agent either way"
    );
    conserved(&state, &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn a_blowout_on_a_capped_dimension_other_than_tokens_retires_the_driver() {
    let (kernel, sink, _, ns) = boot_with(Scripted::new("v1"), None, cpu_ceiling());
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let acted_root = Arc::clone(&acted);
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        Arc::clone(&acted),
        || Scripted::new("v2"),
    );
    kernel
        .spawn_root(
            program(move |root| async move {
                let billed = Consumption::from_dims([(DimKey::ComputeMs, BLOWOUT)]);
                for _ in 0..2 {
                    let corr = root.send(model, b"cpu").unwrap();
                    let reply = root.recv(Match::Corr(corr)).await.unwrap();
                    assert_eq!(reply.consumed, Some(billed.clone()));
                }
                acted_root.notified().await;
                let err = root.send(model, b"anyone there").unwrap_err();
                assert!(
                    matches!(err, KernelError::Refused(Refusal::Unroutable(cap)) if cap == model),
                    "got {err:?}"
                );
                root.exit(b"")
            }),
            ns,
            cpu_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();
    assert_eq!(policy.seen.len(), 2);

    assert_eq!(health(&kernel.entries()), ["down:Retired"]);
    let state = kernel.state();
    let a = state.agent(AgentId::new(0)).unwrap();
    assert_eq!(
        a.overdraft.get(&DimKey::ComputeMs),
        Some(&(2 * (BLOWOUT - CEILING)))
    );
    assert_eq!(a.overdraft.get(&DimKey::Tokens), None, "tokens never moved");
    conserved(&state, &cpu_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn excess_on_a_dimension_the_ceiling_does_not_name_never_retires() {
    const N: usize = 3;
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        acted,
        || Scripted::new("v2"),
    );
    kernel
        .spawn_root(
            program(move |root| async move {
                let billed = Consumption::from_dims([(DimKey::CostMicroUsd, UNCAPPED)]);
                for _ in 0..N {
                    let corr = root.send(model, b"uncapped").unwrap();
                    let reply = root.recv(Match::Corr(corr)).await.unwrap();
                    assert_eq!(reply.consumed, Some(billed.clone()));
                }
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();
    assert_eq!(
        policy.seen.len(),
        N,
        "every report raises the event: the whole charge is overdraft"
    );
    assert!(
        policy.blown.is_empty(),
        "a dimension the harness never capped is not the policy's business"
    );

    assert!(
        health(&kernel.entries()).is_empty(),
        "the driver is still up"
    );
    let state = kernel.state();
    let a = state.agent(AgentId::new(0)).unwrap();
    assert_eq!(
        a.overdraft.get(&DimKey::CostMicroUsd),
        Some(&(N as u64 * UNCAPPED)),
        "loud in the state hash all the same"
    );
    conserved(&state, &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn small_overruns_never_add_up_to_a_retirement() {
    const N: usize = 3;
    let (kernel, sink, _, ns) = boot(Scripted::new("v1"), None);
    let model = model_cap(&ns);
    let acted = Arc::new(Notify::new());
    let supervisor = supervise(
        &kernel,
        Policy::new(&kernel, 3, blowout_multipliers(), &[model_id()]).unwrap(),
        acted,
        || Scripted::new("v2"),
    );
    kernel
        .spawn_root(
            program(move |root| async move {
                for _ in 0..N {
                    let corr = root.send(model, b"over").unwrap();
                    let reply = root.recv(Match::Corr(corr)).await.unwrap();
                    assert_eq!(reply.consumed, Some(tokens(OVER)));
                }
                root.exit(b"")
            }),
            ns,
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    let policy = supervisor.await.unwrap();
    assert_eq!(policy.seen.len(), N);
    assert!(
        policy.blown.is_empty(),
        "the documented soft fence is not a firing offence, however often"
    );

    assert!(health(&kernel.entries()).is_empty());
    let state = kernel.state();
    let a = state.agent(AgentId::new(0)).unwrap();
    assert_eq!(
        a.overdraft.get(&DimKey::Tokens),
        Some(&(N as u64 * (OVER - CEILING)))
    );
    conserved(&state, &root_grant());
    refolds(&kernel, &sink);
}

#[tokio::test]
async fn a_capped_dimension_with_no_multiplier_is_a_harness_bug() {
    let (kernel, _, _, _) = boot_with(Scripted::new("v1"), None, cpu_ceiling());
    let missing = Policy::new(&kernel, 3, BTreeMap::new(), &[model_id()]).err();
    assert_eq!(
        missing,
        Some(DimKey::ComputeMs),
        "capping a dimension is the harness saying it polices that dimension"
    );
    let unknown = DriverId::new(name("never-registered"));
    assert!(
        Policy::new(&kernel, 3, BTreeMap::new(), &[unknown]).is_ok(),
        "no ceiling, nothing to cover"
    );
    kernel.shutdown();
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
