//! M2a: `attach` and the five hook points, end to end (ADR-0008).
//!
//! The harness installs eight native hooks at boot, then a root and a child
//! run a small tree through every point: a `PreSend` that tattles and one
//! that denies, a `PreDeliver` that relays to the parent, an `OnSpawn` that
//! stops the tree at a depth, an `OnExit` obituary, an `OnBudget` line the
//! root crosses three times — twice by sending, once by carving a child —
//! a hook that answers `Deny` where none is admitted, and one that fails
//! closed. Every verdict is in the log; then the part that is the point:
//! the log is read back and folded by a reducer that has none of these
//! programs, and the hash matches the live kernel's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tau_kernel::abi::{
    AgentId, Budget, Consumption, DimKey, DriverId, Endpoint, HookId, Msg, MsgKind, Name, Namespace,
};
use tau_kernel::driver::Driver;
use tau_kernel::hook::{
    FailureMode, HookEvent, HookFailure, HookPoint, HookProgram, HookSource, Rule, Ruling, Verdict,
};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel, KernelError};
use tau_kernel::log::{Entry, Log};
use tau_kernel::reducer::{fold, Outcome, Refusal, Status};
use tau_kernel::syscall::{program, CancelMode, Match, WaitFor};
use tokio::sync::Semaphore;

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

/// A driver that answers only when handed a permit, so the test decides
/// when a reply — and its `PreDeliver` moment — happens. Reports no cost,
/// so every reply refunds the whole reservation.
#[derive(Clone)]
struct Gated {
    permits: Arc<Semaphore>,
}

impl Driver for Gated {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let permits = Arc::clone(&self.permits);
        Box::pin(async move {
            let permit = permits.acquire().await.expect("never closed");
            permit.forget();
            let mut answer = b"re: ".to_vec();
            answer.extend_from_slice(&request.payload);
            (answer, Consumption::none())
        })
    }
}

const FIXTURE: &str = "tests/fixtures/m2a-hooks.log";
const CEILING: u64 = 30;
const LOW: u64 = 50;

fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}

fn tool_id() -> DriverId {
    DriverId::new(name("tool"))
}

fn hook(n: u64) -> HookId {
    HookId::new(n)
}

fn low_tokens() -> HookPoint {
    HookPoint::OnBudget {
        dim: DimKey::Tokens,
        below: LOW,
    }
}

fn native<F>(s: &str, f: F) -> HookProgram
where
    F: Fn(&HookEvent) -> Result<Verdict, HookFailure> + Send + Sync + 'static,
{
    HookProgram::native(name(s), f)
}

