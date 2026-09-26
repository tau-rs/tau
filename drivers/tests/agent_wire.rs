//! The agent wire against ADR-0013's JSON: the ADR's examples round-trip
//! through the types in value, `describe()` is the ADR's projection, the
//! ceiling is §6's, and every row of the `unsupported` table is refused
//! before anything is spawned.

#![cfg(all(feature = "agent", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;

use std::time::Duration;

use serde_json::{json, Value};
use tau_drivers::agent::process::{Ending, Run, Rung};
use tau_drivers::agent::wire::{
    self, Budget, Caps, ErrorKind, Limit, Op, Reply, Request, RunError, Stop, Truncated, Usage,
    VERSION,
};
use tau_drivers::agent::{
    accept, bill, decode, encode, refusal, refused, settle, AgentConfig, Billing, ConfigError,
    Flights, Outcome,
};
use tau_kernel::abi::{Corr, DimKey};

/// The workspace root the ADR's own request is written against.
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the drivers crate has a parent")
        .to_path_buf()
}

const REQUEST_RUN: &str = include_str!("fixtures/agent/request-run.json");
const REQUEST_RESUME: &str = include_str!("fixtures/agent/request-resume.json");
const REPLY: &str = include_str!("fixtures/agent/reply.json");
const REPLY_UNAVAILABLE: &str = include_str!("fixtures/agent/reply-unavailable.json");
const DESCRIBE: &str = include_str!("fixtures/agent/describe.json");
const DESCRIBE_RUN_ONLY: &str = include_str!("fixtures/agent/describe-run-only.json");

fn roundtrip<T: serde::de::DeserializeOwned + serde::Serialize>(fixture: &str) -> T {
    let expected: Value = serde_json::from_str(fixture).unwrap();
    let typed: T = serde_json::from_value(expected.clone()).unwrap();
    let back = serde_json::to_value(&typed).unwrap();
    assert_eq!(back, expected, "the fixture and the types diverged");
    typed
}

#[test]
fn the_request_fixtures_round_trip() {
    let run: Request = roundtrip(REQUEST_RUN);
    assert_eq!(run.op(), Op::Run);
    assert_eq!(run.v(), Some(VERSION));
    assert_eq!(run.workspace(), Some("kernel"));
    assert_eq!(run.tools().unwrap(), ["Read", "Edit", "Bash"]);
    assert_eq!(run.budget().unwrap().turns, Some(40));
    assert_eq!(run.session(), None);

    let resume: Request = roundtrip(REQUEST_RESUME);
    assert_eq!(resume.op(), Op::Resume);
    assert_eq!(
        resume.session(),
        Some("affe155e-9ed1-4477-95f0-c17d40bd9a89")
    );
    assert_eq!(resume.tools(), None, "a resume narrows nothing");
    assert_eq!(resume.budget(), None);
}

