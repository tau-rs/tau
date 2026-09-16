//! One capability probe per feature, on every model the provider lists.
//!
//! The matrix files pin the driver's behaviour on two chosen models; this
//! file answers the other question — *which* models take thinking off, take
//! a temperature, or want `max_completion_tokens` — by recording one small
//! call per (model, feature) pair and rendering the answers into
//! `cassettes/MODELS.md`. Every probe expects [`Expect::Policy`]: a 400 is
//! evidence about the model, not a failed test. `TAU_RECORD=1` re-records,
//! under a cost cap; replay is offline like every other cassette test.

#![cfg(all(feature = "anthropic", feature = "openai"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use common::cassette::{self, Cassette};
use common::scenario::{self, Build, Expect, Make, Scenario, Step, Target};
use serde_json::{json, Value};
use tau_drivers::model::anthropic::{
    AnthropicConfig, AnthropicDriver, ApiKey as AnthropicKey, SamplingMode, ThinkingMode,
    API_KEY_ENV as ANTHROPIC_KEY_ENV,
};
use tau_drivers::model::openai::{
    ApiKey as OpenAiKey, OpenAiConfig, OpenAiDriver, OutputCap, API_KEY_ENV as OPENAI_KEY_ENV,
};
use tau_kernel::abi::{DimKey, Name};
use tau_kernel::bridge::{Content, Message, ModelRequest, Role, Sampling, ToolDef, VERSION};
use tau_kernel::driver::Driver;

// --- probes ---

/// One feature, asked of one model with one call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Probe {
    /// Plain text in, text out.
    Text,
    /// A tool definition in, a tool call out.
    ToolCall,
    /// A request carrying `temperature`.
    SamplingPresent,
    /// Anthropic: config `thinking: Disabled`.
    ThinkingDisabled,
    /// OpenAI: config `output_cap: MaxCompletionTokens`.
    MaxCompletionTokens,
}

impl Probe {
    /// The name in a cassette file name. Snake case, stable.
    fn kind(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::ToolCall => "tool_call",
            Self::SamplingPresent => "sampling_present",
            Self::ThinkingDisabled => "thinking_disabled",
            Self::MaxCompletionTokens => "max_completion_tokens",
        }
    }

    /// The column header in `MODELS.md`.
    fn column(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::ToolCall => "tool call",
            Self::SamplingPresent => "sampling present",
            Self::ThinkingDisabled => "thinking disabled",
            Self::MaxCompletionTokens => "max_completion_tokens",
        }
    }

    fn parse(kind: &str) -> Self {
        match kind {
            "text" => Self::Text,
            "tool_call" => Self::ToolCall,
            "sampling_present" => Self::SamplingPresent,
            "thinking_disabled" => Self::ThinkingDisabled,
            "max_completion_tokens" => Self::MaxCompletionTokens,
            other => panic!("unknown probe kind {other}"),
        }
    }

    /// The probes that make sense for a target, in column order.
    fn for_target(target: Target) -> Vec<Self> {
        match target {
            Target::Anthropic => vec![
                Self::Text,
                Self::ToolCall,
                Self::SamplingPresent,
                Self::ThinkingDisabled,
            ],
            Target::OpenAi => vec![
                Self::Text,
                Self::ToolCall,
                Self::SamplingPresent,
                Self::MaxCompletionTokens,
            ],
            Target::Ollama => vec![],
        }
    }
}

/// The cassette name for a probe: `probe_<kind>__<model>`, with the two
/// characters a model id may carry that a file name should not (`:`, `/`)
/// flattened to `-`.
fn probe_name(p: Probe, model: &str) -> String {
    format!("probe_{}__{}", p.kind(), model.replace([':', '/'], "-"))
}

fn anthropic_key() -> AnthropicKey {
    if scenario::record_enabled() {
        AnthropicKey::from_env(ANTHROPIC_KEY_ENV).expect("ANTHROPIC_API_KEY")
    } else {
        AnthropicKey::new("replay")
    }
}

