//! The Anthropic driver against recorded provider exchanges: the full
//! ADR-0006 matrix on Haiku 4.5, thinking on Opus 5, replayed by the stub.
//! `TAU_RECORD=1` re-records through the relay.

#![cfg(feature = "anthropic")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::calc;
use common::scenario::{self, Expect, Make, Scenario, Step, Target};
use tau_drivers::model::anthropic::{
    AnthropicConfig, AnthropicDriver, ApiKey, InputEstimate, SamplingMode, ThinkingMode,
    API_KEY_ENV,
};
use tau_kernel::bridge::{
    Content, Message, ModelReply, ModelRequest, Role, Sampling, StopReason, VERSION,
};
use tau_kernel::driver::Driver;

const HAIKU: &str = "claude-haiku-4-5-20251001";
const OPUS: &str = "claude-opus-5";

fn key() -> ApiKey {
    if scenario::record_enabled() {
        ApiKey::from_env(API_KEY_ENV).expect("ANTHROPIC_API_KEY")
    } else {
        ApiKey::new("replay")
    }
}

fn price(model: &str) -> (u64, u64) {
    if model.starts_with("claude-opus") {
        (5, 25)
    } else {
        (1, 5)
    }
}

fn make_with(
    model: &'static str,
    key: ApiKey,
    thinking: ThinkingMode,
    sampling: SamplingMode,
    estimate: InputEstimate,
) -> Make {
    Box::new(move |base: &str| {
        let (i, o) = price(model);
        let mut cfg = AnthropicConfig::new(model, key.clone(), 16_000, 4_096, i, o);
        cfg.base_url = base.to_owned();
        cfg.thinking = thinking;
        cfg.sampling = sampling;
        cfg.estimate = estimate;
        Box::new(AnthropicDriver::new(cfg).unwrap()) as Box<dyn Driver>
    })
}

fn make(model: &'static str) -> Make {
    make_with(
        model,
        key(),
        ThinkingMode::default(),
        SamplingMode::default(),
        InputEstimate::default(),
    )
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

/// The second turn: the assistant's reply (thinking + tool calls, verbatim)
/// then one `tool_result` per call, answered by the shared calculator.
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
    model: &'static str,
    build: impl Fn() -> ModelRequest + Send + Sync + 'static,
    expect: Expect,
) -> Scenario {
    Scenario {
        name,
        target: Target::Anthropic,
        model,
        steps: vec![Step {
            build: Box::new(move |_| build()),
            expect,
        }],
    }
}