/// The policy: eight hooks, ids 0..8 in this order.
fn install(kernel: &Arc<Kernel>) {
    let closed = FailureMode::Closed;
    let open = FailureMode::Open;
    // 0: tattle to the parent about any shell request. Emit before a deny
    //    is still delivered: verdicts are independent.
    kernel
        .attach(
            HookPoint::PreSend,
            native("tattle-shell", |e| {
                Ok(match e {
                    HookEvent::PreSend {
                        parent: Some(parent),
                        payload,
                        ..
                    } if payload.starts_with(b"shell") => Verdict::Emit {
                        to: *parent,
                        payload: b"child asked for a shell".to_vec(),
                    },
                    _ => Verdict::Allow,
                })
            }),
            closed,
        )
        .unwrap();
    // 1: no shell below depth 2.
    kernel
        .attach(
            HookPoint::PreSend,
            native("no-shell-below-depth-2", |e| {
                Ok(match e {
                    HookEvent::PreSend { depth, payload, .. }
                        if payload.starts_with(b"shell") && *depth < 2 =>
                    {
                        Verdict::Deny("shell needs depth 2".into())
                    }
                    _ => Verdict::Allow,
                })
            }),
            closed,
        )
        .unwrap();
    // 2: relay every reply a child gets to its parent.
    kernel
        .attach(
            HookPoint::PreDeliver,
            native("relay-to-parent", |e| {
                Ok(match e {
                    HookEvent::PreDeliver {
                        parent: Some(parent),
                        ..
                    } => Verdict::Emit {
                        to: *parent,
                        payload: b"child heard back".to_vec(),
                    },
                    _ => Verdict::Allow,
                })
            }),
            closed,
        )
        .unwrap();
    // 3: the tree stops at depth 1.
    kernel
        .attach(
            HookPoint::OnSpawn,
            native("tree-too-deep", |e| {
                Ok(match e {
                    HookEvent::OnSpawn { depth, .. } if *depth < 1 => {
                        Verdict::Deny("tree too deep".into())
                    }
                    _ => Verdict::Allow,
                })
            }),
            closed,
        )
        .unwrap();
    // 4: an obituary to the parent — or to the deceased, for the root,
    //    which dead-letters.
    kernel
        .attach(
            HookPoint::OnExit,
            native("obituary", |e| {
                Ok(match e {
                    HookEvent::OnExit {
                        subject,
                        parent,
                        outcome,
                        ..
                    } => Verdict::Emit {
                        to: parent.unwrap_or(*subject),
                        payload: match outcome {
                            Outcome::Exited(_) => b"exited".to_vec(),
                            Outcome::Aborted => b"aborted".to_vec(),
                        },
                    },
                    _ => Verdict::Allow,
                })
            }),
            open,
        )
        .unwrap();
    // 5: tokens low, to the agent whose grant crossed.
    kernel
        .attach(
            low_tokens(),
            native("tokens-low", |e| {
                Ok(match e {
                    HookEvent::OnBudget { subject, .. } => Verdict::Emit {
                        to: *subject,
                        payload: b"tokens low".to_vec(),
                    },
                    _ => Verdict::Allow,
                })
            }),
            open,
        )
        .unwrap();
    // 6: says no where nothing can be stopped. A program failure, recorded.
    kernel
        .attach(
            HookPoint::OnExit,
            native("grumpy", |_| Ok(Verdict::Deny("stay".into()))),
            open,
        )
        .unwrap();
    // 7: breaks on a particular request. Closed, so the break is a deny.
    kernel
        .attach(
            HookPoint::PreSend,
            native("flaky", |e| match e {
                HookEvent::PreSend { payload, .. } if payload == b"flaky" => {
                    Err(HookFailure::new("guard exploded"))
                }
                _ => Ok(Verdict::Allow),
            }),
            closed,
        )
        .unwrap();
}

/// 70 tokens: one send's reservation of 30 crosses the line at 50, the
/// refund brings it back over, and carving the child's 40 crosses it again.
fn root_grant() -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 70),
        (DimKey::Calls, 10),
        (DimKey::Depth, 2),
    ])
}

fn child_grant() -> Budget {
    Budget::from_dims([(DimKey::Tokens, 40), (DimKey::Calls, 4), (DimKey::Depth, 1)])
}

/// One roll call as the log shows it: point, subject, rulings.
type Moment<'a> = (&'a HookPoint, AgentId, &'a [(HookId, Ruling)]);

fn from_hook(n: u64) -> Match {
    Match::Sender(Endpoint::Hook { id: hook(n) })
}

fn note(msg: &Msg, from: u64) {
    assert_eq!(msg.from, Endpoint::Hook { id: hook(from) });
    assert_eq!(msg.kind, MsgKind::Notice);
    assert_eq!(msg.corr, None);
}