fn openai_key() -> Option<OpenAiKey> {
    Some(if scenario::record_enabled() {
        OpenAiKey::from_env(OPENAI_KEY_ENV).expect("OPENAI_API_KEY")
    } else {
        OpenAiKey::new("replay")
    })
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

fn calculator() -> ToolDef {
    ToolDef {
        name: Name::new("calculator").unwrap(),
        description: "Evaluates an arithmetic expression.".into(),
        input_schema: json!({"type":"object","properties":{"expression":{"type":"string"}},"required":["expression"],"additionalProperties":false}),
    }
}

fn with_calc(mut req: ModelRequest) -> ModelRequest {
    req.tools = vec![calculator()];
    req
}

/// The one request a probe sends. Small on purpose: the cap is real money.
fn request(p: Probe) -> ModelRequest {
    match p {
        Probe::Text => text("Reply with the single word: pong.", 64),
        Probe::ToolCall => with_calc(text("What is 17*23? Use the calculator tool.", 512)),
        Probe::SamplingPresent => {
            let mut req = text("pong?", 32);
            req.sampling = Some(Sampling {
                temperature: Some(0.2),
                ..Sampling::default()
            });
            req
        }
        Probe::ThinkingDisabled => text("pong?", 64),
        Probe::MaxCompletionTokens => text("pong?", 32),
    }
}

/// The scenario and driver factory for one probe on one model.
///
/// `Scenario::name` and `AnthropicConfig::model` outlive the call, so the
/// model id is leaked: the count is bounded by (models × probes) of one
/// record or replay run, in a test binary.
fn scenario_for(target: Target, model: String, p: Probe) -> (Scenario, Make) {
    let name: &'static str = Box::leak(probe_name(p, &model).into_boxed_str());
    let model: &'static str = Box::leak(model.into_boxed_str());
    let (input, output) = price(target, model);
    let make: Make = match target {
        Target::Anthropic => Box::new(move |base: &str| {
            let mut cfg =
                AnthropicConfig::new(model, anthropic_key(), 16_000, 4_096, input, output);
            cfg.base_url = base.to_owned();
            if p == Probe::SamplingPresent {
                cfg.sampling = SamplingMode::Accepted;
            }
            if p == Probe::ThinkingDisabled {
                cfg.thinking = ThinkingMode::Disabled;
            }
            Box::new(AnthropicDriver::new(cfg).unwrap()) as Box<dyn Driver>
        }),
        Target::OpenAi => Box::new(move |base: &str| {
            let mut cfg = OpenAiConfig::new(model, openai_key(), 16_000, 4_096, input, output);
            cfg.base_url = base.to_owned();
            if p == Probe::MaxCompletionTokens {
                cfg.output_cap = OutputCap::MaxCompletionTokens;
            }
            Box::new(OpenAiDriver::new(cfg).unwrap()) as Box<dyn Driver>
        }),
        Target::Ollama => panic!("probes cover anthropic and openai only"),
    };
    let build: Build = Box::new(move |_| request(p));
    (
        Scenario {
            name,
            target,
            model,
            steps: vec![Step {
                build,
                expect: Expect::Policy,
            }],
        },
        make,
    )
}

// --- models endpoint ---

/// Which provider owns a model id. Every Anthropic id begins `claude-`;
/// everything else in this file is OpenAI's. Used to keep one target's
/// inventory intact when only the other is being re-recorded.
fn target_of(model: &str) -> Target {
    if model.starts_with("claude-") {
        Target::Anthropic
    } else {
        Target::OpenAi
    }
}

/// Input and output price in µUSD per token. Only the ratio between models
/// matters here: it is what makes the cap bite on the expensive ones first,
/// and what the recorded `cost_microusd` is summed from.
fn price(target: Target, model: &str) -> (u64, u64) {
    match target {
        Target::Anthropic => {
            if model.starts_with("claude-fable") {
                (10, 50)
            } else if model.starts_with("claude-opus") {
                (5, 25)
            } else if model.starts_with("claude-sonnet-5") {
                (2, 10)
            } else if model.starts_with("claude-sonnet") {
                (3, 15)
            } else if model.starts_with("claude-haiku") {
                (1, 5)
            } else {
                (5, 25)
            }
        }
        _ if model.contains("pro") => (20, 100),
        _ => (1, 4),
    }
}

/// Model ids that are not chat models, by substring.
const NOT_CHAT: [&str; 11] = [
    "audio",
    "realtime",
    "image",
    "tts",
    "transcribe",
    "search",
    "embedding",
    "moderation",
    "instruct",
    "codex",
    "live",
];

/// `Some(id without the suffix)` when `id` ends in `-YYYY-MM-DD`.
fn undated(id: &str) -> Option<&str> {
    let cut = id.len().checked_sub(11)?;
    let tail = id.get(cut..)?;
    if tail.chars().count() != 11 {
        return None;
    }
    let shaped = tail.chars().zip("-dddd-dd-dd".chars()).all(|(c, want)| {
        if want == 'd' {
            c.is_ascii_digit()
        } else {
            c == want
        }
    });
    shaped.then(|| id.get(..cut)).flatten()
}

/// Whether an OpenAI id is one of the chat models worth a probe: a `gpt-` or
/// `o<digit>` id that is not a modality-specific endpoint, not a
/// `*-chat-latest` moving alias, and not a dated id whose undated alias the
/// provider also lists (probing both would pay twice for one model).
fn chat_capable(id: &str, all: &BTreeSet<String>) -> bool {
    let mut chars = id.chars();
    let shaped = id.starts_with("gpt-")
        || (chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit()));
    shaped
        && !NOT_CHAT.iter().any(|bad| id.contains(bad))
        && !id.ends_with("-chat-latest")
        && !undated(id).is_some_and(|base| all.contains(base))
}

