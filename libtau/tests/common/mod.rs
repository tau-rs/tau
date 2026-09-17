//! The world the loop tests run in: a kernel booted with a scripted model,
//! a store driver that is a tool, and the echo driver. No network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tau_kernel::abi::{Budget, Capability, Consumption, DimKey, DriverId, Name, Namespace};
use tau_kernel::bridge::{
    Content, ErrorKind, ModelError, ModelReply, ModelRequest, StopReason, Usage, VERSION,
};
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::Program;

pub(crate) fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

pub(crate) fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}

pub(crate) fn id(s: &str) -> DriverId {
    DriverId::new(name(s))
}

/// The most one model call may cost, as the harness declares it.
pub(crate) const MODEL_CEILING: u64 = 1_000;
/// The most one tool call may cost.
pub(crate) const TOOL_CEILING: u64 = 64;

/// A root budget that comfortably covers a handful of calls.
pub(crate) fn plenty() -> Budget {
    Budget::from_dims([(DimKey::Tokens, 100_000), (DimKey::Calls, 100)])
}

/// A model driver that answers from a script and remembers what it was
/// asked. `describe()` takes the default: a model is not a tool.
#[derive(Clone, Default)]
pub(crate) struct ScriptedModel {
    replies: Arc<Mutex<VecDeque<ModelReply>>>,
    seen: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ScriptedModel {
    pub(crate) fn new(replies: impl IntoIterator<Item = ModelReply>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into_iter().collect())),
            seen: Arc::default(),
        }
    }

    /// Every request received so far, decoded, in order.
    pub(crate) fn seen(&self) -> Vec<ModelRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Driver for ScriptedModel {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let decoded: ModelRequest =
            serde_json::from_slice(&request.payload).expect("the loop sends bridge JSON");
        self.seen.lock().unwrap().push(decoded);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script ran out of replies");
        let tokens = reply.usage.input_tokens + reply.usage.output_tokens;
        let bytes = serde_json::to_vec(&reply).unwrap();
        Box::pin(async move { (bytes, Consumption::from_dims([(DimKey::Tokens, tokens)])) })
    }
}

/// The store schema from ADR-0006 §2 / `fixtures/bridge/request.json`.
pub(crate) fn store_schema() -> Value {
    json!({
        "type": "object",
        "oneOf": [
            { "properties": { "op": { "const": "read" }, "key": { "type": "string" } }, "required": ["op", "key"] },
            { "properties": { "op": { "const": "write" }, "key": { "type": "string" }, "value": { "type": "string" } }, "required": ["op", "key", "value"] }
        ]
    })
}

pub(crate) const STORE_DESCRIPTION: &str = "Read or write the run's key-value store.";

/// A driver that is a tool. Answers `read` with `hello` (the fixture's
/// answer) and `write` with `ok`; remembers every payload it was sent.
#[derive(Clone, Default)]
pub(crate) struct StoreDriver {
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl StoreDriver {
    pub(crate) fn seen(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }
}

impl Driver for StoreDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        self.seen.lock().unwrap().push(request.payload.clone());
        let input: Value = serde_json::from_slice(&request.payload).unwrap_or(Value::Null);
        let answer: &[u8] = match input.get("op").and_then(Value::as_str) {
            Some("read") => b"hello",
            Some("write") => b"ok",
            _ => b"store: unreadable request",
        };
        let bytes = answer.to_vec();
        Box::pin(async move { (bytes, Consumption::from_dims([(DimKey::Tokens, 1)])) })
    }

    fn describe(&self) -> Option<ToolSchema> {
        Some(ToolSchema {
            description: STORE_DESCRIPTION.into(),
            input_schema: serde_json::to_vec(&store_schema()).unwrap(),
        })
    }
}

/// A driver that claims to be a tool but whose schema bytes are not JSON.
pub(crate) struct BrokenSchemaDriver;

impl Driver for BrokenSchemaDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        Box::pin(async move { (request.payload, Consumption::none()) })
    }

    fn describe(&self) -> Option<ToolSchema> {
        Some(ToolSchema {
            description: "not a schema".into(),
            input_schema: b"{not json".to_vec(),
        })
    }
}

/// A booted kernel and the capabilities it minted.
pub(crate) struct World {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) model: ScriptedModel,
    pub(crate) model_cap: Capability,
    pub(crate) store: StoreDriver,
    pub(crate) store_cap: Capability,
    pub(crate) echo_cap: Capability,
}