#[test]
fn an_absent_v_is_the_projected_version_and_a_stranger_field_is_refused() {
    let bare: Request = serde_json::from_str(r#"{"op":"run","task":"x"}"#).unwrap();
    assert_eq!(bare.v(), None, "absent means the version describe() showed");
    assert_eq!(bare.workspace(), None, "absent is the root itself");
    // A field the schema does not have is refused, never silently dropped:
    // `model` is the harness's decision, not the request's.
    assert!(serde_json::from_str::<Request>(r#"{"op":"run","task":"x","model":"opus"}"#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"op":"fly","task":"x"}"#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"op":"resume","task":"x"}"#).is_err());
}

#[test]
fn the_reply_fixtures_round_trip() {
    let reply: Reply = roundtrip(REPLY);
    assert_eq!(reply.stop, Stop::Done);
    assert_eq!(reply.mode.as_deref(), Some("none"), "verbatim and opaque");
    assert_eq!(reply.usage.tokens(), 44_125 + 381);
    assert_eq!(reply.transcript.len(), 5, "the CLI's own stream, unread");
    let envelope = reply.envelope.unwrap();
    assert_eq!(envelope.artifacts.len(), 2);
    assert!(envelope.error.is_none());

    let refused: Reply = roundtrip(REPLY_UNAVAILABLE);
    let Stop::Error(error) = refused.stop else {
        panic!("an error reply")
    };
    assert_eq!(error.kind, ErrorKind::Unavailable);
    assert!(error.message.contains("codex login status: exit 1"));
    assert_eq!(refused.usage, Usage::default());
    assert!(refused.envelope.is_none(), "no envelope to parse");
}

#[test]
fn every_stop_shape_is_the_adr_s() {
    let shapes = [
        (Stop::Done, r#""done""#),
        (Stop::Limit(Limit::Turns), r#"{"limit":"turns"}"#),
        (Stop::Limit(Limit::Cost), r#"{"limit":"cost"}"#),
        (Stop::Limit(Limit::Wall), r#"{"limit":"wall"}"#),
        (Stop::Abandoned, r#""abandoned""#),
        (
            Stop::Error(RunError {
                kind: ErrorKind::Lost,
                message: "gone".to_owned(),
            }),
            r#"{"error":{"kind":"lost","message":"gone"}}"#,
        ),
    ];
    for (stop, text) in shapes {
        assert_eq!(serde_json::to_string(&stop).unwrap(), text);
        assert_eq!(serde_json::from_str::<Stop>(text).unwrap(), stop);
    }
    for (kind, text) in [
        (ErrorKind::Unsupported, "unsupported"),
        (ErrorKind::Host, "host"),
        (ErrorKind::Unavailable, "unavailable"),
        (ErrorKind::Throttled, "throttled"),
        (ErrorKind::Provider, "provider"),
        (ErrorKind::Envelope, "envelope"),
        (ErrorKind::Lost, "lost"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(text));
    }
}

#[test]
fn describe_is_the_adr_projection() {
    let fixture: Value = serde_json::from_str(DESCRIBE).unwrap();
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        config.describe(Caps::ALL),
        fixture["description"].as_str().unwrap()
    );
    assert_eq!(wire::schema(Caps::ALL), fixture["input_schema"]);

    // The ADR's shape: one branch per op, each naming its `op` as a const
    // and closing its own `properties`, `v` nowhere. Nothing closes the
    // root: an `additionalProperties: false` there, beside a `oneOf`, has
    // no `properties` of its own and refuses every key (#193).
    let schema = &fixture["input_schema"];
    assert_eq!(schema["type"], json!("object"));
    assert!(schema.get("additionalProperties").is_none());
    let branches = schema["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0]["properties"]["op"]["const"], json!("run"));
    assert_eq!(branches[0]["required"], json!(["op", "task"]));
    assert_eq!(branches[0]["additionalProperties"], json!(false));
    assert_eq!(branches[1]["properties"]["op"]["const"], json!("resume"));
    assert_eq!(branches[1]["required"], json!(["op", "session", "task"]));
    assert_eq!(branches[1]["additionalProperties"], json!(false));
    assert!(
        !serde_json::to_string(schema).unwrap().contains("\"v\""),
        "a model never sets the wire version"
    );
}

#[test]
fn a_cli_that_would_refuse_a_field_never_shows_it() {
    // ADR-0013 §9: the codex adapter omits `tools`, `budget` and `resume`,
    // so a model never writes one — unsupported means refused, and a
    // silently ignored allowlist would be advice, not a cage.
    let fixture: Value = serde_json::from_str(DESCRIBE_RUN_ONLY).unwrap();
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        config.describe(Caps::RUN_ONLY),
        fixture["description"].as_str().unwrap()
    );
    assert!(
        !config.describe(Caps::RUN_ONLY).contains("Pass `session`"),
        "a CLI that cannot resume does not advertise resuming"
    );
    let schema = wire::schema(Caps::RUN_ONLY);
    assert_eq!(schema, fixture["input_schema"]);
    assert!(
        schema.get("oneOf").is_none(),
        "one op left, one flat object"
    );
    assert_eq!(
        schema["additionalProperties"],
        json!(false),
        "the one branch's closure lands on the root, beside its `properties`"
    );
    let properties = schema["properties"].as_object().unwrap();
    assert!(properties.contains_key("task") && properties.contains_key("workspace"));
    assert!(!properties.contains_key("tools"));
    assert!(!properties.contains_key("budget"));
}

#[test]
fn the_ceiling_is_the_task_bound() {
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    let ceiling = config.ceiling();
    assert_eq!(ceiling.get(&DimKey::Tokens), Some(400_000));
    assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(2_000_000));
    assert_eq!(ceiling.get(&DimKey::Calls), None, "the kernel adds calls");
    assert_eq!(
        ceiling.get(&DimKey::WallMs),
        None,
        "the reducer charges wall"
    );

    // No cost bound, but prices: the tokens at the dearer of the two.
    let mut priced = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    priced.task_cost_microusd = None;
    priced.input_price_microusd = Some(5);
    priced.output_price_microusd = Some(25);
    assert_eq!(
        priced.ceiling().get(&DimKey::CostMicroUsd),
        Some(400_000 * 25)
    );

    // Neither: a subscription has no per-token price, and a made-up one
    // would be a billing-mode assumption wearing a number.
    let mut unpriced = priced.clone();
    unpriced.input_price_microusd = None;
    unpriced.output_price_microusd = None;
    assert_eq!(unpriced.ceiling().get(&DimKey::CostMicroUsd), None);
    assert_eq!(unpriced.ceiling().get(&DimKey::Tokens), Some(400_000));
}

#[test]
fn the_description_names_the_bound_in_readable_units() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut config = AgentConfig::new("codex", "/bin/sh", root, 1_500_000, Duration::from_secs(60));
    assert!(
        config.describe(Caps::RUN_ONLY).starts_with(
            "Delegate a whole task to a codex session in the workspace. It runs headless, may use \
             up to 1.5M tokens per call"
        ),
        "{}",
        config.describe(Caps::RUN_ONLY)
    );
    config.task_cost_microusd = Some(500_000);
    assert!(config
        .describe(Caps::ALL)
        .contains("spend up to $0.50 (1.5M tokens)"));
    config.task_cost_microusd = Some(1_999_999);
    assert!(
        config.describe(Caps::ALL).contains("$2 ("),
        "a part-cent rounds up to the next dollar, never down"
    );
    config.tools = vec!["Read".to_owned()];
    assert!(config
        .describe(Caps::ALL)
        .contains("headless with tools Read,"));
}

#[test]
fn a_config_the_driver_cannot_honour_is_refused_at_registration() {
    let root = env!("CARGO_MANIFEST_DIR");
    agent::adr_config(root).check().unwrap();

    let mut no_name = agent::adr_config(root);
    no_name.name.clear();
    assert!(matches!(no_name.check(), Err(ConfigError::NoName)));

    let mut no_binary = agent::adr_config(root);
    no_binary.binary = "".into();
    assert!(matches!(no_binary.check(), Err(ConfigError::NoBinary)));

    let mut no_tokens = agent::adr_config(root);
    no_tokens.task_tokens = 0;
    assert!(matches!(
        no_tokens.check(),
        Err(ConfigError::ZeroBound {
            what: "task_tokens"
        })
    ));

    let mut no_wall = agent::adr_config(root);
    no_wall.wall = Duration::ZERO;
    assert!(matches!(
        no_wall.check(),
        Err(ConfigError::ZeroBound { what: "wall" })
    ));

    let mut no_root = agent::adr_config(root);
    no_root.workspace_root = std::path::Path::new(root).join("no-such-directory");
    let err = no_root.check().unwrap_err();
    assert!(matches!(err, ConfigError::WorkspaceRoot { .. }), "{err}");
    assert!(err.to_string().contains("no-such-directory"), "{err}");
}

/// Every row of ADR-0013 §2's `unsupported` table: refused, nothing spawned.
#[test]
fn what_the_driver_will_not_run_is_refused_before_anything_starts() {
    let root = env!("CARGO_MANIFEST_DIR");
    let config = agent::adr_config(root);
    let run = |value: Value| decode(&serde_json::to_vec(&value).unwrap(), &config, Caps::ALL);

    // A v this driver does not speak.
    let err = run(json!({"v": 99, "op": "run", "task": "x"})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("v1"), "{}", err.message);

    // Not a request at all.
    assert_eq!(
        decode(b"not json", &config, Caps::ALL).unwrap_err().kind,
        ErrorKind::Unsupported
    );

    // An op this CLI cannot serve (`codex exec resume` has no JSON stream).
    let err = accept(
        &serde_json::from_str::<Request>(REQUEST_RESUME).unwrap(),
        &config,
        Caps::RUN_ONLY,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("cannot resume"), "{}", err.message);

    // A task over the bound, and an empty one.
    let mut small = agent::adr_config(root);
    small.task_bytes = 8;
    let err = decode(
        br#"{"op":"run","task":"far too long for eight bytes"}"#,
        &small,
        Caps::ALL,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(err.message.contains("the bound is 8"), "{}", err.message);
    assert_eq!(
        run(json!({"op": "run", "task": "   "})).unwrap_err().kind,
        ErrorKind::Unsupported
    );

    // A tool outside the configured set, and a CLI with no allowlist at all.
    let err = run(json!({"op": "run", "task": "x", "tools": ["Read", "Fly"]})).unwrap_err();
    assert!(err.message.contains("`Fly`"), "{}", err.message);
    let err = accept(
        &serde_json::from_value::<Request>(json!({"op": "run", "task": "x", "tools": ["Read"]}))
            .unwrap(),
        &config,
        Caps {
            tools: false,
            ..Caps::ALL
        },
    )
    .unwrap_err();
    assert!(
        err.message.contains("no per-session tool allowlist"),
        "{}",
        err.message
    );

    // A budget above the bound, and a CLI that enforces none.
    let err = run(json!({"op": "run", "task": "x", "budget": {"turns": 41}})).unwrap_err();
    assert!(
        err.message.contains("above the configured bound 40"),
        "{}",
        err.message
    );
    let err = accept(
        &serde_json::from_value::<Request>(
            json!({"op": "run", "task": "x", "budget": {"turns": 1}}),
        )
        .unwrap(),
        &config,
        Caps {
            budget: false,
            ..Caps::ALL
        },
    )
    .unwrap_err();
    assert!(
        err.message.contains("enforces no per-run budget"),
        "{}",
        err.message
    );

    // A workspace that leaves the root, by climbing or by being absolute.
    for escape in ["../..", "/etc", "kernel/../../.."] {
        let err = run(json!({"op": "run", "task": "x", "workspace": escape})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported, "{escape}");
    }
    // A workspace that does not exist ran nothing either, and says so as a
    // host problem rather than a malformed request.
    let err = run(json!({"op": "run", "task": "x", "workspace": "no-such-dir"})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Host);
}

#[test]
fn a_symlink_out_of_the_workspace_root_is_refused() {
    let root = agent::Temp::new("root");
    let outside = agent::Temp::new("outside");
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    std::fs::create_dir(root.path().join("inside")).unwrap();
    let config = agent::adr_config(root.path());

    let accepted = decode(
        br#"{"op":"run","task":"x","workspace":"inside"}"#,
        &config,
        Caps::ALL,
    )
    .unwrap();
    assert!(accepted.workspace.ends_with("inside"));

    let err = decode(
        br#"{"op":"run","task":"x","workspace":"escape"}"#,
        &config,
        Caps::ALL,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert!(
        err.message.contains("outside the workspace root"),
        "{}",
        err.message
    );
}

#[test]
fn an_accepted_request_carries_the_narrowed_cage() {
    // The ADR's request names `kernel` as its workspace, so the root here is
    // the repository's, as a harness's would be.
    let root = repo_root();
    let config = agent::adr_config(&root);
    let accepted = accept(
        &serde_json::from_str::<Request>(REQUEST_RUN).unwrap(),
        &config,
        Caps::ALL,
    )
    .unwrap();
    assert_eq!(accepted.op, Op::Run);
    assert_eq!(accepted.tools, ["Read", "Edit", "Bash"]);
    assert_eq!(
        accepted.cost_microusd,
        Some(500_000),
        "the request narrowed"
    );
    assert_eq!(accepted.turns, Some(40), "and left the turns alone");
    assert!(accepted.workspace.ends_with("kernel"));

    // No request budget: the registration's bound, whole.
    let accepted = decode(br#"{"op":"run","task":"x"}"#, &config, Caps::ALL).unwrap();
    assert_eq!(accepted.cost_microusd, Some(2_000_000));
    assert_eq!(accepted.tools, config.tools, "the configured set");

    // A narrowing to one tool is a narrowing, not a replacement.
    let accepted = decode(
        br#"{"op":"run","task":"x","tools":["Read"]}"#,
        &config,
        Caps::ALL,
    )
    .unwrap();
    assert_eq!(accepted.tools, ["Read"]);
}

#[test]
fn a_budget_may_only_narrow() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut config = agent::adr_config(root);
    config.task_turns = None;
    // Nothing configured: whatever the request asks for is its own bound,
    // and the CLI enforces it.
    let asked: Request = serde_json::from_value(json!({
        "op": "run", "task": "x", "budget": {"turns": 7}
    }))
    .unwrap();
    let accepted = accept(&asked, &config, Caps::ALL).unwrap();
    assert_eq!(accepted.turns, Some(7));
    assert_eq!(
        Budget {
            cost_microusd: None,
            turns: Some(7)
        },
        *asked.budget().unwrap()
    );
}

/// ADR-0013 §5 and §6: what each ending bills.
#[test]
fn the_billing_table() {
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    let usage = Usage {
        input_tokens: 44_125,
        output_tokens: 381,
        cost_microusd: Some(259_148),
        turns: Some(3),
    };

    let nothing = bill(&config, Billing::Nothing);
    assert_eq!(nothing.get(&DimKey::Tokens), None);

    let reported = bill(&config, Billing::Reported(usage));
    assert_eq!(reported.get(&DimKey::Tokens), Some(44_506));
    assert_eq!(
        reported.get(&DimKey::CostMicroUsd),
        Some(259_148),
        "the CLI's own figure, whatever the login"
    );

    let ceiling = bill(&config, Billing::Ceiling);
    assert_eq!(ceiling.get(&DimKey::Tokens), Some(400_000));
    assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(2_000_000));

    // No cost from the CLI: derived at the configured prices, or absent.
    let silent = Usage {
        cost_microusd: None,
        ..usage
    };
    assert_eq!(
        bill(&config, Billing::Reported(silent)).get(&DimKey::CostMicroUsd),
        None
    );
    let mut priced = config.clone();
    priced.input_price_microusd = Some(5);
    priced.output_price_microusd = Some(25);
    assert_eq!(
        bill(&priced, Billing::Reported(silent)).get(&DimKey::CostMicroUsd),
        Some(44_125 * 5 + 381 * 25)
    );

    // Nothing is reported that the kernel or the reducer already counts.
    for dim in [DimKey::Calls, DimKey::WallMs, DimKey::ComputeMs] {
        assert_eq!(reported.get(&dim), None, "{dim}");
        assert_eq!(ceiling.get(&dim), None, "{dim}");
    }
}

/// A run that never started: the reply still names the CLI, and bills
/// nothing.
#[test]
fn a_refusal_is_a_reply_not_an_exception() {
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    let reply = refused(
        &config,
        "2.1.272",
        refusal(ErrorKind::Host, "cannot start `claude`: No such file"),
    );
    assert_eq!(reply.v, VERSION);
    assert_eq!(reply.cli.name, "claude");
    assert!(reply.transcript.is_empty());
    assert_eq!(reply.truncated, Truncated::default());
    let value: Value = serde_json::from_slice(&encode(&reply)).unwrap();
    assert_eq!(value["stop"]["error"]["kind"], json!("host"));
    assert_eq!(value["envelope"], Value::Null);
    assert_eq!(value["usage"]["cost_microusd"], Value::Null);
}

/// The ladder's rows, as ADR-0013 §5 prices them, over a run the adapter
/// already collected.
#[test]
fn settling_prices_what_the_cli_reported_and_the_ceiling_when_it_did_not() {
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    let usage = Usage {
        input_tokens: 100,
        output_tokens: 10,
        cost_microusd: Some(42),
        turns: Some(2),
    };
    let envelope = include_str!("fixtures/agent/envelope.json");
    let outcome = |stop| Outcome {
        stop,
        session: Some("affe155e".to_owned()),
        model: Some("claude-opus-5[1m]".to_owned()),
        mode: Some("none".to_owned()),
        usage,
        final_message: Some(envelope.as_bytes().to_vec()),
    };
    let run = |ending, terminal_at| Run {
        lines: vec![br#"{"type":"result"}"#.to_vec()],
        dropped: 0,
        truncated: false,
        stderr: String::new(),
        ending,
        terminal_at,
        status: None,
    };

    // The CLI reported: `done`, the envelope, the CLI's own usage.
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Completed, Some(0)),
        &outcome(None),
    );
    assert_eq!(reply.stop, Stop::Done);
    assert!(!reply.envelope.unwrap().summary.is_empty());
    assert_eq!(reply.usage, usage);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(110));

    // The interrupt was answered: `abandoned`, no envelope, real usage.
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Abandoned(Rung::Interrupt), Some(0)),
        &outcome(None),
    );
    assert_eq!(reply.stop, Stop::Abandoned);
    assert!(
        reply.envelope.is_none(),
        "no interrupt gives the model a turn"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(110));

    // A signal was needed: still `abandoned`, but billed at the ceiling —
    // the turn it was inside is spent and reported by nobody.
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Abandoned(Rung::Term), Some(0)),
        &outcome(None),
    );
    assert_eq!(reply.stop, Stop::Abandoned);
    assert_eq!(
        reply.usage,
        Usage::default(),
        "nothing trustworthy to report"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000));

    // SIGKILL: `error.lost`, the ceiling.
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Abandoned(Rung::Kill), None),
        &outcome(None),
    );
    let Stop::Error(error) = reply.stop else {
        panic!("lost")
    };
    assert_eq!(error.kind, ErrorKind::Lost);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000));

    // The wall bound climbs the same ladder, reporting a limit instead.
    let (reply, _) = settle(
        &config,
        "2.1.272",
        &run(Ending::WallLimit(Rung::Interrupt), Some(0)),
        &outcome(None),
    );
    assert_eq!(reply.stop, Stop::Limit(Limit::Wall));

    // The CLI exited without its terminal event: lost, at the ceiling.
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Completed, None),
        &outcome(None),
    );
    let Stop::Error(error) = reply.stop else {
        panic!("lost")
    };
    assert_eq!(error.kind, ErrorKind::Lost);
    assert_eq!(consumed.get(&DimKey::Tokens), Some(400_000));

    // A limit the CLI itself reported stands as the adapter read it.
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Completed, Some(0)),
        &outcome(Some(Stop::Limit(Limit::Turns))),
    );
    assert_eq!(reply.stop, Stop::Limit(Limit::Turns));
    assert!(
        reply.envelope.is_none(),
        "the model got no turn to write one"
    );
    assert_eq!(consumed.get(&DimKey::Tokens), Some(110), "real usage");

    // A final message that is not an envelope: `error.envelope`, the
    // transcript intact, and never a synthesized `{"status":"failed"}`.
    let mut broken = outcome(None);
    broken.final_message = Some(b"I could not do it, sorry.".to_vec());
    let (reply, consumed) = settle(
        &config,
        "2.1.272",
        &run(Ending::Completed, Some(0)),
        &broken,
    );
    let Stop::Error(error) = reply.stop else {
        panic!("envelope violation")
    };
    assert_eq!(error.kind, ErrorKind::Envelope);
    assert!(reply.envelope.is_none());
    assert_eq!(reply.transcript.len(), 1, "what the CLI actually said");
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(110),
        "somebody paid for it"
    );
}