/// The models the provider lists, in the order probes should spend money on
/// them: Anthropic cheapest first, OpenAI alphabetical with the `pro` models
/// last, since those are the ones that would eat the cap.
async fn list_models(target: Target, key: &str) -> Vec<String> {
    let client = reqwest::Client::new();
    let base = format!("{}/v1/models", target.live_base_url());
    let req = match target {
        Target::Anthropic => client
            .get(format!("{base}?limit=100"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        _ => client
            .get(base)
            .header("authorization", format!("Bearer {key}")),
    };
    let body: Value = req
        .send()
        .await
        .expect("models request")
        .json()
        .await
        .expect("models response is JSON");
    let all: Vec<String> = body
        .get("data")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no data array in {body}"))
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(ToOwned::to_owned))
        .collect();
    match target {
        Target::Anthropic => {
            let mut ids = all;
            ids.sort_by(|a, b| {
                price(target, a)
                    .0
                    .cmp(&price(target, b).0)
                    .then_with(|| a.cmp(b))
            });
            ids
        }
        _ => {
            let set: BTreeSet<String> = all.iter().cloned().collect();
            let mut ids: Vec<String> = all
                .into_iter()
                .filter(|id| chat_capable(id, &set))
                .collect();
            ids.sort_by(|a, b| {
                a.contains("pro")
                    .cmp(&b.contains("pro"))
                    .then_with(|| a.cmp(b))
            });
            ids
        }
    }
}

// --- MODELS.md ---

const TITLE: &str = "# Provider model capability table";
const RENDERED_FROM: &str = "Rendered from `drivers/tests/cassettes/*/probe_*.json` by `probes::render_models_md`; do not edit by hand.";
const LEGEND: &str = "A cell is `ok` when the recorded response was 2xx, `<status> <the start of the provider's message>` when it was not, and `—` where no cassette exists. OpenAI's gpt-5 and o-series reject the default `max_tokens` field with a 400 asking for `max_completion_tokens`, so on those models `text`, `tool call`, and `sampling present` all read 400 while `max_completion_tokens` reads `ok`: the same default cap field is sent on all three probes, so the request fails before the temperature question is ever reached, and that is the policy this table exists to record, not a driver bug. A model the provider retires keeps its cassettes and its row here, and is named under `retired` below, until someone deletes the files by hand.";
const INVENTORY: &str = "## inventory (at last record)";
const RETIRED_LINE: &str = "- retired (cassette, no longer listed): ";
const UNPROBED_LINE: &str = "- unprobed (listed, no cassette): ";

/// The target a cassette directory (or a cassette's `target` field) names.
fn target_named(name: &str) -> Option<Target> {
    match name {
        "anthropic" => Some(Target::Anthropic),
        "openai" => Some(Target::OpenAi),
        "ollama" => Some(Target::Ollama),
        _ => None,
    }
}

/// The model id a probe was *configured* with: the `model` of the request it
/// sent, not of the reply it got. OpenAI answers a dated id
/// (`gpt-4.1-mini-2025-04-14`) to a request for the alias, so the reply's id
/// is neither what a replay must send nor what `/v1/models` lists; the
/// request's id is both.
fn probe_model(c: &Cassette) -> Option<String> {
    c.exchanges
        .first()
        .and_then(|e| e.request.body.get("model"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| c.model.clone())
}

/// Which probe a cassette holds, read back from the feature its request
/// exercises — a `Cassette` does not carry its scenario name. Each probe is
/// defined by exactly one thing on the wire: tools, `temperature`,
/// `thinking`, `max_completion_tokens`, or none of them.
fn probe_of(c: &Cassette) -> Option<Probe> {
    let target = target_named(&c.target)?;
    let body = &c.exchanges.first()?.request.body;
    let present = |field: &str| body.get(field).is_some_and(|v| !v.is_null());
    if body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|t| !t.is_empty())
    {
        return Some(Probe::ToolCall);
    }
    if present("temperature") {
        return Some(Probe::SamplingPresent);
    }
    if target == Target::Anthropic && present("thinking") {
        return Some(Probe::ThinkingDisabled);
    }
    if target == Target::OpenAi && present("max_completion_tokens") {
        return Some(Probe::MaxCompletionTokens);
    }
    Some(Probe::Text)
}

/// What one probe's cassette says about the model: `ok`, or the status and
/// the start of the provider's complaint.
fn cell(c: &Cassette) -> String {
    let Some(response) = c.exchanges.first().map(|e| &e.response) else {
        return "—".to_owned();
    };
    if (200..300).contains(&response.status) {
        return "ok".to_owned();
    }
    let message = response
        .body
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| response.body.to_string());
    format!("{} {}", response.status, clip(&message))
}