#[tokio::test]
async fn every_point_fires_every_verdict_is_logged_and_the_log_refolds_without_the_programs() {
    let sink = SharedBuf::default();
    let kernel = Kernel::boot(Log::with_sink(sink.clone()).unwrap(), tokio_spawner);
    let permits = Arc::new(Semaphore::new(0));
    let tool = kernel
        .register_driver(
            tool_id(),
            Gated {
                permits: Arc::clone(&permits),
            },
            Budget::from_dims([(DimKey::Tokens, CEILING)]),
        )
        .unwrap();
    install(&kernel);
    let ns = Namespace::from_caps([tool]);

    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen_root = Arc::clone(&seen);
    let child_ns = ns.clone();
    let root_permits = Arc::clone(&permits);
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let log = |s: &str| seen_root.lock().unwrap().push(s.to_owned());
                // --- OnBudget, by sending: 70 → 40 crosses 50, once.
                let corr = root.send(tool, b"one").unwrap();
                let low = root.recv(from_hook(5)).await.unwrap();
                note(&low, 5);
                assert_eq!(root.read(low.payload).unwrap(), b"tokens low");
                root_permits.add_permits(1);
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(root.read(reply.payload).unwrap(), b"re: one");
                // --- back to 70; crossing again fires again.
                let corr = root.send(tool, b"two").unwrap();
                let low = root.recv(from_hook(5)).await.unwrap();
                note(&low, 5);
                root_permits.add_permits(1);
                root.recv(Match::Corr(corr)).await.unwrap();
                log("root crossed twice by sending");

                // --- OnSpawn allows; carving 40 from 70 is the third crossing.
                let child_permits = Arc::clone(&root_permits);
                let child = root
                    .spawn(
                        program(move |child| async move {
                            // PreSend: hook 0 tattles, hook 1 denies, hook 7
                            // is never asked.
                            let err = child.send(tool, b"shell rm -rf /").unwrap_err();
                            let KernelError::Denied { hook: by, reason } = err else {
                                panic!("expected Denied, got {err:?}");
                            };
                            assert_eq!(by, hook(1));
                            assert_eq!(reason, "shell needs depth 2");
                            // PreSend: hook 7 breaks; closed means denied.
                            let err = child.send(tool, b"flaky").unwrap_err();
                            let KernelError::Denied { hook: by, reason } = err else {
                                panic!("expected Denied, got {err:?}");
                            };
                            assert_eq!(by, hook(7));
                            assert_eq!(reason, "guard exploded");
                            // An allowed send; PreDeliver relays the reply.
                            let corr = child.send(tool, b"hello").unwrap();
                            child_permits.add_permits(1);
                            let reply = child.recv(Match::Corr(corr)).await.unwrap();
                            assert_eq!(child.read(reply.payload).unwrap(), b"re: hello");
                            // OnSpawn: a grandchild would be at depth 0.
                            let err = child
                                .spawn(
                                    program(|g| async move { g.exit(b"") }),
                                    Namespace::empty(),
                                    Budget::from_dims([(DimKey::Tokens, 1)]),
                                )
                                .unwrap_err();
                            assert!(
                                matches!(err, KernelError::Denied { hook, .. } if hook == HookId::new(3)),
                                "got {err:?}"
                            );
                            child.exit(b"child done")
                        }),
                        child_ns,
                        child_grant(),
                    )
                    .unwrap();
                let low = root.recv(from_hook(5)).await.unwrap();
                note(&low, 5);
                log("root crossed a third time by carving");

                // --- what the hooks told the parent about its child, in order.
                let tattle = root.recv(from_hook(0)).await.unwrap();
                note(&tattle, 0);
                assert_eq!(root.read(tattle.payload).unwrap(), b"child asked for a shell");
                let relay = root.recv(from_hook(2)).await.unwrap();
                note(&relay, 2);
                assert_eq!(root.read(relay.payload).unwrap(), b"child heard back");
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.agent, child);
                assert_eq!(root.read(done.result().unwrap()).unwrap(), b"child done");
                let obit = root.recv(from_hook(4)).await.unwrap();
                note(&obit, 4);
                assert_eq!(root.read(obit.payload).unwrap(), b"exited");
                log("root heard the tattle, the relay, and the obituary");
                root.exit(b"root done")
            }),
            ns,
            root_grant(),
        )
        .unwrap();

    kernel.drained().await.unwrap();
    kernel.shutdown();
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [
            "root crossed twice by sending",
            "root crossed a third time by carving",
            "root heard the tattle, the relay, and the obituary",
        ]
    );
    let Outcome::Exited(blob) = kernel.claim(root).unwrap() else {
        panic!("the root exited on its own");
    };
    assert_eq!(kernel.read(blob).unwrap(), b"root done");

    // --- the registry is at the head of the log, before any agent
    let entries = kernel.entries();
    let attached: Vec<HookId> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Attached { hook, .. } => Some(*hook),
            _ => None,
        })
        .collect();
    assert_eq!(attached, (0..8).map(hook).collect::<Vec<_>>());
    let first_agent = entries
        .iter()
        .position(|e| matches!(e, Entry::Spawned { .. }))
        .unwrap();
    // The driver, the eight hooks, and the root's own `OnSpawn` roll call —
    // the root included, ADR-0008 §1 — then the root.
    assert_eq!(first_agent, 1 + 8 + 1, "drivers, hooks, then the root");
    assert!(entries[1..=8]
        .iter()
        .all(|e| matches!(e, Entry::Attached { .. })));
    assert!(matches!(
        &entries[9],
        Entry::Verdicts { point: HookPoint::OnSpawn, subject, .. } if *subject == root
    ));

    // --- one roll call per moment; the denied send: tattle, deny, no send
    let rolls: Vec<Moment<'_>> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Verdicts {
                point,
                subject,
                roll,
                ..
            } => Some((point, *subject, roll.as_slice())),
            _ => None,
        })
        .collect();
    let child = AgentId::new(1);
    let denied = rolls
        .iter()
        .find(|(p, s, roll)| {
            **p == HookPoint::PreSend
                && *s == child
                && matches!(roll.first(), Some((_, Ruling::Emit { .. })))
        })
        .expect("the shell attempt's roll call");
    assert_eq!(denied.2.len(), 2, "hook 7 was never asked");
    assert!(matches!(denied.2[1], (h, Ruling::Deny(_)) if h == hook(1)));
    let at = entries
        .iter()
        .position(|e| matches!(e, Entry::Verdicts { subject, roll, .. } if *subject == child && roll.len() == 2))
        .unwrap();
    assert!(
        matches!(&entries[at + 1], Entry::Emitted { hook: h, to, .. } if *h == hook(0) && *to == root),
        "the tattle is delivered even though the send was denied"
    );
    assert!(
        !matches!(&entries[at + 2], Entry::Sent { .. }),
        "and the send never happened"
    );

    // --- the flaky guard: a closed failure is a deny, with the error
    let flaky = rolls
        .iter()
        .find(|(p, s, roll)| **p == HookPoint::PreSend && *s == child && roll.len() == 3)
        .expect("the flaky attempt's roll call");
    assert!(matches!(
        flaky.2[2],
        (h, Ruling::Failed { mode: FailureMode::Closed, .. }) if h == hook(7)
    ));

    // --- OnBudget: exactly three crossings, all the root's
    let crossings: Vec<AgentId> = rolls
        .iter()
        .filter(|(p, ..)| **p == low_tokens())
        .map(|(_, s, _)| *s)
        .collect();
    assert_eq!(crossings, vec![root, root, root]);

    // --- OnExit: the child's and the root's; grumpy's deny is a failure
    let exits: Vec<(AgentId, &[(HookId, Ruling)])> = rolls
        .iter()
        .filter(|(p, ..)| **p == HookPoint::OnExit)
        .map(|(_, s, r)| (*s, *r))
        .collect();
    assert_eq!(exits.len(), 2);
    assert_eq!(exits[0].0, child);
    assert_eq!(exits[1].0, root);
    for (_, roll) in &exits {
        assert!(matches!(roll[0], (h, Ruling::Emit { .. }) if h == hook(4)));
        assert!(matches!(
            roll[1],
            (h, Ruling::Failed { mode: FailureMode::Open, .. }) if h == hook(6)
        ));
    }
    // The root's obituary went to the root, which had just finished: the
    // last note in the log, just before the harness's claim.
    let last = entries
        .iter()
        .rev()
        .find(|e| matches!(e, Entry::Emitted { .. }))
        .unwrap();
    assert!(matches!(last, Entry::Emitted { hook: h, to, .. } if *h == hook(4) && *to == root));
    let state = kernel.state();
    assert_eq!(state.agent(root).unwrap().status, Status::Exited);
    assert!(state.agent(root).unwrap().mailbox.is_empty(), "dead letter");
    assert!(state.is_drained());

    // --- the obligation: a fold with none of the programs, same hash
    let live = kernel.state_hash();
    assert_eq!(fold(&entries).unwrap().hash(), live);
    let bytes = sink.contents();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(reread.entries(), entries.as_slice());
    let refold = fold(reread.entries()).unwrap();
    assert_eq!(refold.hash(), live);
    assert_eq!(refold.hooks().count(), 8);

    // Regenerate with `TAU_UPDATE_FIXTURES=1 cargo test -p tau-kernel --test
    // m2a_hooks`, and read the diff.
    if std::env::var_os("TAU_UPDATE_FIXTURES").is_some() {
        std::fs::write(FIXTURE, &bytes).unwrap();
    }
}

