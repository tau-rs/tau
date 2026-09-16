//! End to end, offline: an agent program runs the real `libtau` tool loop
//! through the kernel against each model driver, and the provider's side
//! of the conversation is a recorded cassette. The loop decides to call the
//! calculator, feeds its answer back, and stops on `end_turn`; every
//! request it caused must equal the recording byte for byte, so the loop,
//! the toolbox projection, the driver's encoding, and the kernel's
//! send/recv/settle are all on real provider bytes with no network.

#![cfg(all(feature = "anthropic", feature = "openai"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::e2e::{
    self, ample, MakeModel, Program, Run, CALC_PROMPT, CALC_TOKENS, HAIKU, MINI, OPUS,
    PARALLEL_PROMPT, QWEN, THINKING_PROMPT,
};
use libtau::ToolLoopError;
use serde_json::json;
use tau_kernel::abi::{Budget, DimKey};
use tau_kernel::bridge::{Content, Role, StopReason};

/// The Anthropic driver with a placeholder key: the stub never checks it.
fn anthropic(model: &'static str) -> MakeModel {
    e2e::anthropic(model, tau_drivers::model::anthropic::ApiKey::new("replay"))
}

/// The OpenAI driver with a placeholder key, or Ollama with none.
fn openai(model: &'static str, ollama: bool) -> MakeModel {
    let key = (!ollama).then(|| tau_drivers::model::openai::ApiKey::new("replay"));
    e2e::openai(model.to_owned(), key, ollama)
}

// --- tool loop to end_turn ------------------------------------------------

/// The round trip through the loop: two requests on the wire, both the
/// recording; the calculator was called with the model's input; the
/// transcript is user, assistant (tool call), user (tool result), with
/// the final text as the returned reply; the agent paid the cassette's
/// usage plus the calculator's.
async fn round_trip(dir_name: &str, make: MakeModel, max_tokens: u32) -> Run {
    let run = e2e::run(
        dir_name,
        "tool_result_round_trip",
        make,
        Program {
            user: CALC_PROMPT,
            max_tokens,
            budget: ample,
        },
    )
    .await;
    assert_eq!(run.reply().stop, StopReason::EndTurn);
    run.assert_sent_matches_recording(2);
    assert_eq!(run.calls, [json!({"expression": "17*23"})]);

    let roles: Vec<Role> = run.transcript.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    let Some(Content::ToolResult {
        content, is_error, ..
    }) = run
        .transcript
        .messages
        .get(2)
        .and_then(|m| m.content.first())
    else {
        panic!("{:?}", run.transcript.messages);
    };
    assert_eq!(content, "391");
    assert!(!is_error);
    assert_eq!(
        run.spent.get(&DimKey::Calls),
        Some(&3),
        "two model calls, one tool call"
    );
    run
}

/// The model tokens the cassette's replies report, summed.
fn usage_of(run: &Run, sum: impl Fn(&serde_json::Value) -> u64) -> u64 {
    run.exchanges.iter().map(|e| sum(&e.response.body)).sum()
}

fn anthropic_usage(body: &serde_json::Value) -> u64 {
    let u = body.get("usage").unwrap();
    u.get("input_tokens").unwrap().as_u64().unwrap()
        + u.get("output_tokens").unwrap().as_u64().unwrap()
}

