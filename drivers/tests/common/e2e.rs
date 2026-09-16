//! One agent program over a model driver: the kernel boots, the driver and
//! the calculator are registered, the real `libtau` tool loop runs to its
//! end, and what the loop caused to be sent comes back with the run. Two
//! sources for the provider's side: [`run`] replays a cassette through the
//! stub, [`live`] relays to the real provider through the forwarding stub.

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
use super::cassette::{self, Exchange, RecordedResponse};
use super::scenario::Target;
use super::{Captured, Stub};

/// A model driver and the ceiling it was built with, at a base URL.
pub(crate) type MakeModel = Box<dyn Fn(&str) -> (Arc<dyn Driver>, Budget) + Send + Sync>;

pub(crate) const HAIKU: &str = "claude-haiku-4-5-20251001";
pub(crate) const OPUS: &str = "claude-opus-5";
pub(crate) const MINI: &str = "gpt-4.1-mini";
pub(crate) const QWEN: &str = "qwen3:1.7b";

pub(crate) const CALC_PROMPT: &str = "What is 17*23? Use the calculator tool.";
pub(crate) const PARALLEL_PROMPT: &str =
    "Compute 2+2 and 3+3 as two separate calculator calls in one turn.";
pub(crate) const THINKING_PROMPT: &str = "Work out the sum of the first 12 prime numbers step by step, then verify your total by calling the calculator tool once with the full addition expression.";

/// The Anthropic driver in the config the cassettes were recorded with
/// (`cassettes_anthropic.rs`): Opus at 5/25, everything else at 1/5 µUSD
/// per token.
#[cfg(feature = "anthropic")]
pub(crate) fn anthropic(
    model: &'static str,
    key: tau_drivers::model::anthropic::ApiKey,
) -> MakeModel {
    use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver};
    Box::new(move |base: &str| {
        let (i, o) = if model.starts_with("claude-opus") {
            (5, 25)
        } else {
            (1, 5)
        };
        let mut cfg = AnthropicConfig::new(model, key.clone(), 16_000, 4_096, i, o);
        cfg.base_url = base.to_owned();
        let driver = AnthropicDriver::new(cfg).unwrap();
        let ceiling = driver.ceiling();
        (Arc::new(driver) as Arc<dyn Driver>, ceiling)
    })
}

/// The OpenAI driver in the config the cassettes were recorded with
/// (`cassettes_openai.rs`), 1/4 µUSD per token; Ollama is the same driver
/// with no key and no prices.
#[cfg(feature = "openai")]
pub(crate) fn openai(
    model: String,
    key: Option<tau_drivers::model::openai::ApiKey>,
    ollama: bool,
) -> MakeModel {
    use tau_drivers::model::openai::{OpenAiConfig, OpenAiDriver};
    Box::new(move |base: &str| {
        let (i, o) = if ollama { (0, 0) } else { (1, 4) };
        let mut cfg = OpenAiConfig::new(model.clone(), key.clone(), 16_000, 4_096, i, o);
        cfg.base_url = base.to_owned();
        let driver = OpenAiDriver::new(cfg).unwrap();
        let ceiling = driver.ceiling();
        (Arc::new(driver) as Arc<dyn Driver>, ceiling)
    })
}

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
    /// The provider's side, in order: the cassette's exchanges on replay,
    /// what the relay saw when live.
    pub(crate) exchanges: Vec<Exchange>,
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
    let stub = super::start(super::script(answers)).await;
    drive(stub, cassette.exchanges, make, program).await
}

/// Like [`run`], but the model's side is the real provider behind
/// `target`, reached through the forwarding stub; the exchanges are what
/// the relay saw. Nothing is written to disk.
pub(crate) async fn live(target: Target, make: MakeModel, program: Program) -> Run {
    let (stub, mut relayed) = super::start_forwarding(target.live_base_url()).await;
    let mut run = drive(stub, Vec::new(), make, program).await;
    while let Ok(r) = relayed.try_recv() {
        let request = cassette::redact(&r.request)
            .unwrap_or_else(|h| panic!("{}: refusing to keep header {h}", target.dir_name()));
        let body: Value = serde_json::from_slice(&r.body).unwrap_or_else(|e| {
            panic!(
                "{}: provider body is not JSON: {e}: {}",
                target.dir_name(),
                String::from_utf8_lossy(&r.body)
            )
        });
        run.exchanges.push(Exchange {
            request,
            response: RecordedResponse {
                status: r.status,
                body,
            },
        });
    }
    run
}

async fn drive(mut stub: Stub, exchanges: Vec<Exchange>, make: MakeModel, program: Program) -> Run {
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
        exchanges,
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
        for (i, (sent, ex)) in self.sent.iter().zip(&self.exchanges).enumerate() {
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
