//! `infer`: one model call over `send`, `recv`, `read`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    end_turn, error_reply, id, plenty, provider, recording_sleep, refusal, slot, take,
    tokio_spawner, transport, Waits, World, MODEL_CEILING,
};
use libtau::{infer, infer_with, prompt, should_retry, InferError, RetryPolicy};
use tau_kernel::abi::{Budget, Consumption, Corr, DimKey, Namespace};
use tau_kernel::bridge::{ErrorKind, ModelError, ModelReply, StopReason};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{BoxFuture, Delivery, Kernel, KernelError};
use tau_kernel::log::Log;
use tau_kernel::reducer::{Outcome, Refusal};
use tau_kernel::syscall::{program, CancelMode, WaitFor};
use tokio::sync::Notify;

#[tokio::test]
async fn a_request_reaches_the_driver_and_its_reply_comes_back_decoded() {
    let world = World::boot([end_turn("hi there")]);
    let out = slot();
    let sink = Arc::clone(&out);
    let model = world.model_cap;
    world
        .run(
            program(move |h| async move {
                let request = prompt("hello", 64);
                let result = infer(&h, model, &request, &mut Vec::new()).await;
                sink.lock().unwrap().replace((request, result));
                h.exit(b"")
            }),
            world.all_caps(),
            plenty(),
        )
        .await;

    let (request, result) = take(&out);
    let reply = result.unwrap();
    assert_eq!(reply, end_turn("hi there"));
    assert_eq!(
        world.model.seen(),
        vec![request],
        "the driver received exactly the request, as bridge JSON"
    );
}

#[tokio::test]
async fn a_send_refused_on_budget_is_terminal_and_nothing_is_sent() {
    let world = World::boot([end_turn("never")]);
    let out = slot();
    let sink = Arc::clone(&out);
    let model = world.model_cap;
    // Less than the model's ceiling: the reservation cannot be made.
    let starved = Budget::from_dims([(DimKey::Tokens, MODEL_CEILING - 1), (DimKey::Calls, 5)]);
    world
        .run(
            program(move |h| async move {
                let result = infer(&h, model, &prompt("hello", 64), &mut Vec::new()).await;
                sink.lock().unwrap().replace(result);
                h.exit(b"")
            }),
            world.all_caps(),
            starved,
        )
        .await;

    match take(&out) {
        Err(InferError::Send(KernelError::Refused(Refusal::Budget(_)))) => {}
        other => panic!("expected a budget refusal, got {other:?}"),
    }
    assert!(world.model.seen().is_empty(), "no retry, no delivery");
}

#[tokio::test]
async fn a_capability_not_held_is_a_terminal_send_error() {
    let world = World::boot([end_turn("never")]);
    let out = slot();
    let sink = Arc::clone(&out);
    let model = world.model_cap;
    world
        .run(
            program(move |h| async move {
                let result = infer(&h, model, &prompt("hello", 64), &mut Vec::new()).await;
                sink.lock().unwrap().replace(result);
                h.exit(b"")
            }),
            Namespace::from_caps([world.store_cap]),
            plenty(),
        )
        .await;

    match take(&out) {
        Err(InferError::Send(KernelError::Refused(Refusal::NotHeld { .. }))) => {}
        other => panic!("expected NotHeld, got {other:?}"),
    }
}

/// A model that never answers.
#[derive(Clone, Default)]
struct Silent;

impl Driver for Silent {
    fn handle(&self, _request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        Box::pin(std::future::pending())
    }

    fn abandon(&self, _corr: Corr) {}
}

