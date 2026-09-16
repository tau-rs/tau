//! End to end, live: the `e2e_cassettes.rs` programs, but the provider's
//! side is the real Anthropic API, the real OpenAI API, or a local Ollama,
//! reached through the forwarding stub (the recorder's relay). Ignored by
//! default, never in CI; `just live e2e [anthropic|openai|ollama]` runs
//! them with keys from the Keychain, or by hand:
//!
//! ```sh
//! TAU_ANTHROPIC_LIVE=1 ANTHROPIC_API_KEY=... \
//!     cargo test -p tau-drivers --all-features --test e2e_live -- --ignored --nocapture anthropic_live
//! TAU_OPENAI_LIVE=1 OPENAI_API_KEY=... \
//!     cargo test -p tau-drivers --all-features --test e2e_live -- --ignored --nocapture openai_live
//! TAU_OLLAMA_LIVE=1 [TAU_OLLAMA_BASE_URL=http://localhost:11434] [TAU_OLLAMA_MODEL=qwen3:1.7b] \
//!     cargo test -p tau-drivers --all-features --test e2e_live -- --ignored --nocapture ollama_live
//! ```
//!
//! A live model chooses its own expression and call count, so a live row
//! asserts the shape of what happened, not the bytes: the loop ended on
//! `end_turn`, the calculator answered every call, the kernel billed
//! exactly the usage the provider reported. Byte equality with a
//! recording is replay's job, in `e2e_cassettes.rs`.
//!
//! Rows run cheapest model first, in one process per provider, and the
//! run stops before a row whose worst case (two calls at the driver's
//! ceiling) would take the total past `TAU_RECORD_CAP_MICROUSD` (default
//! 3 000 000, three dollars): the same knob the probe recorder honours.
//! Stopping is a failure, not a pass: a suite that ran no rows has
//! verified nothing.

#![cfg(all(feature = "anthropic", feature = "openai"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::calc;
use common::e2e::{
    self, ample, MakeModel, Program, Run, CALC_PROMPT, CALC_TOKENS, HAIKU, MINI, OPUS,
    PARALLEL_PROMPT, QWEN, THINKING_PROMPT,
};
use common::scenario::Target;
use libtau::ToolLoopError;
use serde_json::Value;
use tau_kernel::abi::{Budget, DimKey};
use tau_kernel::bridge::{Content, Role, StopReason};

const CAP_ENV: &str = "TAU_RECORD_CAP_MICROUSD";
const DEFAULT_CAP_MICROUSD: u64 = 3_000_000;

fn cap() -> u64 {
    std::env::var(CAP_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CAP_MICROUSD)
}

fn enabled(flag: &str) -> bool {
    if std::env::var(flag).as_deref() == Ok("1") {
        return true;
    }
    eprintln!("{flag} is not 1; skipping");
    false
}

/// One live program and what must hold whatever the model chose.
struct Row {
    name: &'static str,
    model: String,
    make: MakeModel,
    program: Program,
    check: fn(Target, &Run),
}

/// Runs `rows` in order against `target`, stopping under the cap.
async fn run_rows(target: Target, rows: Vec<Row>) {
    let cap = cap();
    let mut spent = 0u64;
    let mut ran = 0usize;
    for row in rows {
        // The driver's ceiling is the kernel's own worst case per call, and
        // every row here makes at most two.
        let (_, ceiling) = (row.make)("http://unused.invalid");
        let worst = 2 * ceiling.get(&DimKey::CostMicroUsd).unwrap_or(0);
        assert!(
            spent + worst <= cap,
            "{}/{}: spent {spent} + worst case {worst} would exceed {CAP_ENV}={cap} µUSD; \
             {ran} rows ran, stopping",
            target.dir_name(),
            row.name
        );
        let run = e2e::live(target, row.make, row.program).await;
        if let Some(e) = run.exchanges.iter().find(|e| e.response.status == 401) {
            panic!("{}: key rejected: {}", target.dir_name(), e.response.body);
        }
        (row.check)(target, &run);
        let cost = run.spent.get(&DimKey::CostMicroUsd).copied().unwrap_or(0);
        spent += cost;
        ran += 1;
        eprintln!(
            "live {}/{} [{}]: {} requests, {} tool calls, {} tokens, {cost} µUSD",
            target.dir_name(),
            row.name,
            row.model,
            run.sent.len(),
            run.calls.len(),
            run.spent.get(&DimKey::Tokens).copied().unwrap_or(0),
        );
    }
    eprintln!("live {}: {ran} rows, {spent} µUSD", target.dir_name());
}

