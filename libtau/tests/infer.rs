//! `infer`: one model call over `send`, `recv`, `read`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::{end_turn, id, plenty, slot, take, tokio_spawner, World, MODEL_CEILING};
use libtau::{infer, prompt, InferError};
use tau_kernel::abi::{Budget, Consumption, Corr, DimKey, Namespace};
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
                let result = infer(&h, model, &request).await;
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
                let result = infer(&h, model, &prompt("hello", 64)).await;
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
                let result = infer(&h, model, &prompt("hello", 64)).await;
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
                            let result = infer(&child, model, &prompt("hello", 64)).await;
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
