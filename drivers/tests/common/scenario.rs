//! One scenario table, two modes: replay the cassette through the stub, or
//! record it through the relay against the real provider.

use std::collections::BTreeMap;

use serde_json::Value;
use tau_kernel::abi::{AgentId, Consumption, Corr, DimKey};
use tau_kernel::bridge::{Content, ErrorKind, ModelReply, ModelRequest, StopReason};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

use super::cassette::{self, Cassette, Exchange, RecordedResponse};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Target {
    Anthropic,
    OpenAi,
    Ollama,
}

impl Target {
    pub(crate) fn dir_name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Ollama => "ollama",
        }
    }
    pub(crate) fn live_base_url(self) -> String {
        match self {
            Self::Anthropic => "https://api.anthropic.com".into(),
            Self::OpenAi => "https://api.openai.com".into(),
            Self::Ollama => std::env::var("TAU_OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
        }
    }
}

pub(crate) enum Expect {
    Stop(StopReason),
    ToolCall {
        name: &'static str,
        min: usize,
    },
    /// Like `ToolCall`, but also requires at least one `Content::Thinking`
    /// block: for scenarios that must force adaptive thinking to engage,
    /// where a `ToolCall` reply with no thinking block means the prompt was
    /// too easy, not that the driver is broken.
    ThinkingToolCall {
        name: &'static str,
    },
    ProviderError,
    Policy,
}

/// Checks one reply against its expectation. `name` and `step` (zero-based,
/// as in `run_steps`) prefix every assertion: the probe replays run more
/// than a hundred scenarios concurrently inside one test, so a bare
/// `ModelReply` in a panic cannot be traced back to its cassette.
pub(crate) fn check(
    name: &str,
    step: usize,
    expect: &Expect,
    reply: &ModelReply,
    consumed: &Consumption,
) {
    let at = format!("{name} step {step}:");
    let billed = || {
        assert!(
            reply.usage.input_tokens > 0 && reply.usage.output_tokens > 0,
            "{at} usage is real: {:?}",
            reply.usage
        );
        assert_eq!(
            consumed.get(&DimKey::Tokens),
            Some(reply.usage.input_tokens + reply.usage.output_tokens),
            "{at} billed tokens do not match the reply's usage: {:?}",
            reply.usage
        );
    };
    match expect {
        Expect::Stop(stop) => {
            assert_eq!(&reply.stop, stop, "{at} {reply:#?}");
            billed();
        }
        Expect::ToolCall { name: tool, min } => {
            assert_eq!(reply.stop, StopReason::ToolCall, "{at} {reply:#?}");
            let calls: Vec<_> = reply
                .content
                .iter()
                .filter(|c| matches!(c, Content::ToolCall { .. }))
                .collect();
            assert!(
                calls.len() >= *min,
                "{at} wanted >= {min} tool calls, got {}: {reply:#?}",
                calls.len()
            );
            for c in calls {
                if let Content::ToolCall { name: n, .. } = c {
                    assert_eq!(n, tool, "{at} wrong tool: {reply:#?}");
                }
            }
            billed();
        }
        Expect::ThinkingToolCall { name: tool } => {
            assert_eq!(reply.stop, StopReason::ToolCall, "{at} {reply:#?}");
            assert!(
                reply
                    .content
                    .iter()
                    .any(|c| matches!(c, Content::Thinking { .. })),
                "{at} no thinking block: adaptive thinking did not engage; use a harder prompt: {reply:#?}"
            );
            let calls: Vec<_> = reply
                .content
                .iter()
                .filter(|c| matches!(c, Content::ToolCall { .. }))
                .collect();
            assert!(!calls.is_empty(), "{at} no tool call: {reply:#?}");
            for c in calls {
                if let Content::ToolCall { name: n, .. } = c {
                    assert_eq!(n, tool, "{at} wrong tool: {reply:#?}");
                }
            }
            billed();
        }
        Expect::ProviderError => {
            assert!(
                matches!(&reply.stop, StopReason::Error(e) if e.kind == ErrorKind::Provider),
                "{at} {reply:#?}"
            );
            assert_eq!(
                consumed.get(&DimKey::Tokens),
                None,
                "{at} a rejection bills nothing"
            );
        }
        Expect::Policy => match &reply.stop {
            StopReason::Error(e) => assert_eq!(e.kind, ErrorKind::Provider, "{at} {reply:#?}"),
            _ => billed(),
        },
    }
}

pub(crate) type Build = Box<dyn Fn(&[ModelReply]) -> ModelRequest + Send + Sync>;
pub(crate) struct Step {
    pub(crate) build: Build,
    pub(crate) expect: Expect,
}
pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) target: Target,
    pub(crate) model: &'static str,
    pub(crate) steps: Vec<Step>,
}
pub(crate) type Make = Box<dyn Fn(&str) -> Box<dyn Driver> + Send + Sync>;