#[test]
fn the_fixture_refolds_to_the_pinned_state_hash() {
    // This binary has no `tattle-shell`, no `no-shell-below-depth-2`, none
    // of the eight. The fold does not need them: it confirms the roll calls
    // and applies the notes. If this ever needs a program, someone made
    // replay re-evaluate a hook, and that is the bug this test exists for.
    let bytes = include_bytes!("fixtures/m2a-hooks.log");
    let log = Log::read_from(&bytes[..]).unwrap();
    let state = fold(log.entries()).unwrap();
    assert!(state.is_drained());
    assert_eq!(state.hooks().count(), 8);
    insta::assert_snapshot!(state.hash().to_string());
}

#[tokio::test]
async fn attach_is_boot_only_and_closed_where_it_can_veto() {
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let allow = || native("allow", |_| Ok(Verdict::Allow));
    for point in [
        HookPoint::PreSend,
        HookPoint::PreDeliver,
        HookPoint::OnSpawn,
    ] {
        let err = kernel
            .attach(point.clone(), allow(), FailureMode::Open)
            .unwrap_err();
        assert!(
            matches!(err, KernelError::Refused(Refusal::OpenAtVetoPoint(ref p)) if *p == point),
            "got {err:?}"
        );
    }
    assert_eq!(
        kernel
            .attach(HookPoint::OnExit, allow(), FailureMode::Open)
            .unwrap(),
        hook(0)
    );
    kernel
        .spawn_root(
            program(|root| async move { root.exit(b"") }),
            Namespace::empty(),
            Budget::empty(),
        )
        .unwrap();
    let err = kernel
        .attach(HookPoint::OnExit, allow(), FailureMode::Closed)
        .unwrap_err();
    assert!(matches!(err, KernelError::Refused(Refusal::AfterBoot)));
    kernel.drained().await.unwrap();
    kernel.shutdown();
}

