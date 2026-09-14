//! `Handle::describe`: the read that lets a tool loop project its namespace
//! (ADR-0006 §5). Authority is checked, bytes are forwarded, nothing is
//! logged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use tau_kernel::abi::{Budget, Capability, Consumption, DimKey, DriverId, Name, Namespace};
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel, KernelError};
use tau_kernel::log::Log;
use tau_kernel::reducer::Refusal;
use tau_kernel::syscall::program;

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

fn id(s: &str) -> DriverId {
    DriverId::new(Name::new(s).unwrap())
}

fn tokens(n: u64) -> Budget {
    Budget::from_dims([(DimKey::Tokens, n)])
}

const SCHEMA: &[u8] =
    br#"{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}"#;

/// A driver that is a tool. Its schema bytes are deliberately not something
/// the kernel could make sense of as anything but bytes.
struct StoreDriver;

impl Driver for StoreDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        Box::pin(async move { (request.payload, Consumption::none()) })
    }

    fn describe(&self) -> Option<ToolSchema> {
        Some(ToolSchema {
            description: "Read the run's key-value store.".into(),
            input_schema: SCHEMA.to_vec(),
        })
    }
}

type Seen = Arc<Mutex<Vec<Result<Option<(DriverId, ToolSchema)>, KernelError>>>>;

/// Boots a kernel with a store driver and an echo driver, spawns a root that
/// holds only `held`, and records what `describe` says for each of `asked`.
async fn describe_from_root(
    held: impl Fn(Capability, Capability) -> Vec<Capability>,
    asked: impl Fn(Capability, Capability) -> Vec<Capability> + Send + 'static,
) -> (
    Vec<Result<Option<(DriverId, ToolSchema)>, KernelError>>,
    usize,
) {
    let kernel = Kernel::boot(Log::with_sink(Vec::new()).unwrap(), tokio_spawner);
    let store = kernel
        .register_driver(id("store"), StoreDriver, tokens(8))
        .unwrap();
    let echo = kernel
        .register_driver(id("echo"), EchoDriver::new(), tokens(8))
        .unwrap();
    let entries_before = kernel.entries().len();

    let seen: Seen = Arc::default();
    let sink = Arc::clone(&seen);
    let root = kernel
        .spawn_root(
            program(move |h| async move {
                for cap in asked(store, echo) {
                    sink.lock().unwrap().push(h.describe(cap));
                }
                h.exit(b"")
            }),
            Namespace::from_caps(held(store, echo)),
            tokens(64),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    // One `Spawned`, one `Exited`: `describe` added nothing. Measured before
    // the claim, which is a log entry of its own.
    let entries_added = kernel.entries().len() - entries_before;
    kernel.claim(root).unwrap();
    let results = std::mem::take(&mut *seen.lock().unwrap());
    (results, entries_added)
}

#[tokio::test]
async fn a_tool_driver_is_described_under_the_name_the_harness_gave_it() {
    let (results, added) = describe_from_root(|s, e| vec![s, e], |s, _| vec![s]).await;
    let described = results.into_iter().next().unwrap().unwrap();
    let (name, schema) = described.expect("the store driver is a tool");
    assert_eq!(name, id("store"));
    assert_eq!(schema.description, "Read the run's key-value store.");
    assert_eq!(schema.input_schema, SCHEMA);
    assert_eq!(
        added, 2,
        "describe is a read: only Spawned and Exited were logged"
    );
}

#[tokio::test]
async fn a_driver_that_is_not_a_tool_describes_as_none() {
    let (results, _) = describe_from_root(|s, e| vec![s, e], |_, e| vec![e]).await;
    let described = results.into_iter().next().unwrap().unwrap();
    assert_eq!(
        described, None,
        "the echo driver takes the default: not a tool"
    );
}

#[tokio::test]
async fn describe_is_refused_for_a_capability_the_agent_does_not_hold() {
    // Root holds echo only, asks about store: authority stays in the kernel,
    // and a child cannot learn about tools outside its namespace by asking.
    let (results, added) = describe_from_root(|_, e| vec![e], |s, _| vec![s]).await;
    match results.into_iter().next().unwrap() {
        Err(KernelError::Refused(Refusal::NotHeld { .. })) => {}
        other => panic!("expected NotHeld, got {other:?}"),
    }
    assert_eq!(added, 2);
}