pub(crate) async fn call(
    driver: &dyn Driver,
    corr: u64,
    req: &ModelRequest,
) -> (ModelReply, Consumption) {
    let (bytes, consumed) = driver
        .handle(Delivery {
            corr: Corr::new(corr),
            from: AgentId::new(1),
            payload: serde_json::to_vec(req).unwrap(),
        })
        .await;
    (serde_json::from_slice(&bytes).unwrap(), consumed)
}

pub(crate) fn record_enabled() -> bool {
    std::env::var("TAU_RECORD").as_deref() == Ok("1")
}

/// Whether `name` is in `TAU_RECORD_ONLY` (comma-separated scenario
/// names); every scenario when it is unset. Re-recording one new scenario
/// should not churn a dozen committed cassettes.
pub(crate) fn record_selected(name: &str) -> bool {
    match std::env::var("TAU_RECORD_ONLY") {
        Ok(only) => only.split(',').any(|s| s.trim() == name),
        Err(_) => true,
    }
}

/// Sums two consumptions dimension-wise. `Consumption` has no add of its
/// own; fold both `iter()`s into a `BTreeMap` (never `HashMap` — the reducer
/// lint bans it workspace-wide, and this helper is compiled into the same
/// crate) and rebuild from the total.
fn add(a: &Consumption, b: &Consumption) -> Consumption {
    let mut total: BTreeMap<DimKey, u64> = BTreeMap::new();
    for (dim, amount) in a.iter().chain(b.iter()) {
        *total.entry(dim.clone()).or_insert(0) += amount;
    }
    Consumption::from_dims(total)
}

/// Runs every step of `s` against `driver`, checking each reply as it
/// arrives. Returns the replies in order and their summed consumption.
async fn run_steps(s: &Scenario, driver: &dyn Driver) -> (Vec<ModelReply>, Consumption) {
    let mut replies = Vec::new();
    let mut total = Consumption::default();
    for (i, step) in s.steps.iter().enumerate() {
        let req = (step.build)(&replies);
        let (reply, consumed) = call(driver, i as u64 + 1, &req).await;
        check(s.name, i, &step.expect, &reply, &consumed);
        total = add(&total, &consumed);
        replies.push(reply);
    }
    (replies, total)
}

pub(crate) async fn replay(s: &Scenario, make: Make) {
    replay_from(s, s.target.dir_name(), make).await;
}

pub(crate) async fn replay_from(s: &Scenario, dir_name: &str, make: Make) {
    let c = cassette::load(dir_name, s.name).unwrap_or_else(|| {
        panic!(
            "{}/{}: no cassette; run `just live record`",
            dir_name, s.name
        )
    });
    let answers = c
        .exchanges
        .iter()
        .map(|e| (e.response.status, e.response.body.to_string()));
    let mut stub = super::start(super::script(answers)).await;
    let driver = make(&stub.base_url);
    let (_, _) = run_steps(s, driver.as_ref()).await;
    for (i, ex) in c.exchanges.iter().enumerate() {
        let sent = stub
            .captured
            .try_recv()
            .unwrap_or_else(|_| panic!("{}: fewer requests than recorded (missing #{i})", s.name));
        assert_eq!(
            sent.path(),
            ex.request.path,
            "{}: exchange #{i} path",
            s.name
        );
        assert_eq!(
            sent.json(),
            ex.request.body,
            "{}: exchange #{i} body differs from the recording",
            s.name
        );
    }
    assert!(
        stub.captured.try_recv().is_err(),
        "{}: more requests than recorded",
        s.name
    );
}

pub(crate) async fn record(s: &Scenario, make: Make) -> Consumption {
    let (stub, mut relayed) = super::start_forwarding(s.target.live_base_url()).await;
    let driver = make(&stub.base_url);
    let (replies, total) = run_steps(s, driver.as_ref()).await;
    let mut exchanges = Vec::new();
    while let Ok(r) = relayed.try_recv() {
        let request = cassette::redact(&r.request)
            .unwrap_or_else(|h| panic!("{}: refusing to record header {h}", s.name));
        let body: Value = serde_json::from_slice(&r.body)
            .unwrap_or_else(|e| panic!("{}: provider body is not JSON: {e}", s.name));
        exchanges.push(Exchange {
            request,
            response: RecordedResponse {
                status: r.status,
                body,
            },
        });
    }
    assert!(!exchanges.is_empty(), "{}: nothing was relayed", s.name);
    let model = replies
        .iter()
        .rev()
        .find_map(|r| r.model.clone())
        .or_else(|| Some(s.model.to_owned()));
    let c = Cassette {
        v: 1,
        recorded_at: today(),
        target: s.target.dir_name().to_owned(),
        model,
        exchanges,
    };
    cassette::save(&c, s.name);
    eprintln!("recorded {}/{}", s.target.dir_name(), s.name);
    total
}

fn today() -> String {
    // Date only, from the system clock via `std::process` to stay clear of
    // the `SystemTime::now` lint: the recorder runs by hand, never in the reducer.
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .expect("date");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