fn openai_usage(body: &serde_json::Value) -> u64 {
    body.get("usage")
        .unwrap()
        .get("total_tokens")
        .unwrap()
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn anthropic_tool_loop_reaches_end_turn_over_the_cassette() {
    let run = round_trip("anthropic", anthropic(HAIKU), 256).await;
    assert_eq!(
        run.spent.get(&DimKey::Tokens),
        Some(&(usage_of(&run, anthropic_usage) + CALC_TOKENS))
    );
}

#[tokio::test]
async fn openai_tool_loop_reaches_end_turn_over_the_cassette() {
    let run = round_trip("openai", openai(MINI, false), 256).await;
    assert_eq!(
        run.spent.get(&DimKey::Tokens),
        Some(&(usage_of(&run, openai_usage) + CALC_TOKENS))
    );
}

#[tokio::test]
async fn ollama_tool_loop_reaches_end_turn_over_the_cassette() {
    let run = round_trip("ollama", openai(QWEN, true), 512).await;
    assert_eq!(
        run.spent.get(&DimKey::Tokens),
        Some(&(usage_of(&run, openai_usage) + CALC_TOKENS))
    );
}

// --- thinking replayed by the loop (Anthropic only: the OpenAI-compatible
// wire has no thinking format, ADR-0007 §2; #123 is about reading it) ------

#[tokio::test]
async fn anthropic_tool_loop_replays_the_recorded_thinking_block_on_its_second_call() {
    let run = e2e::run(
        "anthropic",
        "thinking_on_replayed_second_turn",
        anthropic(OPUS),
        Program {
            user: THINKING_PROMPT,
            max_tokens: 4096,
            budget: ample,
        },
    )
    .await;
    assert_eq!(run.reply().stop, StopReason::EndTurn);
    run.assert_sent_matches_recording(2);
    assert_eq!(
        run.calls,
        [json!({"expression": "2+3+5+7+11+13+17+19+23+29+31+37"})]
    );

    // The loop carried the sealed block: the transcript's assistant turn
    // starts with it, and the second wire request has the recorded
    // signature, unchanged in value.
    let assistant = run.transcript.messages.get(1).unwrap();
    assert!(
        matches!(assistant.content.first(), Some(Content::Thinking { .. })),
        "{:?}",
        assistant.content
    );
    let second = run.sent.get(1).unwrap().json();
    let recorded = &run.exchanges.get(1).unwrap().request.body;
    assert_eq!(
        second.pointer("/messages/1/content/0/signature"),
        recorded.pointer("/messages/1/content/0/signature")
    );
    assert!(second.pointer("/messages/1/content/0/signature").is_some());
}

// --- parallel tool calls to end_turn ---------------------------------------

/// Two calculator calls in one turn: both answered, in order, in one user
/// turn, and the second request is the recording.
async fn parallel_round_trip(dir_name: &str, make: MakeModel) -> Run {
    let run = e2e::run(
        dir_name,
        "parallel_tool_calls_round_trip",
        make,
        Program {
            user: PARALLEL_PROMPT,
            max_tokens: 512,
            budget: ample,
        },
    )
    .await;
    assert_eq!(run.reply().stop, StopReason::EndTurn);
    run.assert_sent_matches_recording(2);
    assert_eq!(
        run.calls,
        [json!({"expression": "2+2"}), json!({"expression": "3+3"})]
    );
    let results: Vec<&str> = run
        .transcript
        .messages
        .get(2)
        .unwrap()
        .content
        .iter()
        .map(|c| match c {
            Content::ToolResult {
                content,
                is_error: false,
                ..
            } => content.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(results, ["4", "6"]);
    assert_eq!(
        run.spent.get(&DimKey::Calls),
        Some(&4),
        "two model calls, two tool calls"
    );
    run
}

#[tokio::test]
async fn anthropic_tool_loop_answers_parallel_calls_over_the_cassette() {
    parallel_round_trip("anthropic", anthropic(HAIKU)).await;
}

#[tokio::test]
async fn openai_tool_loop_answers_parallel_calls_over_the_cassette() {
    parallel_round_trip("openai", openai(MINI, false)).await;
}

#[tokio::test]
async fn ollama_tool_loop_answers_parallel_calls_over_the_cassette() {
    parallel_round_trip("ollama", openai(QWEN, true)).await;
}

// --- budget refusal terminal ---------------------------------------------

/// Room for the first model call and the calculator, one token short of
/// reserving the model's ceiling a second time: `ceiling + first usage`.
/// After the first call settles `first usage` and the calculator settles
/// its one token, `ceiling - 1` is left.
fn one_call_short(first_usage: u64) -> impl Fn(&Budget) -> Budget {
    move |ceiling: &Budget| {
        Budget::from_dims([
            (
                DimKey::Tokens,
                ceiling.get(&DimKey::Tokens).unwrap() + first_usage,
            ),
            (DimKey::CostMicroUsd, 10_000_000),
            (DimKey::Calls, 16),
        ])
    }
}

/// The loop ran the first model call and the tool, then the second `send`
/// was refused on budget: `ToolLoopError::Budget`, terminal, one request
/// on the wire, the transcript holding the tool result and nothing after.
async fn budget_refusal(
    dir_name: &str,
    make: MakeModel,
    max_tokens: u32,
    budget: fn(&Budget) -> Budget,
) {
    let run = e2e::run(
        dir_name,
        "tool_result_round_trip",
        make,
        Program {
            user: CALC_PROMPT,
            max_tokens,
            budget,
        },
    )
    .await;
    assert!(
        matches!(run.result, Err(ToolLoopError::Budget(_))),
        "{:?}",
        run.result
    );
    run.assert_sent_matches_recording(1);
    assert_eq!(
        run.calls.len(),
        1,
        "the calculator answered before the refusal"
    );
    let roles: Vec<Role> = run.transcript.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    assert_eq!(
        run.spent.get(&DimKey::Calls),
        Some(&2),
        "nothing was retried"
    );
}

#[tokio::test]
async fn anthropic_tool_loop_stops_when_the_second_call_cannot_be_reserved() {
    // 580 + 54 on the cassette's first reply.
    budget_refusal("anthropic", anthropic(HAIKU), 256, |c| {
        one_call_short(634)(c)
    })
    .await;
}

#[tokio::test]
async fn openai_tool_loop_stops_when_the_second_call_cannot_be_reserved() {
    // total_tokens 74 on the cassette's first reply.
    budget_refusal("openai", openai(MINI, false), 256, |c| {
        one_call_short(74)(c)
    })
    .await;
}

#[tokio::test]
async fn ollama_tool_loop_stops_when_the_second_call_cannot_be_reserved() {
    // total_tokens 276 on the cassette's first reply.
    budget_refusal("ollama", openai(QWEN, true), 512, |c| {
        one_call_short(276)(c)
    })
    .await;
}
