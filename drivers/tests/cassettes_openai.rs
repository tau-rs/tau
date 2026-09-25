//! The OpenAI-compatible driver against recorded provider exchanges: the
//! full ADR-0006 matrix on `gpt-4.1-mini`, the same shape replayed against a
//! local Ollama on `qwen3:1.7b` (a subset — no single-turn `parallel_tool_calls`,
//! `stop_sequence_stop`, `max_completion_tokens_cap`, or `bad_key_401`).
//! `TAU_RECORD=1` re-records through the relay.

#![cfg(feature = "openai")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::calc;
use common::cassette;
use common::scenario::{self, Expect, Make, Scenario, Step, Target};
use serde_json::Value;
use tau_drivers::model::openai::{
    ApiKey, OpenAiConfig, OpenAiDriver, OutputCap, API_KEY_ENV, PROVIDER,
};
use tau_kernel::bridge::{
    Content, Message, ModelReply, ModelRequest, Role, Sampling, StopReason, VERSION,
};
use tau_kernel::driver::Driver;

const MINI: &str = "gpt-4.1-mini";
const QWEN: &str = "qwen3:1.7b";

/// The scenario names Ollama answers; everything else in `scenarios` is
/// OpenAI-only (the `–` column of the brief's table).
const OLLAMA_SCENARIOS: &[&str] = &[
    "text_end_turn",
    "tool_call",
    "tool_result_round_trip",
    "parallel_tool_calls_round_trip",
    "max_tokens_stop",
    "sampling_accepted",
    "bad_request_400",
    "unknown_model_404",
];

fn key(target: Target) -> Option<ApiKey> {
    match target {
        Target::Ollama => None,
        _ if scenario::record_enabled() => {
            Some(ApiKey::from_env(API_KEY_ENV).expect("OPENAI_API_KEY"))
        }
        _ => Some(ApiKey::new("replay")),
    }
}

fn make_with(target: Target, model: &'static str, key: Option<ApiKey>, cap: OutputCap) -> Make {
    Box::new(move |base: &str| {
        let (i, o) = if target == Target::Ollama {
            (0, 0)
        } else {
            (1, 4)
        };
        let mut cfg = OpenAiConfig::new(model, key.clone(), 16_000, 4_096, i, o);
        cfg.base_url = base.to_owned();
        cfg.output_cap = cap;
        Box::new(OpenAiDriver::new(cfg).unwrap()) as Box<dyn Driver>
    })
}

fn make(target: Target, model: &'static str) -> Make {
    make_with(target, model, key(target), OutputCap::default())
}

/// Ollama's `qwen3:1.7b` reasons in-band before answering, and Ollama's
/// OpenAI-compatible endpoint exposes no knob the driver sends to turn that
/// off, so a tight cap burns entirely on reasoning tokens and leaves no
/// content. Widen the budget for Ollama (measured: the pong prompt needs
/// 136-200 tokens); OpenAI keeps its own `wanted`.
fn budget(target: Target, wanted: u32) -> u32 {
    if target == Target::Ollama {
        wanted.max(512)
    } else {
        wanted
    }
}

fn text(prompt: &str, max_tokens: u32) -> ModelRequest {
    ModelRequest {
        v: VERSION,
        system: Some("You are terse.".into()),
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text {
                text: prompt.into(),
            }],
        }],
        tools: vec![],
        max_tokens,
        sampling: None,
    }
}

fn with_calc(mut req: ModelRequest) -> ModelRequest {
    req.tools = vec![calc::tool_def()];
    req
}

/// The second turn: the assistant's reply (tool calls, verbatim) then one
/// `tool_result` per call.
fn calc_result(first: &ModelRequest, prev: &ModelReply) -> ModelRequest {
    let mut req = first.clone();
    req.messages.push(Message {
        role: Role::Assistant,
        content: prev.content.clone(),
    });
    let results = prev
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall { id, input, .. } => Some(Content::ToolResult {
                call_id: id.clone(),
                content: calc::answer(input),
                is_error: false,
                error_kind: None,
            }),
            _ => None,
        })
        .collect();
    req.messages.push(Message {
        role: Role::User,
        content: results,
    });
    req
}

fn one(
    name: &'static str,
    target: Target,
    model: &'static str,
    build: impl Fn() -> ModelRequest + Send + Sync + 'static,
    expect: Expect,
) -> Scenario {
    Scenario {
        name,
        target,
        model,
        steps: vec![Step {
            build: Box::new(move |_| build()),
            expect,
        }],
    }
}

fn round_trip(
    name: &'static str,
    target: Target,
    model: &'static str,
    prompt: &'static str,
    max_tokens: u32,
    first_expect: Expect,
) -> Scenario {
    let first = move || with_calc(text(prompt, max_tokens));
    Scenario {
        name,
        target,
        model,
        steps: vec![
            Step {
                build: Box::new(move |_| first()),
                expect: first_expect,
            },
            Step {
                build: Box::new(move |prev| calc_result(&first(), prev.last().unwrap())),
                expect: Expect::Stop(StopReason::EndTurn),
            },
        ],
    }
}