#[tokio::test]
async fn a_denied_reply_is_never_delivered_and_the_request_stays_open() {
    // PreDeliver is what the world says back. A deny there leaves the
    // roll call in the log and nothing in the mailbox; the correlation is
    // still open, so the owner waits until it is cancelled.
    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let permits = Arc::new(Semaphore::new(1));
    let tool = kernel
        .register_driver(
            tool_id(),
            Gated { permits },
            Budget::from_dims([(DimKey::Tokens, CEILING)]),
        )
        .unwrap();
    kernel
        .attach(
            HookPoint::PreDeliver,
            native("no-secrets", |e| {
                Ok(match e {
                    HookEvent::PreDeliver { payload, .. } if payload.ends_with(b"secret") => {
                        Verdict::Deny("no secret leaves any endpoint".into())
                    }
                    _ => Verdict::Allow,
                })
            }),
            FailureMode::Closed,
        )
        .unwrap();
    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let corr = root.send(tool, b"secret").unwrap();
                // The reply never comes; the cancel notice does.
                let msg = root
                    .recv(Match::Or(vec![
                        Match::Corr(corr),
                        Match::Kind(MsgKind::Notice),
                    ]))
                    .await
                    .unwrap();
                assert_eq!(msg.from, Endpoint::Harness);
                root.exit(b"")
            }),
            Namespace::from_caps([tool]),
            Budget::from_dims([(DimKey::Tokens, 100), (DimKey::Calls, 1)]),
        )
        .unwrap();
    // Wait for the deny to be logged, then cancel with grace.
    loop {
        let denied = kernel.entries().iter().any(|e| {
            matches!(e, Entry::Verdicts { point: HookPoint::PreDeliver, roll, .. }
                if matches!(roll.first(), Some((_, Ruling::Deny(_)))))
        });
        if denied {
            break;
        }
        tokio::task::yield_now().await;
    }
    let state = kernel.state();
    assert!(state.agent(root).unwrap().mailbox.is_empty());
    assert_eq!(state.open_corrs(root).count(), 1, "still open");
    assert!(!kernel
        .entries()
        .iter()
        .any(|e| matches!(e, Entry::Replied { .. })));
    kernel
        .cancel_from_harness(root, &CancelMode::grace(10))
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
}

