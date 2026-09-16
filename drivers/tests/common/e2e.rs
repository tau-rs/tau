//! One agent program over a cassette: the kernel boots, a model driver and
//! the calculator are registered, the real `libtau` tool loop runs to its
//! end, and what the loop caused to be sent is checked against the
//! recording, request for request.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use libtau::{prompt, tool_loop, ToolLoopError, Toolbox};
use serde_json::Value;
use tau_kernel::abi::{Budget, Consumption, DimKey, DriverId, Name, Namespace};
use tau_kernel::bridge::{ModelReply, ModelRequest};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::program as agent_program;

use super::calc;
use super::cassette::{self, Cassette};
use super::Captured;

/// A model driver and the ceiling it was built with, at a base URL.
pub(crate) type MakeModel = Box<dyn Fn(&str) -> (Arc<dyn Driver>, Budget) + Send + Sync>;

/// `register_driver` takes a concrete `Driver`; this is the one for a
/// driver already behind an `Arc<dyn Driver>`.
struct Shared(Arc<dyn Driver>);

impl Driver for Shared {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        self.0.handle(request)
    }
    fn describe(&self) -> Option<ToolSchema> {
        self.0.describe()
    }
}

/// The calculator as a tool driver: answers from [`calc::answer`] and
/// keeps every input it was sent.
#[derive(Clone, Default)]
pub(crate) struct Calculator {
    pub(crate) seen: Arc<Mutex<Vec<Value>>>,
}

/// What the calculator bills per call, and its registration ceiling.
pub(crate) const CALC_TOKENS: u64 = 1;

impl Driver for Calculator {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let input: Value = serde_json::from_slice(&request.payload).unwrap();
        self.seen.lock().unwrap().push(input.clone());
        Box::pin(async move {
            (
                calc::answer(&input).into_bytes(),
                Consumption::from_dims([(DimKey::Tokens, CALC_TOKENS)]),
            )
        })
    }
    fn describe(&self) -> Option<ToolSchema> {
        Some(calc::tool_schema())
    }
}

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

/// What the program asks, before the loop adds the toolbox.
pub(crate) struct Program {
    pub(crate) user: &'static str,
    pub(crate) max_tokens: u32,
    /// The root grant, given the model driver's ceiling.
    pub(crate) budget: fn(&Budget) -> Budget,
}

/// The default root grant: room for many calls at any of the matrix's
/// ceilings.
pub(crate) fn ample(_ceiling: &Budget) -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, 1_000_000),
        (DimKey::CostMicroUsd, 10_000_000),
        (DimKey::Calls, 16),
    ])
}

/// What one run left behind.
pub(crate) struct Run {
    pub(crate) cassette: Cassette,
    /// The loop's answer.
    pub(crate) result: Result<ModelReply, ToolLoopError>,
    /// The request after the loop: the full transcript.
    pub(crate) transcript: ModelRequest,
    /// Every input the calculator was sent, in order.
    pub(crate) calls: Vec<Value>,
    /// Every request the stub received, in order.
    pub(crate) sent: Vec<Captured>,
    /// The root agent's settled spend, by dimension.
    pub(crate) spent: BTreeMap<DimKey, u64>,
}

/// Boots a kernel, registers the model (scripted from the cassette) and the
/// calculator, runs `program` through the tool loop, and drains.
pub(crate) async fn run(dir_name: &str, name: &str, make: MakeModel, program: Program) -> Run {
    let cassette = cassette::load(dir_name, name)
        .unwrap_or_else(|| panic!("{dir_name}/{name}: no cassette; run `just live record`"));
    let answers = cassette
        .exchanges
        .iter()
        .map(|e| (e.response.status, e.response.body.to_string()));
    let mut stub = super::start(super::script(answers)).await;
    let (model, ceiling) = make(&stub.base_url);
    let calculator = Calculator::default();

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let model_cap = kernel
        .register_driver(
            DriverId::new(Name::new("model").unwrap()),
            Shared(model),
            ceiling.clone(),
        )
        .unwrap();
    let calc_cap = kernel
        .register_driver(
            DriverId::new(Name::new(calc::NAME).unwrap()),
            calculator.clone(),
            Budget::from_dims([(DimKey::Tokens, CALC_TOKENS)]),
        )
        .unwrap();
    let ns = Namespace::from_caps([model_cap, calc_cap]);

    type Outcome = (ModelRequest, Result<ModelReply, ToolLoopError>);
    let outcome: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let sink = Arc::clone(&outcome);
    let (user, max_tokens) = (program.user, program.max_tokens);
    let root = kernel
        .spawn_root(
            agent_program(move |h| async move {
                let toolbox = Toolbox::project(&h, &[calc_cap]).expect("the calculator is a tool");
                let mut request = prompt(user, max_tokens);
                request.system = Some("You are terse.".into());
                let result =
                    tool_loop(&h, model_cap, &toolbox, &mut request, &mut Vec::new()).await;
                sink.lock().unwrap().replace((request, result));
                h.exit(b"")
            }),
            ns,
            (program.budget)(&ceiling),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    let (transcript, result) = outcome.lock().unwrap().take().expect("the program ran");
    let mut sent = Vec::new();
    while let Ok(captured) = stub.captured.try_recv() {
        sent.push(captured);
    }
    let spent = kernel.state().agent(root).unwrap().spent.clone();
    let calls = calculator.seen.lock().unwrap().clone();
    Run {
        cassette,
        result,
        transcript,
        calls,
        sent,
        spent,
    }
}

impl Run {
    /// Every request the loop caused is the recorded one, in order, and
    /// there are exactly `count` of them.
    pub(crate) fn assert_sent_matches_recording(&self, count: usize) {
        assert_eq!(self.sent.len(), count, "requests on the wire");
        for (i, (sent, ex)) in self.sent.iter().zip(&self.cassette.exchanges).enumerate() {
            assert_eq!(sent.path(), ex.request.path, "exchange #{i} path");
            assert_eq!(
                sent.json(),
                ex.request.body,
                "exchange #{i} body differs from the recording"
            );
        }
    }

    /// The reply that ended the loop, or a panic with the error.
    pub(crate) fn reply(&self) -> &ModelReply {
        match &self.result {
            Ok(reply) => reply,
            Err(e) => panic!("the loop did not end on a reply: {e:?}"),
        }
    }
}