/// The first 40 characters, with the characters that would break a markdown
/// table turned into spaces.
fn clip(message: &str) -> String {
    message
        .chars()
        .take(40)
        .map(|c| {
            if c == '|' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn bullet(prefix: &str, ids: &[String]) -> String {
    if ids.is_empty() {
        format!("{prefix}none\n")
    } else {
        format!("{prefix}{}\n", ids.join(", "))
    }
}

/// The table, from the cassettes alone. Pure and order-independent: models
/// sort by id inside a target, targets are Anthropic then OpenAI, so two
/// runs over the same cassettes render the same bytes.
fn render_models_md(cassettes: &[Cassette], retired: &[String], unprobed: &[String]) -> String {
    let mut cells: BTreeMap<(String, String, &'static str), String> = BTreeMap::new();
    for c in cassettes {
        let (Some(model), Some(p)) = (probe_model(c), probe_of(c)) else {
            continue;
        };
        cells.insert((c.target.clone(), model, p.kind()), cell(c));
    }

    let mut out = format!("{TITLE}\n\n{RENDERED_FROM}\n\n{LEGEND}\n");
    for target in [Target::Anthropic, Target::OpenAi] {
        let probes = Probe::for_target(target);
        let dir = target.dir_name();
        out.push_str(&format!("\n## {dir}\n\n| model"));
        for p in &probes {
            out.push_str(&format!(" | {}", p.column()));
        }
        out.push_str(" |\n");
        out.push_str(&"|---".repeat(probes.len() + 1));
        out.push_str("|\n");
        let models: BTreeSet<&String> = cells
            .keys()
            .filter(|(t, _, _)| t == dir)
            .map(|(_, model, _)| model)
            .collect();
        for model in models {
            out.push_str(&format!("| {model}"));
            for p in &probes {
                let key = (dir.to_owned(), model.clone(), p.kind());
                let value = cells.get(&key).map_or("—", String::as_str);
                out.push_str(&format!(" | {value}"));
            }
            out.push_str(" |\n");
        }
    }
    out.push_str(&format!("\n{INVENTORY}\n\n"));
    out.push_str(&bullet(RETIRED_LINE, retired));
    out.push_str(&bullet(UNPROBED_LINE, unprobed));
    out
}

/// The two inventory bullets, read back: the one part of `MODELS.md` the
/// cassettes on disk cannot tell you, since a retired model is one whose
/// cassette is gone and an unprobed one never had a cassette at all.
fn inventory_from(md: &str) -> (Vec<String>, Vec<String>) {
    let list = |prefix: &str| -> Vec<String> {
        md.lines()
            .find_map(|line| line.strip_prefix(prefix))
            .filter(|rest| *rest != "none")
            .map(|rest| rest.split(", ").map(ToOwned::to_owned).collect())
            .unwrap_or_default()
    };
    (list(RETIRED_LINE), list(UNPROBED_LINE))
}

/// Every `probe_*` cassette on disk, in `all_files` order.
fn probe_cassettes() -> Vec<Cassette> {
    cassette::all_files()
        .iter()
        .filter(|f| {
            f.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("probe_"))
        })
        .map(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()))
        })
        .collect()
}