#[tokio::test]
async fn a_rule_at_pre_send_denies_and_the_log_carries_its_source() {
    // The second tier (ADR-0008 §5): the same veto as hook 1 above, written
    // as one line, parsed at `attach`, its canonical text in the `Attached`
    // entry. A rule attached at a point its `when` does not name is
    // refused, not installed as a silent no-op.
    let sink = SharedBuf::default();
    let kernel = Kernel::boot(Log::with_sink(sink.clone()).unwrap(), tokio_spawner);
    let tool = kernel
        .register_driver(
            tool_id(),
            Gated {
                permits: Arc::new(Semaphore::new(1)),
            },
            Budget::from_dims([(DimKey::Tokens, CEILING)]),
        )
        .unwrap();
    let text = "when pre_send if driver == tool and payload contains \"rm -rf\" then deny \"no recursive deletes\"";
    let rule = Rule::parse(text).unwrap();
    let err = kernel
        .attach(
            HookPoint::OnSpawn,
            HookProgram::Rule(rule.clone()),
            FailureMode::Closed,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        KernelError::Refused(Refusal::RulePoint {
            rule: HookPoint::PreSend,
            attached: HookPoint::OnSpawn
        })
    ));
    let id = kernel
        .attach(
            HookPoint::PreSend,
            HookProgram::Rule(rule),
            FailureMode::Closed,
        )
        .unwrap();
    assert_eq!(id, hook(0));

    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let err = root.send(tool, b"shell rm -rf /").unwrap_err();
                let KernelError::Denied { hook: by, reason } = err else {
                    panic!("expected Denied, got {err:?}");
                };
                assert_eq!(by, hook(0));
                assert_eq!(reason, "no recursive deletes");
                let corr = root.send(tool, b"shell ls").unwrap();
                let reply = root.recv(Match::Corr(corr)).await.unwrap();
                assert_eq!(root.read(reply.payload).unwrap(), b"re: shell ls");
                root.exit(b"root done")
            }),
            Namespace::from_caps([tool]),
            root_grant(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let entries = kernel.entries();
    assert!(matches!(
        &entries[1],
        Entry::Attached { hook: h, point: HookPoint::PreSend, program: HookSource::Rule(src), .. }
            if *h == hook(0) && src == text
    ));
    let rolls: Vec<&[(HookId, Ruling)]> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Verdicts {
                point: HookPoint::PreSend,
                subject,
                roll,
                ..
            } if *subject == root => Some(roll.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(rolls.len(), 2, "one roll call per send attempt");
    assert!(matches!(rolls[0], [(h, Ruling::Deny(_))] if *h == hook(0)));
    assert!(matches!(rolls[1], [(h, Ruling::Allow)] if *h == hook(0)));
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Sent { .. }))
            .count(),
        1,
        "the denied send never happened"
    );

    // The fold confirms the roll call without parsing the rule.
    let live = kernel.state_hash();
    assert_eq!(fold(&entries).unwrap().hash(), live);
    let bytes = sink.contents();
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(fold(reread.entries()).unwrap().hash(), live);
}