impl World {
    pub(crate) fn boot(replies: impl IntoIterator<Item = ModelReply>) -> Self {
        let kernel = Kernel::boot(Log::with_sink(Vec::new()).unwrap(), tokio_spawner);
        let model = ScriptedModel::new(replies);
        let model_cap = kernel
            .register_driver(
                id("model"),
                model.clone(),
                Budget::from_dims([(DimKey::Tokens, MODEL_CEILING)]),
            )
            .unwrap();
        let store = StoreDriver::default();
        let store_cap = kernel
            .register_driver(
                id("store"),
                store.clone(),
                Budget::from_dims([(DimKey::Tokens, TOOL_CEILING)]),
            )
            .unwrap();
        let echo_cap = kernel
            .register_driver(
                id("echo"),
                EchoDriver::new(),
                Budget::from_dims([(DimKey::Tokens, TOOL_CEILING)]),
            )
            .unwrap();
        Self {
            kernel,
            model,
            model_cap,
            store,
            store_cap,
            echo_cap,
        }
    }

    /// Everything the world minted.
    pub(crate) fn all_caps(&self) -> Namespace {
        Namespace::from_caps([self.model_cap, self.store_cap, self.echo_cap])
    }

    /// Runs `program` as the root with `ns` and `budget`, to completion.
    pub(crate) async fn run(&self, program: Program, ns: Namespace, budget: Budget) {
        self.kernel.spawn_root(program, ns, budget).unwrap();
        self.kernel.drained().await.unwrap();
        self.kernel.shutdown();
    }
}

/// A place for a program to leave its result for the test to inspect.
pub(crate) type Slot<T> = Arc<Mutex<Option<T>>>;

pub(crate) fn slot<T>() -> Slot<T> {
    Arc::default()
}

pub(crate) fn take<T>(slot: &Slot<T>) -> T {
    slot.lock()
        .unwrap()
        .take()
        .expect("the program left a result")
}

// --- reply builders -------------------------------------------------------

pub(crate) fn reply(content: Vec<Content>, stop: StopReason) -> ModelReply {
    ModelReply {
        v: VERSION,
        model: Some("scripted".into()),
        content,
        stop,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
        },
    }
}

pub(crate) fn end_turn(text: &str) -> ModelReply {
    reply(
        vec![Content::Text { text: text.into() }],
        StopReason::EndTurn,
    )
}

pub(crate) fn text(s: &str) -> Content {
    Content::Text { text: s.into() }
}

pub(crate) fn tool_call(id: &str, name: &str, input: Value) -> Content {
    Content::ToolCall {
        id: id.into(),
        name: name.into(),
        input,
    }
}

pub(crate) fn calls(blocks: Vec<Content>) -> ModelReply {
    reply(blocks, StopReason::ToolCall)
}

// --- error replies and the recorded wait ----------------------------------

/// A reply whose stop is a driver-side error.
pub(crate) fn error_reply(kind: ErrorKind, message: &str) -> ModelReply {
    reply(
        Vec::new(),
        StopReason::Error(ModelError {
            kind,
            message: message.into(),
        }),
    )
}

/// The driver's `transport` reply for a timeout, as the Anthropic driver
/// words it.
pub(crate) fn transport() -> ModelReply {
    error_reply(ErrorKind::Transport, "no answer within 600s")
}

/// The driver's `provider` reply for an HTTP status, as the Anthropic
/// driver words it.
pub(crate) fn provider(status: u16) -> ModelReply {
    error_reply(
        ErrorKind::Provider,
        &format!("HTTP {status} some_error: text"),
    )
}

/// A model-side decline: a stop reason, not an error.
pub(crate) fn refusal() -> ModelReply {
    reply(Vec::new(), StopReason::Refusal)
}

/// The waits a retry policy asked for, in order.
pub(crate) type Waits = Arc<Mutex<Vec<Duration>>>;

/// A sleep that returns at once and records what it was asked to wait.
/// The policy's schedule is asserted without wall time.
pub(crate) fn recording_sleep(
    waits: &Waits,
) -> impl FnMut(Duration) -> std::future::Ready<()> + Send + 'static {
    let waits = Arc::clone(waits);
    move |d| {
        waits.lock().unwrap().push(d);
        std::future::ready(())
    }
}

// --- a driver that raises (ADR-0014) ----------------------------------------

/// What a panicking test driver says, so the hook below can tell it from a
/// failed assertion.
pub(crate) const RAISED: &str = "the driver raised instead of reporting";

/// Silences the driver panics these tests cause on purpose; every other
/// panic still reports through the default hook.
pub(crate) fn quiet_panics() {
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

/// A driver whose `handle` unwinds on every request. As a tool it presents
/// the store's schema, so the loop projects and validates it like the store.
#[derive(Clone, Copy, Default)]
pub(crate) struct Boom {
    pub(crate) tool: bool,
}

impl Driver for Boom {
    fn handle(&self, _request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        panic!("{RAISED}");
    }

    fn describe(&self) -> Option<ToolSchema> {
        self.tool.then(|| ToolSchema {
            description: STORE_DESCRIPTION.into(),
            input_schema: serde_json::to_vec(&store_schema()).unwrap(),
        })
    }
}