/// The path of the rendered table.
fn models_md() -> PathBuf {
    cassette::dir().join("MODELS.md")
}

/// The two inventory bullets as the file on disk has them. Empty when there
/// is no file yet.
fn inventory_on_disk() -> (Vec<String>, Vec<String>) {
    inventory_from(&std::fs::read_to_string(models_md()).unwrap_or_default())
}

/// Renders the table from the probe cassettes on disk plus the two bullets,
/// and writes it. The only writer of `MODELS.md`, so a render-only refresh
/// and the end of a recording run cannot drift apart.
fn write_models_md(retired: &[String], unprobed: &[String]) {
    let rendered = render_models_md(&probe_cassettes(), retired, unprobed);
    std::fs::write(models_md(), rendered).expect("write MODELS.md");
}

/// Every `probe_*` cassette sitting in one target's directory.
fn probe_files(target: Target) -> Vec<PathBuf> {
    cassette::all_files()
        .into_iter()
        .filter(|f| {
            let in_dir = f
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|d| d.to_str())
                == Some(target.dir_name());
            let is_probe = f
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("probe_"));
            in_dir && is_probe
        })
        .collect()
}

/// Replays one probe cassette through a driver configured exactly as the
/// recording was: same model, same knobs, so the request the stub captures
/// must equal the request on disk byte for byte.
async fn replay_probe(file: PathBuf) {
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| panic!("{}: no stem", file.display()))
        .to_owned();
    let rest = stem
        .strip_prefix("probe_")
        .unwrap_or_else(|| panic!("{stem}: not a probe cassette"));
    let (kind, model_slug) = rest
        .split_once("__")
        .unwrap_or_else(|| panic!("{stem}: no __ between probe and model"));
    let c: Cassette = serde_json::from_str(&std::fs::read_to_string(&file).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let target = target_named(&c.target)
        .unwrap_or_else(|| panic!("{}: unknown target {}", file.display(), c.target));
    let model = probe_model(&c).unwrap_or_else(|| model_slug.to_owned());
    let (s, make) = scenario_for(target, model, Probe::parse(kind));
    scenario::replay_from(&s, target.dir_name(), make).await;
}

/// Replays every probe cassette of one target, all at once.
///
/// One probe is one stub round-trip over a real socket; a hundred and
/// eighty of them in sequence overran the quick profile's 5 s ceiling, and
/// they are independent — each `replay_from` starts its own stub — so they
/// run as spawned tasks. A panicking replay is resumed on this thread with
/// its payload intact, so the failure still names the scenario.
async fn replay_all(target: Target) {
    let files = probe_files(target);
    let n = files.len();
    assert!(
        n > 0,
        "no probe cassettes for {}; run `just live record probes`",
        target.dir_name()
    );
    let mut handles = Vec::with_capacity(n);
    for file in files {
        handles.push(tokio::spawn(replay_probe(file)));
    }
    for handle in handles {
        if let Err(joined) = handle.await {
            match joined.try_into_panic() {
                Ok(payload) => std::panic::resume_unwind(payload),
                Err(e) => panic!("replay task did not finish: {e}"),
            }
        }
    }
    eprintln!("{n} {} probe cassettes replayed", target.dir_name());
}

// --- tests ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anthropic_probe_cassettes_replay() {
    replay_all(Target::Anthropic).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn openai_probe_cassettes_replay() {
    replay_all(Target::OpenAi).await;
}