/// The flight registry (ADR-0013 §5): what `Driver::abandon` reaches, and
/// what it remembers when the abandon arrives first.
#[test]
fn the_flight_registry_remembers_an_abandon_that_arrived_first() {
    let flights = std::sync::Arc::new(Flights::default());
    assert_eq!(flights.in_flight(), 0);

    let (flight, early) = flights.enter(Corr::new(1));
    assert!(!early);
    assert_eq!(flights.in_flight(), 1);
    assert!(!flight.cancel().abandoned());
    flights.abandon(Corr::new(1));
    assert!(flight.cancel().abandoned(), "the open run was reached");
    drop(flight);
    assert_eq!(flights.in_flight(), 0, "a finished run leaves the table");

    // Phase two of `cancel` can beat the delivery to the driver: the corr is
    // remembered, and the run that arrives next is abandoned before it
    // spawns anything.
    flights.abandon(Corr::new(2));
    let (flight, early) = flights.enter(Corr::new(2));
    assert!(early, "abandoned before the driver saw it");
    assert!(!flight.cancel().abandoned(), "nothing was started to stop");
    assert_eq!(
        bill(
            &agent::adr_config(env!("CARGO_MANIFEST_DIR")),
            Billing::Nothing
        )
        .get(&DimKey::Tokens),
        None
    );

    // A harness shutting down leaves no CLI behind.
    let (other, _) = flights.enter(Corr::new(3));
    flights.abandon_all();
    assert!(flight.cancel().abandoned() && other.cancel().abandoned());
}

/// Re-renders the two schema fixtures from the types, for the day a field
/// moves: `cargo test -p tau-drivers --all-features --test agent_wire --
/// --ignored rewrite_schema_fixtures`. Ignored by default — the fixtures
/// are the assertion, and a test that rewrote its own expectation would
/// assert nothing.
#[test]
#[ignore = "writes fixtures; run deliberately"]
fn rewrite_schema_fixtures() {
    let config = agent::adr_config(env!("CARGO_MANIFEST_DIR"));
    let write = |name: &str, value: &Value| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent")
            .join(name);
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
        )
        .unwrap();
    };
    for (name, caps) in [
        ("describe.json", Caps::ALL),
        ("describe-run-only.json", Caps::RUN_ONLY),
    ] {
        write(
            name,
            &json!({
                "description": config.describe(caps),
                "input_schema": wire::schema(caps),
            }),
        );
    }
    write(
        "envelope-schema.json",
        &tau_drivers::agent::envelope::schema(),
    );
}