/// What a calculator call looks like on each target. `qwen3:1.7b` reasons
/// in-band before every answer and Ollama exposes it as `reasoning`, which
/// the driver seals as a `thinking` block (#123): the Ollama cassettes pin
/// that the block is there. OpenAI's chat wire carries no reasoning.
fn calc_call(target: Target) -> Expect {
    if target == Target::Ollama {
        Expect::ThinkingToolCall { name: "calculator" }
    } else {
        Expect::ToolCall {
            name: "calculator",
            min: 1,
        }
    }
}

fn scenarios(target: Target) -> Vec<(Scenario, Make)> {
    let model = if target == Target::Ollama { QWEN } else { MINI };
    let unknown_model = if target == Target::Ollama {
        "does-not-exist:latest"
    } else {
        "gpt-does-not-exist"
    };

    let mut stop5 = text(
        "Count from 1 to 10, one number per line.",
        budget(target, 128),
    );
    stop5.sampling = Some(Sampling {
        stop_sequences: vec!["5".into()],
        ..Sampling::default()
    });
    let mut warm = text("pong?", budget(target, 32));
    warm.sampling = Some(Sampling {
        temperature: Some(0.2),
        ..Sampling::default()
    });
    // `system` must be `None` here too: the OpenAI-compatible driver folds
    // it into a first `system`-role message, so `system: Some(_)` alone
    // would make this a one-message request, not an empty one.
    let empty = ModelRequest {
        system: None,
        messages: vec![],
        ..text("", budget(target, 32))
    };

    let all = vec![
        (
            one(
                "text_end_turn",
                target,
                model,
                move || text("Reply with the single word: pong.", budget(target, 64)),
                Expect::Stop(StopReason::EndTurn),
            ),
            make(target, model),
        ),
        (
            one(
                "tool_call",
                target,
                model,
                move || {
                    with_calc(text(
                        "What is 17*23? Use the calculator tool.",
                        budget(target, 256),
                    ))
                },
                calc_call(target),
            ),
            make(target, model),
        ),
        (
            round_trip(
                "tool_result_round_trip",
                target,
                model,
                "What is 17*23? Use the calculator tool.",
                budget(target, 256),
                calc_call(target),
            ),
            make(target, model),
        ),
        (
            one(
                "parallel_tool_calls",
                target,
                model,
                move || {
                    with_calc(text(
                        "Compute 2+2 and 3+3 as two separate calculator calls in one turn.",
                        budget(target, 512),
                    ))
                },
                Expect::ToolCall {
                    name: "calculator",
                    min: 2,
                },
            ),
            make(target, model),
        ),
        (
            round_trip(
                "parallel_tool_calls_round_trip",
                target,
                model,
                "Compute 2+2 and 3+3 as two separate calculator calls in one turn.",
                budget(target, 512),
                Expect::ToolCall {
                    name: "calculator",
                    min: 2,
                },
            ),
            make(target, model),
        ),
        (
            one(
                "max_tokens_stop",
                target,
                model,
                || text("Write a 500-word essay about rivers.", 16),
                Expect::Stop(StopReason::MaxTokens),
            ),
            make(target, model),
        ),
        (
            // OpenAI's chat completions does not distinguish a stop
            // sequence from a natural stop, so this cassette pins that the
            // sequence is applied but the bridge reports `end_turn`.
            one(
                "stop_sequence_stop",
                target,
                model,
                move || stop5.clone(),
                Expect::Stop(StopReason::EndTurn),
            ),
            make(target, model),
        ),
        (
            one(
                "sampling_accepted",
                target,
                model,
                move || warm.clone(),
                Expect::Stop(StopReason::EndTurn),
            ),
            make(target, model),
        ),
        (
            one(
                "max_completion_tokens_cap",
                target,
                model,
                move || text("pong?", budget(target, 32)),
                Expect::Stop(StopReason::EndTurn),
            ),
            make_with(target, model, key(target), OutputCap::MaxCompletionTokens),
        ),
        (
            one(
                "bad_request_400",
                target,
                model,
                move || empty.clone(),
                Expect::ProviderError,
            ),
            make(target, model),
        ),
        (
            one(
                "bad_key_401",
                target,
                model,
                move || text("pong?", budget(target, 32)),
                Expect::ProviderError,
            ),
            make_with(
                target,
                model,
                Some(ApiKey::new("invalid-key")),
                OutputCap::default(),
            ),
        ),
        (
            one(
                "unknown_model_404",
                target,
                unknown_model,
                move || text("pong?", budget(target, 32)),
                Expect::ProviderError,
            ),
            make(target, unknown_model),
        ),
    ];

    if target == Target::Ollama {
        all.into_iter()
            .filter(|(s, _)| OLLAMA_SCENARIOS.contains(&s.name))
            .collect()
    } else {
        all
    }
}