/// The two inventory bullets are read from the file under test and fed back
/// into the renderer (`inventory_from`, above), so they are the one part of
/// `MODELS.md` this test does not verify — the one fact the cassettes on
/// disk cannot re-derive on their own.
#[test]
fn models_md_is_what_the_cassettes_render_to() {
    let cassettes = probe_cassettes();
    let on_disk = std::fs::read_to_string(models_md()).unwrap_or_default();
    let (retired, unprobed) = inventory_from(&on_disk);
    assert_eq!(
        on_disk,
        render_models_md(&cassettes, &retired, &unprobed),
        "MODELS.md is stale; run `just live record`"
    );
}

/// Re-renders `MODELS.md` from the cassettes already on disk, keeping the
/// inventory bullets the file already carries. No provider call and no
/// money: this is the refresh for when the *renderer* changes — a new
/// column, a reworded legend — as opposed to when the evidence changes.
#[test]
#[ignore = "rewrites drivers/tests/cassettes/MODELS.md"]
fn rewrite_models_md_from_disk() {
    let (retired, unprobed) = inventory_on_disk();
    write_models_md(&retired, &unprobed);
    eprintln!("MODELS.md re-rendered; retired={retired:?} unprobed={unprobed:?}");
}

#[tokio::test]
#[ignore = "TAU_RECORD=1 and provider keys; costs money"]
async fn record_probes() {
    if !scenario::record_enabled() {
        eprintln!("TAU_RECORD is not 1; skipping");
        return;
    }
    let cap: u64 = std::env::var("TAU_RECORD_CAP_MICROUSD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3_000_000);
    let only = std::env::var("TAU_RECORD_TARGET").ok();
    let mut spent = 0u64;
    // A scoped run learns nothing about the other provider, and the two
    // bullets are one file: start from what the file says about the targets
    // this run will not visit, or the recovery step of a capped OpenAI run
    // (`TAU_RECORD_TARGET=anthropic`) would quietly erase the `unprobed`
    // list it exists to work through. A whole-world run starts from nothing.
    let (mut retired, mut unprobed) = match only.as_deref() {
        None => (Vec::new(), Vec::new()),
        Some(o) => {
            let (was_retired, was_unprobed) = inventory_on_disk();
            let elsewhere = |ids: Vec<String>| -> Vec<String> {
                ids.into_iter()
                    .filter(|m| target_of(m).dir_name() != o)
                    .collect()
            };
            (elsewhere(was_retired), elsewhere(was_unprobed))
        }
    };
    for target in [Target::Anthropic, Target::OpenAi] {
        if only.as_deref().is_some_and(|o| o != target.dir_name()) {
            continue;
        }
        let env_name = match target {
            Target::Anthropic => ANTHROPIC_KEY_ENV,
            _ => OPENAI_KEY_ENV,
        };
        let key = std::env::var(env_name).unwrap_or_else(|_| panic!("{env_name} is not set"));
        let listed = list_models(target, &key).await;
        let probed: BTreeSet<String> = probe_cassettes()
            .iter()
            .filter(|c| c.target == target.dir_name())
            .filter_map(probe_model)
            .collect();
        retired.extend(probed.into_iter().filter(|m| !listed.contains(m)));
        let mut first = true;
        'models: for model in &listed {
            for probe in Probe::for_target(target) {
                if spent >= cap {
                    eprintln!("cap {cap} µUSD reached before {model}; stopping");
                    unprobed.extend(listed.iter().skip_while(|m| *m != model).cloned());
                    break 'models;
                }
                let (s, make) = scenario_for(target, model.clone(), probe);
                let name = s.name;
                let consumed = scenario::record(&s, make).await;
                spent += consumed.get(&DimKey::CostMicroUsd).unwrap_or(0);
                if first {
                    // A wrong key answers 401 to everything, and `Policy`
                    // accepts a provider error, so the whole run would
                    // "pass" and record a table of 401s. Stop on the first.
                    first = false;
                    let c = cassette::load(target.dir_name(), name).expect("just recorded");
                    if c.exchanges.first().map(|e| e.response.status) == Some(401) {
                        panic!("key rejected: {env_name}");
                    }
                }
            }
        }
    }
    write_models_md(&retired, &unprobed);
    eprintln!("probes recorded; {spent} µUSD; retired={retired:?} unprobed={unprobed:?}");
}