#[tokio::test]
async fn a_cancel_while_waiting_ends_the_call_with_the_reason() {
    let kernel = Kernel::boot(Log::with_sink(Vec::new()).unwrap(), tokio_spawner);
    let model = kernel
        .register_driver(
            id("model"),
            Silent,
            Budget::from_dims([(DimKey::Tokens, MODEL_CEILING)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([model]);
    let out = slot();
    let sink = Arc::clone(&out);
    let sent = Arc::new(Notify::new());
    let sent_child = Arc::clone(&sent);
    let child_ns = ns.clone();

    let root = kernel
        .spawn_root(
            program(move |root| async move {
                let child = root
                    .spawn(
                        program(move |child| async move {
                            // Signal before the call so the parent's cancel
                            // lands while `infer` is inside `recv`. The
                            // notify is plain memory, not a guard.
                            sent_child.notify_one();
                            let result =
                                infer(&child, model, &prompt("hello", 64), &mut Vec::new()).await;
                            sink.lock().unwrap().replace(result);
                            child.exit(b"stopped in time")
                        }),
                        child_ns,
                        Budget::from_dims([(DimKey::Tokens, 5_000), (DimKey::Calls, 2)]),
                    )
                    .unwrap();
                sent.notified().await;
                // Let the child reach its `recv` before the notice is sent.
                tokio::task::yield_now().await;
                root.cancel(child, CancelMode::grace(10).with_reason(b"enough"))
                    .unwrap();
                let done = root.wait(WaitFor::Child(child)).await.unwrap();
                assert_eq!(done.agent, child);
                let Outcome::Exited(blob) = done.outcome else {
                    panic!("the child exited on its own within the grace period");
                };
                let bytes = root.read(blob).unwrap();
                root.exit(&bytes)
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 10_000),
                (DimKey::Calls, 10),
                (DimKey::Depth, 1),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let Outcome::Exited(blob) = kernel.claim(root).unwrap() else {
        panic!("root exited");
    };
    assert_eq!(kernel.read(blob).unwrap(), b"stopped in time");
    match take(&out) {
        Err(InferError::Cancelled { reason }) => assert_eq!(reason, b"enough"),
        other => panic!("expected Cancelled, got {other:?}"),
    }
}

// --- retries ----------------------------------------------------------------

/// Runs `infer_with` under `policy` as the root, recording every wait.
async fn run_retrying(
    world: &World,
    budget: Budget,
    policy: RetryPolicy,
) -> (Result<ModelReply, InferError>, Vec<Duration>) {
    let out = slot();
    let sink = Arc::clone(&out);
    let waits: Waits = Arc::default();
    let sleep = recording_sleep(&waits);
    let model = world.model_cap;
    world
        .run(
            program(move |h| async move {
                let result = infer_with(
                    &h,
                    model,
                    &prompt("hello", 64),
                    &mut Vec::new(),
                    &policy,
                    sleep,
                )
                .await;
                sink.lock().unwrap().replace(result);
                h.exit(b"")
            }),
            world.all_caps(),
            budget,
        )
        .await;
    let waits = waits.lock().unwrap().clone();
    (take(&out), waits)
}

/// Three retries, doubling from 100 ms, capped at 250 ms.
fn three() -> RetryPolicy {
    RetryPolicy {
        retries: 3,
        backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(250),
    }
}

#[tokio::test]
async fn a_transport_error_is_retried_and_every_attempt_is_a_send() {
    let world = World::boot([transport(), end_turn("second time lucky")]);
    let (result, waits) = run_retrying(&world, plenty(), three()).await;
    assert_eq!(result.unwrap(), end_turn("second time lucky"));
    assert_eq!(world.model.seen().len(), 2, "two sends, one per attempt");
    assert_eq!(waits, vec![Duration::from_millis(100)]);
}

#[tokio::test]
async fn a_429_twice_is_retried_with_a_doubling_wait() {
    let world = World::boot([provider(429), provider(429), end_turn("ok")]);
    let (result, waits) = run_retrying(&world, plenty(), three()).await;
    assert_eq!(result.unwrap(), end_turn("ok"));
    assert_eq!(world.model.seen().len(), 3);
    assert_eq!(
        waits,
        vec![Duration::from_millis(100), Duration::from_millis(200)]
    );
}

#[tokio::test]
async fn the_wait_is_capped_at_max_backoff() {
    let world = World::boot([transport(), transport(), transport(), end_turn("ok")]);
    let (result, waits) = run_retrying(&world, plenty(), three()).await;
    assert_eq!(result.unwrap(), end_turn("ok"));
    assert_eq!(
        waits,
        vec![
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(250),
        ]
    );
}

#[tokio::test]
async fn every_5xx_is_retried() {
    for status in [500, 502, 503, 529] {
        let world = World::boot([provider(status), end_turn("ok")]);
        let (result, waits) = run_retrying(&world, plenty(), three()).await;
        assert_eq!(result.unwrap(), end_turn("ok"), "HTTP {status}");
        assert_eq!(world.model.seen().len(), 2, "HTTP {status}");
        assert_eq!(waits.len(), 1, "HTTP {status}");
    }
}

#[tokio::test]
async fn a_non_retryable_error_is_returned_after_one_send() {
    let cases = [
        provider(400),
        provider(401),
        provider(404),
        error_reply(ErrorKind::Provider, "HTTP 200 body is not JSON: eof"),
        error_reply(ErrorKind::Provider, "no status at all"),
        error_reply(ErrorKind::OverCeiling, "input estimated at 9210 tokens"),
        error_reply(ErrorKind::Unsupported, "seed"),
    ];
    for expected in cases {
        let world = World::boot([expected.clone(), end_turn("never")]);
        let (result, waits) = run_retrying(&world, plenty(), three()).await;
        assert_eq!(result.unwrap(), expected);
        assert_eq!(world.model.seen().len(), 1, "one send for {expected:?}");
        assert!(waits.is_empty(), "no wait for {expected:?}");
    }
}

#[tokio::test]
async fn a_refusal_is_a_stop_reason_and_is_never_retried() {
    let world = World::boot([refusal(), end_turn("never")]);
    let (result, waits) = run_retrying(&world, plenty(), three()).await;
    assert_eq!(result.unwrap(), refusal());
    assert_eq!(world.model.seen().len(), 1);
    assert!(waits.is_empty());
}

#[tokio::test]
async fn when_retries_run_out_the_last_error_reply_is_returned() {
    let world = World::boot([
        transport(),
        provider(529),
        transport(),
        provider(503),
        end_turn("never"),
    ]);
    let (result, waits) = run_retrying(&world, plenty(), three()).await;
    assert_eq!(result.unwrap(), provider(503), "the last attempt's reply");
    assert_eq!(world.model.seen().len(), 4, "one plus three retries");
    assert_eq!(waits.len(), 3);
}

#[tokio::test]
async fn a_budget_refusal_mid_retry_ends_the_retries() {
    let world = World::boot([transport(), end_turn("never")]);
    // One call's worth: the first attempt reserves it, the retry cannot.
    let one_call = Budget::from_dims([(DimKey::Tokens, 100_000), (DimKey::Calls, 1)]);
    let (result, waits) = run_retrying(&world, one_call, three()).await;
    match result {
        Err(InferError::Send(KernelError::Refused(Refusal::Budget(_)))) => {}
        other => panic!("expected a budget refusal, got {other:?}"),
    }
    assert_eq!(
        world.model.seen().len(),
        1,
        "the refused attempt was never sent"
    );
    assert_eq!(
        waits,
        vec![Duration::from_millis(100)],
        "the wait happened; the send after it was refused"
    );
}

#[tokio::test]
async fn the_default_policy_is_no_retries_and_infer_never_waits() {
    let world = World::boot([transport(), end_turn("never")]);
    let (result, waits) = run_retrying(&world, plenty(), RetryPolicy::default()).await;
    assert_eq!(result.unwrap(), transport());
    assert_eq!(world.model.seen().len(), 1);
    assert!(waits.is_empty());
    assert_eq!(RetryPolicy::default(), RetryPolicy::NONE);
}

#[test]
fn the_schedule_doubles_and_saturates() {
    let policy = RetryPolicy {
        retries: 100,
        backoff: Duration::from_secs(1),
        max_backoff: Duration::MAX,
    };
    assert_eq!(policy.wait(0), Duration::from_secs(1));
    assert_eq!(policy.wait(1), Duration::from_secs(2));
    assert_eq!(policy.wait(10), Duration::from_secs(1024));
    assert_eq!(policy.wait(99), Duration::MAX, "saturates, never overflows");
    let capped = RetryPolicy::transient(3);
    assert_eq!(capped.retries, 3);
    assert_eq!(capped.wait(0), Duration::from_millis(500));
    assert_eq!(
        capped.wait(4),
        Duration::from_secs(8),
        "capped at max_backoff"
    );
    assert_eq!(RetryPolicy::NONE.wait(0), Duration::ZERO);
}

#[test]
fn the_decision_table_reads_the_status_out_of_the_message() {
    let err = |kind, msg: &str| {
        StopReason::Error(ModelError {
            kind,
            message: msg.into(),
        })
    };
    assert!(should_retry(&err(ErrorKind::Transport, "anything")));
    assert!(should_retry(&err(
        ErrorKind::Provider,
        "HTTP 429 rate_limit_error: slow"
    )));
    assert!(should_retry(&err(
        ErrorKind::Provider,
        "HTTP 529 overloaded_error: busy"
    )));
    assert!(should_retry(&err(
        ErrorKind::Provider,
        "HTTP 502: <html>bad gateway</html>"
    )));
    assert!(!should_retry(&err(
        ErrorKind::Provider,
        "HTTP 400 invalid_request_error: x"
    )));
    assert!(!should_retry(&err(
        ErrorKind::Provider,
        "HTTP 200 body could not be mapped: x"
    )));
    assert!(!should_retry(&err(ErrorKind::Provider, "HTTP: no digits")));
    assert!(!should_retry(&err(
        ErrorKind::Provider,
        "HTTP 99999 out of range"
    )));
    assert!(!should_retry(&err(ErrorKind::Provider, "")));
    assert!(!should_retry(&err(
        ErrorKind::OverCeiling,
        "HTTP 503 would be a lie"
    )));
    assert!(!should_retry(&err(
        ErrorKind::Unsupported,
        "HTTP 503 would be a lie"
    )));
    assert!(!should_retry(&StopReason::Refusal));
    assert!(!should_retry(&StopReason::EndTurn));
    assert!(!should_retry(&StopReason::ToolCall));
    assert!(!should_retry(&StopReason::MaxTokens));
    assert!(!should_retry(&StopReason::StopSequence));
}