fn find(target: Target, name: &str) -> (Scenario, Make) {
    scenarios(target)
        .into_iter()
        .find(|(s, _)| s.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
}

macro_rules! replay_tests {
    ($target:expr, $($fn_name:ident => $scenario:literal),* $(,)?) => { $(
        #[tokio::test]
        async fn $fn_name() { let (s, make) = find($target, $scenario); scenario::replay(&s, make).await; }
    )* };
}

replay_tests! { Target::OpenAi,
    openai_text_end_turn => "text_end_turn",
    openai_tool_call => "tool_call",
    openai_tool_result_round_trip => "tool_result_round_trip",
    openai_parallel_tool_calls => "parallel_tool_calls",
    openai_parallel_tool_calls_round_trip => "parallel_tool_calls_round_trip",
    openai_max_tokens_stop => "max_tokens_stop",
    openai_stop_sequence_stop => "stop_sequence_stop",
    openai_sampling_accepted => "sampling_accepted",
    openai_max_completion_tokens_cap => "max_completion_tokens_cap",
    openai_bad_request_400 => "bad_request_400",
    openai_bad_key_401 => "bad_key_401",
    openai_unknown_model_404 => "unknown_model_404",
}

replay_tests! { Target::Ollama,
    ollama_text_end_turn => "text_end_turn",
    ollama_tool_call => "tool_call",
    ollama_tool_result_round_trip => "tool_result_round_trip",
    ollama_parallel_tool_calls_round_trip => "parallel_tool_calls_round_trip",
    ollama_max_tokens_stop => "max_tokens_stop",
    ollama_sampling_accepted => "sampling_accepted",
    ollama_bad_request_400 => "bad_request_400",
    ollama_unknown_model_404 => "unknown_model_404",
}

/// #123, on the recorded Ollama round trip: both replies carry the
/// server's `reasoning` text verbatim as the first block, tagged with this
/// driver's `provider`; the second request on the wire is the recording,
/// whose assistant turn has no reasoning in it — the block is read by the
/// program and never replayed to the server.
#[tokio::test]
async fn ollama_reasoning_is_the_recorded_text_and_is_not_sent_back() {
    let c = cassette::load("ollama", "tool_result_round_trip").expect("cassette");
    let answers = c
        .exchanges
        .iter()
        .map(|e| (e.response.status, e.response.body.to_string()));
    let mut stub = common::start(common::script(answers)).await;
    let driver = make(Target::Ollama, QWEN)(&stub.base_url);
    let recorded = |i: usize| -> &Value {
        c.exchanges
            .get(i)
            .and_then(|e| e.response.body.pointer("/choices/0/message/reasoning"))
            .expect("the cassette carries `reasoning` on both replies")
    };

    let first = with_calc(text(
        "What is 17*23? Use the calculator tool.",
        budget(Target::Ollama, 256),
    ));
    let (reply, _) = scenario::call(driver.as_ref(), 1, &first).await;
    assert_eq!(reply.stop, StopReason::ToolCall);
    assert_eq!(
        reply.content.first(),
        Some(&Content::Thinking {
            provider: PROVIDER.into(),
            data: recorded(0).clone(),
        }),
        "{:?}",
        reply.content
    );
    stub.captured.recv().await.expect("first request");

    let second = calc_result(&first, &reply);
    let (reply, _) = scenario::call(driver.as_ref(), 2, &second).await;
    assert_eq!(reply.stop, StopReason::EndTurn);
    assert_eq!(
        reply.content.first(),
        Some(&Content::Thinking {
            provider: PROVIDER.into(),
            data: recorded(1).clone(),
        }),
        "{:?}",
        reply.content
    );
    let sent = stub.captured.recv().await.expect("second request");
    assert_eq!(
        sent.json(),
        c.exchanges.get(1).unwrap().request.body,
        "the second request is the recording, reasoning and all absent"
    );
    let raw = String::from_utf8(sent.body.clone()).unwrap();
    assert!(!raw.contains("reasoning"), "{raw}");
}

#[tokio::test]
#[ignore = "TAU_RECORD=1 and OPENAI_API_KEY; costs money"]
async fn record_openai() {
    if !scenario::record_enabled() {
        eprintln!("TAU_RECORD is not 1; skipping");
        return;
    }
    let mut total = 0;
    for (s, make) in scenarios(Target::OpenAi)
        .into_iter()
        .filter(|(s, _)| scenario::record_selected(s.name))
    {
        let c = scenario::record(&s, make).await;
        total += c.get(&tau_kernel::abi::DimKey::CostMicroUsd).unwrap_or(0);
    }
    eprintln!("openai matrix recorded; {total} µUSD");
}

#[tokio::test]
#[ignore = "TAU_RECORD=1 and a local Ollama"]
async fn record_ollama() {
    if reqwest::get(format!("{}/api/tags", Target::Ollama.live_base_url()))
        .await
        .is_err()
    {
        eprintln!("ollama is not reachable; skipping");
        return;
    }
    if !scenario::record_enabled() {
        eprintln!("TAU_RECORD is not 1; skipping");
        return;
    }
    let mut total = 0;
    for (s, make) in scenarios(Target::Ollama)
        .into_iter()
        .filter(|(s, _)| scenario::record_selected(s.name))
    {
        let c = scenario::record(&s, make).await;
        total += c.get(&tau_kernel::abi::DimKey::CostMicroUsd).unwrap_or(0);
    }
    eprintln!("ollama matrix recorded; {total} µUSD");
}