fn round_trip(
    name: &'static str,
    model: &'static str,
    prompt: &'static str,
    max_tokens: u32,
    first_expect: Expect,
) -> Scenario {
    let first = move || with_calc(text(prompt, max_tokens));
    Scenario {
        name,
        target: Target::Anthropic,
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

fn scenarios() -> Vec<(Scenario, Make)> {
    let mut stop5 = text("Count from 1 to 10, one number per line.", 128);
    stop5.sampling = Some(Sampling {
        stop_sequences: vec!["5".into()],
        ..Sampling::default()
    });
    let mut warm = text("pong?", 32);
    warm.sampling = Some(Sampling {
        temperature: Some(0.2),
        ..Sampling::default()
    });
    let empty = ModelRequest {
        messages: vec![],
        ..text("", 32)
    };

    vec![
        (
            one(
                "text_end_turn",
                HAIKU,
                || text("Reply with the single word: pong.", 64),
                Expect::Stop(StopReason::EndTurn),
            ),
            make(HAIKU),
        ),
        (
            one(
                "tool_call",
                HAIKU,
                || with_calc(text("What is 17*23? Use the calculator tool.", 256)),
                Expect::ToolCall {
                    name: "calculator",
                    min: 1,
                },
            ),
            make(HAIKU),
        ),
        (
            round_trip(
                "tool_result_round_trip",
                HAIKU,
                "What is 17*23? Use the calculator tool.",
                256,
                Expect::ToolCall {
                    name: "calculator",
                    min: 1,
                },
            ),
            make(HAIKU),
        ),
        (
            one(
                "parallel_tool_calls",
                HAIKU,
                || {
                    with_calc(text(
                        "Compute 2+2 and 3+3 as two separate calculator calls in one turn.",
                        512,
                    ))
                },
                Expect::ToolCall {
                    name: "calculator",
                    min: 2,
                },
            ),
            make(HAIKU),
        ),
        (
            round_trip(
                "parallel_tool_calls_round_trip",
                HAIKU,
                "Compute 2+2 and 3+3 as two separate calculator calls in one turn.",
                512,
                Expect::ToolCall {
                    name: "calculator",
                    min: 2,
                },
            ),
            make(HAIKU),
        ),
        (
            one(
                "max_tokens_stop",
                HAIKU,
                || text("Write a 500-word essay about rivers.", 16),
                Expect::Stop(StopReason::MaxTokens),
            ),
            make(HAIKU),
        ),
        (
            one(
                "stop_sequence_stop",
                HAIKU,
                move || stop5.clone(),
                Expect::Stop(StopReason::StopSequence),
            ),
            make(HAIKU),
        ),
        (
            one(
                "sampling_accepted",
                HAIKU,
                move || warm.clone(),
                Expect::Stop(StopReason::EndTurn),
            ),
            make_with(
                HAIKU,
                key(),
                ThinkingMode::default(),
                SamplingMode::Accepted,
                InputEstimate::default(),
            ),
        ),
        (
            round_trip(
                "thinking_on_replayed_second_turn",
                OPUS,
                "Work out the sum of the first 12 prime numbers step by step, then verify your total by calling the calculator tool once with the full addition expression.",
                4096,
                Expect::ThinkingToolCall { name: "calculator" },
            ),
            make(OPUS),
        ),
        (
            one(
                "thinking_disabled",
                OPUS,
                || text("pong?", 64),
                Expect::Stop(StopReason::EndTurn),
            ),
            make_with(
                OPUS,
                key(),
                ThinkingMode::Disabled,
                SamplingMode::default(),
                InputEstimate::default(),
            ),
        ),
        (
            one(
                "count_tokens_estimate",
                HAIKU,
                || text("pong?", 32),
                Expect::Stop(StopReason::EndTurn),
            ),
            make_with(
                HAIKU,
                key(),
                ThinkingMode::default(),
                SamplingMode::default(),
                InputEstimate::CountTokens,
            ),
        ),
        (
            one(
                "bad_request_400",
                HAIKU,
                move || empty.clone(),
                Expect::ProviderError,
            ),
            make(HAIKU),
        ),
        (
            one(
                "bad_key_401",
                HAIKU,
                || text("pong?", 32),
                Expect::ProviderError,
            ),
            make_with(
                HAIKU,
                ApiKey::new("invalid-key"),
                ThinkingMode::default(),
                SamplingMode::default(),
                InputEstimate::default(),
            ),
        ),
        (
            one(
                "unknown_model_404",
                "claude-does-not-exist",
                || text("pong?", 32),
                Expect::ProviderError,
            ),
            make("claude-does-not-exist"),
        ),
    ]
}

fn find(name: &str) -> (Scenario, Make) {
    scenarios()
        .into_iter()
        .find(|(s, _)| s.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
}

macro_rules! replay_tests {
    ($($name:ident),* $(,)?) => { $(
        #[tokio::test]
        async fn $name() { let (s, make) = find(stringify!($name)); scenario::replay(&s, make).await; }
    )* };
}

replay_tests! {
    text_end_turn, tool_call, tool_result_round_trip, parallel_tool_calls,
    parallel_tool_calls_round_trip, max_tokens_stop,
    stop_sequence_stop, sampling_accepted, thinking_on_replayed_second_turn, thinking_disabled,
    count_tokens_estimate, bad_request_400, bad_key_401, unknown_model_404,
}

#[tokio::test]
#[ignore = "TAU_RECORD=1 and ANTHROPIC_API_KEY; costs money"]
async fn record_all() {
    if !scenario::record_enabled() {
        eprintln!("TAU_RECORD is not 1; skipping");
        return;
    }
    let mut total = 0;
    for (s, make) in scenarios()
        .into_iter()
        .filter(|(s, _)| scenario::record_selected(s.name))
    {
        let c = scenario::record(&s, make).await;
        total += c.get(&tau_kernel::abi::DimKey::CostMicroUsd).unwrap_or(0);
    }
    eprintln!("anthropic matrix recorded; {total} µUSD");
}