// --- what any live row must satisfy -------------------------------------

/// The model tokens the relayed replies report, summed, as each driver
/// bills them.
fn relayed_usage(target: Target, run: &Run) -> u64 {
    run.exchanges
        .iter()
        .map(|e| {
            let u = e.response.body.get("usage").unwrap();
            let n = |k: &str| u.get(k).unwrap().as_u64().unwrap();
            match target {
                Target::Anthropic => n("input_tokens") + n("output_tokens"),
                Target::OpenAi | Target::Ollama => n("total_tokens"),
            }
        })
        .sum()
}

/// The final reply's text blocks, joined.
fn final_text(run: &Run) -> String {
    run.reply()
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The calculator's answers, in call order.
fn answers(run: &Run) -> Vec<String> {
    run.calls.iter().map(calc::answer).collect()
}

/// The loop ended on `end_turn`; the calculator was called and every call
/// in the transcript has a non-error result; the wire saw one request per
/// assistant turn plus the last; the kernel's `Calls` and `Tokens` are
/// exactly what happened and what the provider reported.
fn shape(target: Target, run: &Run) {
    assert_eq!(run.reply().stop, StopReason::EndTurn, "{:?}", run.reply());
    assert!(!run.calls.is_empty(), "the calculator was never called");
    let (mut tool_calls, mut tool_results) = (0usize, 0usize);
    for m in &run.transcript.messages {
        for c in &m.content {
            match c {
                Content::ToolCall { .. } => tool_calls += 1,
                Content::ToolResult { is_error, .. } => {
                    assert!(!is_error, "{c:?}");
                    tool_results += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(tool_calls, tool_results, "every tool call has a result");
    assert_eq!(
        tool_calls,
        run.calls.len(),
        "every tool call reached the calculator"
    );
    let turns = run
        .transcript
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    assert_eq!(
        run.sent.len(),
        turns + 1,
        "one request per assistant turn, plus the last"
    );
    assert_eq!(
        run.exchanges.len(),
        run.sent.len(),
        "the relay saw every request"
    );
    assert_eq!(
        run.spent.get(&DimKey::Calls),
        Some(&((run.sent.len() + run.calls.len()) as u64)),
        "model calls plus tool calls"
    );
    assert_eq!(
        run.spent.get(&DimKey::Tokens),
        Some(&(relayed_usage(target, run) + run.calls.len() as u64 * CALC_TOKENS)),
        "the kernel billed what the provider reported, plus the calculator"
    );
}

/// [`shape`], and the final text carries the last calculator answer.
fn round_trip(target: Target, run: &Run) {
    shape(target, run);
    let text = final_text(run);
    let last = answers(run).pop().unwrap();
    assert!(
        text.contains(&last),
        "the final text does not carry the calculator's answer {last}: {text:?}"
    );
}

/// [`shape`], and both expressions were answered. Whether the model put
/// both calls in one turn is its choice, reported and not asserted.
fn parallel(target: Target, run: &Run) {
    shape(target, run);
    let got = answers(run);
    for want in ["4", "6"] {
        assert!(
            got.iter().any(|a| a == want),
            "no call answered {want}: {got:?}"
        );
    }
    let first_turn = run
        .transcript
        .messages
        .iter()
        .find(|m| m.role == Role::Assistant)
        .map_or(0, |m| {
            m.content
                .iter()
                .filter(|c| matches!(c, Content::ToolCall { .. }))
                .count()
        });
    eprintln!("  parallel: {first_turn} calls in the first turn");
}

/// [`shape`], and the first assistant turn opens with a signed thinking
/// block that the second request carried back; the provider accepting
/// that request is the proof the signature round-tripped.
fn thinking(target: Target, run: &Run) {
    shape(target, run);
    let assistant = run
        .transcript
        .messages
        .iter()
        .find(|m| m.role == Role::Assistant)
        .unwrap();
    assert!(
        matches!(assistant.content.first(), Some(Content::Thinking { .. })),
        "{:?}",
        assistant.content
    );
    let signature = run
        .sent
        .get(1)
        .unwrap()
        .json()
        .pointer("/messages/1/content/0/signature")
        .and_then(Value::as_str)
        .map(str::to_owned);
    assert!(
        signature.is_some_and(|s| !s.is_empty()),
        "the second request carries no thinking signature"
    );
}

/// Exactly the driver's token ceiling: the first reserve fits, the settle
/// leaves `ceiling - usage - 1` after the calculator, and the second
/// reserve is refused, whatever the usage was.
fn ceiling_only(ceiling: &Budget) -> Budget {
    Budget::from_dims([
        (DimKey::Tokens, ceiling.get(&DimKey::Tokens).unwrap()),
        (DimKey::CostMicroUsd, 10_000_000),
        (DimKey::Calls, 16),
    ])
}

/// The first model call ran and the tool answered, then the second `send`
/// was refused on budget: terminal, one request on the wire, nothing
/// retried.
fn budget_refusal(_target: Target, run: &Run) {
    assert!(
        matches!(run.result, Err(ToolLoopError::Budget(_))),
        "{:?}",
        run.result
    );
    assert_eq!(run.sent.len(), 1, "one request on the wire");
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

// --- the rows, cheapest model first ---------------------------------------

fn program(user: &'static str, max_tokens: u32, budget: fn(&Budget) -> Budget) -> Program {
    Program {
        user,
        max_tokens,
        budget,
    }
}

/// The three rows every provider runs: round trip, parallel calls, budget
/// refusal.
fn common_rows(model: &str, max_tokens: u32, make: impl Fn() -> MakeModel) -> Vec<Row> {
    vec![
        Row {
            name: "tool_result_round_trip",
            model: model.to_owned(),
            make: make(),
            program: program(CALC_PROMPT, max_tokens, ample),
            check: round_trip,
        },
        Row {
            name: "parallel_tool_calls_round_trip",
            model: model.to_owned(),
            make: make(),
            program: program(PARALLEL_PROMPT, 512, ample),
            check: parallel,
        },
        Row {
            name: "budget_refusal",
            model: model.to_owned(),
            make: make(),
            program: program(CALC_PROMPT, max_tokens, ceiling_only),
            check: budget_refusal,
        },
    ]
}

#[tokio::test]
#[ignore = "needs TAU_ANTHROPIC_LIVE=1 and ANTHROPIC_API_KEY; costs money"]
async fn anthropic_live() {
    use tau_drivers::model::anthropic::{ApiKey, API_KEY_ENV};
    if !enabled("TAU_ANTHROPIC_LIVE") {
        return;
    }
    let key = ApiKey::from_env(API_KEY_ENV).expect("ANTHROPIC_API_KEY is set");
    let haiku = || e2e::anthropic(HAIKU, key.clone());
    let mut rows = common_rows(HAIKU, 256, haiku);
    rows.push(Row {
        name: "thinking_on_replayed_second_turn",
        model: OPUS.to_owned(),
        make: e2e::anthropic(OPUS, key.clone()),
        program: program(THINKING_PROMPT, 4096, ample),
        check: thinking,
    });
    run_rows(Target::Anthropic, rows).await;
}

#[tokio::test]
#[ignore = "needs TAU_OPENAI_LIVE=1 and OPENAI_API_KEY; costs money"]
async fn openai_live() {
    use tau_drivers::model::openai::{ApiKey, API_KEY_ENV};
    if !enabled("TAU_OPENAI_LIVE") {
        return;
    }
    let key = ApiKey::from_env(API_KEY_ENV).expect("OPENAI_API_KEY is set");
    let mini = || e2e::openai(MINI.to_owned(), Some(key.clone()), false);
    run_rows(Target::OpenAi, common_rows(MINI, 256, mini)).await;
}

#[tokio::test]
#[ignore = "needs TAU_OLLAMA_LIVE=1 and a local Ollama"]
async fn ollama_live() {
    if !enabled("TAU_OLLAMA_LIVE") {
        return;
    }
    let base = Target::Ollama.live_base_url();
    if reqwest::get(format!("{base}/api/tags")).await.is_err() {
        panic!("ollama is not reachable at {base}");
    }
    let model = std::env::var("TAU_OLLAMA_MODEL").unwrap_or_else(|_| QWEN.to_owned());
    let qwen = || e2e::openai(model.clone(), None, true);
    run_rows(Target::Ollama, common_rows(&model, 512, qwen)).await;
}
