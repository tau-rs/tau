//! The tool loop: projection, one round per `tool_call` stop, every failure
//! fed back as a typed `tool_result`, budget refusals terminal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    calls, end_turn, id, name, plenty, provider, recording_sleep, refusal, reply, slot,
    store_schema, take, text, tokio_spawner, tool_call, transport, BrokenSchemaDriver, Waits,
    World, MODEL_CEILING, STORE_DESCRIPTION, TOOL_CEILING,
};
use libtau::{
    prompt, render_result, tool_loop, tool_loop_with, Note, ProjectError, RetryPolicy,
    ToolLoopError, Toolbox,
};
use serde_json::{json, Value};
use tau_kernel::abi::{Budget, Capability, DimKey, Namespace};
use tau_kernel::bridge::{
    Content, ErrorKind, ModelError, ModelReply, ModelRequest, Role, Sampling, StopReason, ToolDef,
    ToolErrorKind,
};
use tau_kernel::kernel::Kernel;
use tau_kernel::log::Log;
use tau_kernel::syscall::{program, Handle};

type LoopOutcome = (ModelRequest, Result<ModelReply, ToolLoopError>);

/// Projects `caps` from inside the program, runs the loop on `request`, and
/// leaves the transcript and the outcome in a slot. The hook notes the loop
/// saw are dropped; [`run_loop_noted`] keeps them.
async fn run_loop(
    world: &World,
    ns: Namespace,
    budget: Budget,
    caps: Vec<Capability>,
    extra: Vec<(Capability, ToolDef)>,
    request: ModelRequest,
) -> LoopOutcome {
    let (transcript, result, _) = run_loop_noted(world, ns, budget, caps, extra, request).await;
    (transcript, result)
}

/// [`run_loop`], with the hook notes the loop saw and the program read.
async fn run_loop_noted(
    world: &World,
    ns: Namespace,
    budget: Budget,
    caps: Vec<Capability>,
    extra: Vec<(Capability, ToolDef)>,
    request: ModelRequest,
) -> (ModelRequest, Result<ModelReply, ToolLoopError>, Vec<Note>) {
    let out = slot();
    let sink = Arc::clone(&out);
    let model = world.model_cap;
    world
        .run(
            program(move |h| async move {
                let mut toolbox = Toolbox::project(&h, &caps).expect("projection");
                for (cap, def) in extra {
                    toolbox.add(cap, def).expect("add");
                }
                let mut request = request;
                let mut notes = Vec::new();
                let result = tool_loop(&h, model, &toolbox, &mut request, &mut notes).await;
                sink.lock().unwrap().replace((request, result, notes));
                h.exit(b"")
            }),
            ns,
            budget,
        )
        .await;
    take(&out)
}

fn tool_result(call_id: &str, content: &str, kind: Option<ToolErrorKind>) -> Content {
    Content::ToolResult {
        call_id: call_id.into(),
        content: content.into(),
        is_error: kind.is_some(),
        error_kind: kind,
    }
}

fn results_of(request: &ModelRequest) -> Vec<Content> {
    let last = request.messages.last().expect("a turn");
    assert_eq!(last.role, Role::User);
    last.content.clone()
}

// --- the happy path, pinned to the ADR's fixture ---------------------------

#[tokio::test]
async fn one_round_reproduces_the_fixture_request_exactly() {
    // The scripted model answers the first call with the fixture's assistant
    // turn, and the second with end_turn. The second request the driver
    // receives must be `fixtures/bridge/request.json`, byte for byte in value.
    let world = World::boot([
        calls(vec![
            text("Let me look."),
            tool_call("call_1", "store", json!({ "op": "read", "key": "a" })),
        ]),
        end_turn("It is `hello`."),
    ]);
    let mut request = prompt("What is stored under key a?", 1024);
    request.system = Some("You are a research assistant with a key-value store.".into());
    request.sampling = Some(Sampling {
        temperature: Some(0.0),
        seed: Some(42),
        ..Sampling::default()
    });

    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        request,
    )
    .await;

    let last = result.unwrap();
    assert_eq!(last.stop, StopReason::EndTurn);
    let seen = world.model.seen();
    assert_eq!(seen.len(), 2);
    let fixture: Value = serde_json::from_str(include_str!(
        "../../kernel/tests/fixtures/bridge/request.json"
    ))
    .unwrap();
    let second = seen.get(1).unwrap();
    assert_eq!(serde_json::to_value(second).unwrap(), fixture);
    assert_eq!(
        transcript.messages, second.messages,
        "the caller's request holds the transcript the model saw"
    );
    assert_eq!(
        world.store.seen(),
        vec![br#"{"key":"a","op":"read"}"#.to_vec()],
        "the driver received exactly the tool_call input bytes"
    );
}

#[tokio::test]
async fn projection_names_the_tool_after_its_driver_and_uses_its_schema() {
    let world = World::boot([end_turn("nothing to do")]);
    let (transcript, _) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("hi", 64),
    )
    .await;
    assert_eq!(
        transcript.tools,
        vec![ToolDef {
            name: name("store"),
            description: STORE_DESCRIPTION.into(),
            input_schema: store_schema(),
        }]
    );
    assert_eq!(world.model.seen().first().unwrap().tools, transcript.tools);
}

#[tokio::test]
async fn sealed_thinking_blocks_go_back_in_the_assistant_turn_unchanged() {
    // The scripted model thinks (two sealed blocks, one of them a shape the
    // loop has never seen) before calling the tool. The second request
    // must carry that whole turn back, blocks first, exactly as replied:
    // the provider rejects a turn whose thinking was edited or dropped.
    let sealed = |data: Value| Content::Thinking {
        provider: "anthropic".into(),
        data,
    };
    let first = calls(vec![
        sealed(json!({"type": "thinking", "thinking": "", "signature": "EqQBCkYIBxgC"})),
        sealed(json!({"type": "something_new", "nested": {"deep": [1, 2, 3]}})),
        text("Let me look."),
        tool_call("call_1", "store", json!({ "op": "read", "key": "a" })),
    ]);
    let expected_turn = first.content.clone();
    let world = World::boot([first, end_turn("It is `hello`.")]);

    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("What is stored under key a?", 1024),
    )
    .await;

    assert_eq!(result.unwrap().stop, StopReason::EndTurn);
    let seen = world.model.seen();
    let second = seen.get(1).expect("two calls");
    let assistant = second.messages.get(1).expect("the assistant turn");
    assert_eq!(assistant.role, Role::Assistant);
    assert_eq!(
        assistant.content, expected_turn,
        "sealed blocks in place, unread"
    );
    assert_eq!(
        results_of(second),
        vec![tool_result("call_1", "hello", None)],
        "and the tool was still called"
    );
    assert_eq!(transcript.messages, second.messages);
}

// --- one test per error_kind ---------------------------------------------

#[tokio::test]
async fn an_unknown_tool_is_fed_back_without_a_send() {
    let world = World::boot([
        calls(vec![tool_call("call_2", "serch", json!({ "query": "x" }))]),
        end_turn("sorry"),
    ]);
    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![(world.echo_cap, echo_def("search"))],
        prompt("go", 64),
    )
    .await;
    assert_eq!(result.unwrap().stop, StopReason::EndTurn);
    assert_eq!(
        results_of(&transcript),
        vec![tool_result(
            "call_2",
            "unknown tool `serch`; available: store, search",
            Some(ToolErrorKind::UnknownTool)
        )]
    );
    assert!(world.store.seen().is_empty(), "nothing was sent anywhere");
}

#[tokio::test]
async fn bad_args_are_fed_back_without_a_send() {
    let world = World::boot([
        calls(vec![tool_call("call_3", "store", json!({ "op": "read" }))]),
        end_turn("oops"),
    ]);
    let (transcript, _) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    match results_of(&transcript).as_slice() {
        [Content::ToolResult {
            call_id,
            content,
            is_error,
            error_kind,
        }] => {
            assert_eq!(call_id, "call_3");
            assert!(is_error);
            assert_eq!(*error_kind, Some(ToolErrorKind::BadArgs));
            assert!(content.starts_with("bad args for `store`: "), "{content}");
            assert!(content.contains("missing properties 'key'"), "{content}");
        }
        other => panic!("expected one tool_result, got {other:?}"),
    }
    assert!(world.store.seen().is_empty());
}

#[tokio::test]
async fn a_capability_not_held_is_fed_back_as_denied() {
    // The model is told `store` exists (the harness added it by hand), but
    // this agent's namespace does not hold it. The kernel refuses the send.
    let world = World::boot([
        calls(vec![tool_call(
            "call_4",
            "store",
            json!({ "op": "read", "key": "a" }),
        )]),
        end_turn("understood"),
    ]);
    let store_def = ToolDef {
        name: name("store"),
        description: STORE_DESCRIPTION.into(),
        input_schema: store_schema(),
    };
    let (transcript, _) = run_loop(
        &world,
        Namespace::from_caps([world.model_cap]),
        plenty(),
        vec![],
        vec![(world.store_cap, store_def)],
        prompt("go", 64),
    )
    .await;
    assert_eq!(
        results_of(&transcript),
        vec![tool_result(
            "call_4",
            "denied: this agent does not hold `store`",
            Some(ToolErrorKind::Denied)
        )]
    );
    assert!(world.store.seen().is_empty());
}

#[test]
fn an_unreadable_reply_is_rendered_as_failed() {
    // The one `failed` the loop can observe before the M3 error envelope:
    // the reply's payload is not in the blob store. A running kernel never
    // produces it, so the render path is exercised directly.
    assert_eq!(
        render_result("call_5".into(), "search", None),
        tool_result(
            "call_5",
            "`search`: reply payload unavailable",
            Some(ToolErrorKind::Failed)
        )
    );
    assert_eq!(
        render_result("call_6".into(), "search", Some(b"ok".to_vec())),
        tool_result("call_6", "ok", None)
    );
    assert_eq!(
        render_result("call_7".into(), "search", Some(vec![0xff, b'a'])),
        tool_result("call_7", "\u{fffd}a", None),
        "non-UTF-8 bytes are rendered lossily, not refused"
    );
}

// --- the rest of the contract ---------------------------------------------

#[tokio::test]
async fn parallel_calls_are_answered_in_order_in_one_user_turn() {
    let world = World::boot([
        calls(vec![
            tool_call("call_a", "store", json!({ "op": "read", "key": "a" })),
            tool_call("call_b", "nope", json!({})),
            tool_call(
                "call_c",
                "store",
                json!({ "op": "write", "key": "b", "value": "v" }),
            ),
        ]),
        end_turn("done"),
    ]);
    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert_eq!(result.unwrap().stop, StopReason::EndTurn);
    assert_eq!(
        results_of(&transcript),
        vec![
            tool_result("call_a", "hello", None),
            tool_result(
                "call_b",
                "unknown tool `nope`; available: store",
                Some(ToolErrorKind::UnknownTool)
            ),
            tool_result("call_c", "ok", None),
        ]
    );
    assert_eq!(
        transcript.messages.len(),
        3,
        "user, assistant, one user turn with every result"
    );
    assert_eq!(world.store.seen().len(), 2);
}

#[tokio::test]
async fn the_echo_driver_is_a_tool_when_the_harness_names_it() {
    let world = World::boot([
        calls(vec![tool_call("call_e", "echo", json!("ping"))]),
        end_turn("pong"),
    ]);
    let (transcript, _) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![],
        vec![(world.echo_cap, echo_def("echo"))],
        prompt("go", 64),
    )
    .await;
    // The invocation is exactly the input's JSON bytes, and the reply is
    // exactly those bytes as text: a JSON string, quotes included.
    assert_eq!(
        results_of(&transcript),
        vec![tool_result("call_e", "\"ping\"", None)]
    );
}

#[tokio::test]
async fn stops_other_than_tool_call_end_the_loop_and_are_returned() {
    for stop in [
        StopReason::MaxTokens,
        StopReason::Refusal,
        StopReason::StopSequence,
        StopReason::Error(ModelError {
            kind: ErrorKind::OverCeiling,
            message: "too big".into(),
        }),
    ] {
        let world = World::boot([reply(vec![text("partial")], stop.clone())]);
        let (transcript, result) = run_loop(
            &world,
            world.all_caps(),
            plenty(),
            vec![world.store_cap],
            vec![],
            prompt("go", 64),
        )
        .await;
        let reply = result.unwrap();
        assert_eq!(reply.stop, stop);
        assert_eq!(world.model.seen().len(), 1, "exactly one call for {stop:?}");
        assert_eq!(
            transcript.messages.len(),
            1,
            "the ending reply is returned, not appended"
        );
    }
}

#[tokio::test]
async fn a_tool_call_stop_with_no_calls_is_malformed() {
    let world = World::boot([calls(vec![text("I would call a tool, but")])]);
    let (_, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert!(matches!(result, Err(ToolLoopError::Malformed { .. })));
}

#[tokio::test]
async fn a_budget_refusal_on_a_tool_send_is_terminal() {
    // Enough for the model calls, not enough to reserve one tool ceiling
    // after the first model call's reservation is settled? Simpler: give
    // exactly one call, which the model call spends.
    let world = World::boot([
        calls(vec![tool_call(
            "call_1",
            "store",
            json!({ "op": "read", "key": "a" }),
        )]),
        end_turn("unreachable"),
    ]);
    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        Budget::from_dims([
            (DimKey::Tokens, MODEL_CEILING + TOOL_CEILING),
            (DimKey::Calls, 1),
        ]),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert!(
        matches!(result, Err(ToolLoopError::Budget(_))),
        "got {result:?}"
    );
    assert_eq!(world.model.seen().len(), 1, "no second model call");
    assert!(world.store.seen().is_empty());
    assert_eq!(
        transcript.messages.len(),
        2,
        "the assistant turn was recorded before the refusal"
    );
}

// --- projection ------------------------------------------------------------

#[tokio::test]
async fn projection_refuses_a_duplicate_a_non_tool_and_an_unheld_cap() {
    let world = World::boot([]);
    let out = slot();
    let sink = Arc::clone(&out);
    let (store, echo, model) = (world.store_cap, world.echo_cap, world.model_cap);
    world
        .run(
            program(move |h| async move {
                let outcomes = (
                    Toolbox::project(&h, &[store, store]).err(),
                    Toolbox::project(&h, &[echo]).err(),
                    Toolbox::project(&h, &[model]).err(),
                );
                sink.lock().unwrap().replace(outcomes);
                h.exit(b"")
            }),
            Namespace::from_caps([store, echo]),
            plenty(),
        )
        .await;
    let (dup, not_tool, unheld) = take(&out);
    assert!(matches!(dup, Some(ProjectError::Duplicate(n)) if n == name("store")));
    assert!(matches!(not_tool, Some(ProjectError::NotATool(c)) if c == echo));
    assert!(matches!(unheld, Some(ProjectError::Describe { cap, .. }) if cap == model));
}

#[tokio::test]
async fn a_driver_with_unusable_schema_bytes_is_refused_at_projection() {
    let kernel = Kernel::boot(Log::with_sink(Vec::new()).unwrap(), tokio_spawner);
    let broken = kernel
        .register_driver(
            id("broken"),
            BrokenSchemaDriver,
            Budget::from_dims([(DimKey::Tokens, 1)]),
        )
        .unwrap();
    let out = slot();
    let sink = Arc::clone(&out);
    kernel
        .spawn_root(
            program(move |h: Handle| async move {
                sink.lock()
                    .unwrap()
                    .replace(Toolbox::project(&h, &[broken]).err());
                h.exit(b"")
            }),
            Namespace::from_caps([broken]),
            plenty(),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();
    assert!(matches!(
        take(&out),
        Some(ProjectError::Schema { name: n, .. }) if n == name("broken")
    ));
}

#[tokio::test]
async fn add_refuses_a_schema_that_does_not_compile() {
    // `add` never touches the kernel; the world only supplies a capability.
    let world = World::boot([]);
    let mut toolbox = Toolbox::new();
    let err = toolbox
        .add(
            world.echo_cap,
            ToolDef {
                name: name("bad"),
                description: String::new(),
                input_schema: json!({ "type": "not-a-type" }),
            },
        )
        .unwrap_err();
    assert!(matches!(err, ProjectError::Schema { .. }), "{err}");
}

fn echo_def(n: &str) -> ToolDef {
    ToolDef {
        name: name(n),
        description: "Echoes its input.".into(),
        input_schema: json!({}),
    }
}

// --- retries ---------------------------------------------------------------

/// Runs `tool_loop_with` under `policy` with the store as the one tool,
/// recording every wait.
async fn run_loop_retrying(
    world: &World,
    budget: Budget,
    policy: RetryPolicy,
) -> (LoopOutcome, Vec<Duration>) {
    let out = slot();
    let sink = Arc::clone(&out);
    let waits: Waits = Arc::default();
    let sleep = recording_sleep(&waits);
    let model = world.model_cap;
    let store = world.store_cap;
    world
        .run(
            program(move |h| async move {
                let toolbox = Toolbox::project(&h, &[store]).expect("projection");
                let mut request = prompt("go", 64);
                let result = tool_loop_with(
                    &h,
                    model,
                    &toolbox,
                    &mut request,
                    &mut Vec::new(),
                    &policy,
                    sleep,
                )
                .await;
                sink.lock().unwrap().replace((request, result));
                h.exit(b"")
            }),
            world.all_caps(),
            budget,
        )
        .await;
    let waits = waits.lock().unwrap().clone();
    (take(&out), waits)
}

fn two_retries() -> RetryPolicy {
    RetryPolicy {
        retries: 2,
        backoff: Duration::from_millis(10),
        max_backoff: Duration::from_secs(1),
    }
}

#[tokio::test]
async fn the_loop_retries_a_transient_error_in_the_middle_of_a_round() {
    let world = World::boot([
        calls(vec![tool_call(
            "call_1",
            "store",
            json!({ "op": "read", "key": "a" }),
        )]),
        provider(529),
        end_turn("done"),
    ]);
    let ((transcript, result), waits) = run_loop_retrying(&world, plenty(), two_retries()).await;
    assert_eq!(result.unwrap(), end_turn("done"));
    assert_eq!(
        world.model.seen().len(),
        3,
        "call, failed retry target, retry"
    );
    assert_eq!(world.store.seen().len(), 1, "the tool ran once");
    assert_eq!(waits, vec![Duration::from_millis(10)]);
    assert_eq!(
        transcript.messages.len(),
        3,
        "user, assistant, tool results: the failed attempt leaves no turn"
    );
}

#[tokio::test]
async fn a_model_send_refused_on_budget_mid_retry_is_the_loops_budget_error() {
    let world = World::boot([transport(), end_turn("never")]);
    let one_call = Budget::from_dims([(DimKey::Tokens, 100_000), (DimKey::Calls, 1)]);
    let ((_, result), waits) = run_loop_retrying(&world, one_call, two_retries()).await;
    assert!(
        matches!(result, Err(ToolLoopError::Budget(_))),
        "got {result:?}"
    );
    assert_eq!(
        world.model.seen().len(),
        1,
        "the refused retry was never sent"
    );
    assert_eq!(waits.len(), 1);
}

#[tokio::test]
async fn a_model_send_refused_on_budget_before_any_call_is_the_loops_budget_error() {
    let world = World::boot([end_turn("never")]);
    let starved = Budget::from_dims([(DimKey::Tokens, MODEL_CEILING - 1), (DimKey::Calls, 5)]);
    let (_, result) = run_loop(
        &world,
        world.all_caps(),
        starved,
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert!(
        matches!(result, Err(ToolLoopError::Budget(_))),
        "got {result:?}"
    );
    assert!(world.model.seen().is_empty());
}

#[tokio::test]
async fn the_loop_never_retries_a_refusal() {
    let world = World::boot([refusal(), end_turn("never")]);
    let ((_, result), waits) = run_loop_retrying(&world, plenty(), two_retries()).await;
    assert_eq!(result.unwrap(), refusal());
    assert_eq!(world.model.seen().len(), 1);
    assert!(waits.is_empty());
}

// ------------------------------------------------------------------- hooks

use tau_kernel::hook::{FailureMode, HookEvent, HookPoint, HookProgram, Rule, Verdict};

#[tokio::test]
async fn a_send_a_rule_denies_is_fed_back_as_denied_with_the_reason() {
    // The same policy as the native hook below, as one line of the Rule
    // language (ADR-0008 §5): no recompile, the same `denied` tool result.
    let world = World::boot([
        calls(vec![tool_call(
            "call_5",
            "store",
            json!({ "op": "write", "key": "k", "value": "forbidden" }),
        )]),
        end_turn("noted"),
    ]);
    let rule = Rule::parse(
        "when pre_send if driver == store and payload contains \"forbidden\" then deny \"the store does not take that word\"",
    )
    .unwrap();
    world
        .kernel
        .attach(
            HookPoint::PreSend,
            HookProgram::Rule(rule),
            FailureMode::Closed,
        )
        .unwrap();
    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert_eq!(result.unwrap().stop, StopReason::EndTurn);
    assert_eq!(
        results_of(&transcript),
        vec![tool_result(
            "call_5",
            "denied by hook:0: the store does not take that word",
            Some(ToolErrorKind::Denied)
        )]
    );
    assert!(world.store.seen().is_empty(), "the send never happened");
}

#[tokio::test]
async fn a_send_a_hook_denies_is_fed_back_as_denied_with_the_reason() {
    // ADR-0008 §3 / ADR-0006 §4: the model reads why and self-corrects. The
    // hook watches the store driver only, so the model call itself — whose
    // transcript will carry the forbidden word back — is not denied.
    let world = World::boot([
        calls(vec![tool_call(
            "call_5",
            "store",
            json!({ "op": "write", "key": "k", "value": "forbidden" }),
        )]),
        end_turn("noted"),
    ]);
    world
        .kernel
        .attach(
            HookPoint::PreSend,
            HookProgram::native(name("no-forbidden-writes"), |e| {
                Ok(match e {
                    HookEvent::PreSend {
                        driver, payload, ..
                    } if driver.name().as_str() == "store"
                        && payload.windows(9).any(|w| w == b"forbidden") =>
                    {
                        Verdict::Deny("the store does not take that word".into())
                    }
                    _ => Verdict::Allow,
                })
            }),
            FailureMode::Closed,
        )
        .unwrap();
    let (transcript, result) = run_loop(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert_eq!(result.unwrap().stop, StopReason::EndTurn);
    assert_eq!(
        results_of(&transcript),
        vec![tool_result(
            "call_5",
            "denied by hook:0: the store does not take that word",
            Some(ToolErrorKind::Denied)
        )]
    );
    assert!(world.store.seen().is_empty(), "the send never happened");
}

#[tokio::test]
async fn a_hook_note_during_a_model_call_is_not_a_cancellation() {
    // A hook that emits on every model send puts a `Notice` in the mailbox
    // before the reply. It is from a hook, not from a canceller: the loop
    // carries on, and the note lands in `notes` for the program (#83).
    let world = World::boot([
        calls(vec![tool_call(
            "call_6",
            "store",
            json!({ "op": "read", "key": "a" }),
        )]),
        end_turn("done"),
    ]);
    world
        .kernel
        .attach(
            HookPoint::PreSend,
            HookProgram::native(name("note-every-send"), |e| {
                Ok(match e {
                    HookEvent::PreSend { subject, .. } => Verdict::Emit {
                        to: *subject,
                        payload: b"noted".to_vec(),
                    },
                    _ => Verdict::Allow,
                })
            }),
            FailureMode::Closed,
        )
        .unwrap();
    let (transcript, result, notes) = run_loop_noted(
        &world,
        world.all_caps(),
        plenty(),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    let reply = result.unwrap();
    assert_eq!(reply.stop, StopReason::EndTurn);
    assert_eq!(
        results_of(&transcript),
        vec![tool_result("call_6", "hello", None)]
    );
    assert_eq!(world.model.seen().len(), 2);
    // Three sends — model, store, model — and a note on each; every one was
    // resolved by a `recv` the loop made, so every one is surfaced.
    let payloads: Vec<Vec<u8>> = notes
        .iter()
        .map(|n| world.kernel.read(n.payload).unwrap())
        .collect();
    assert_eq!(payloads, vec![b"noted".to_vec(); 3]);
    let first = notes.first().map(|n| n.hook);
    assert!(notes.iter().all(|n| Some(n.hook) == first));
}

#[tokio::test]
async fn an_on_budget_note_during_a_tool_call_reaches_the_program_after_the_loop() {
    // The controller pattern of HANDOFF §10 / ADR-0008 Consequences: an
    // `OnBudget` hook watches the root's `calls` and emits to the root when
    // it dips under the line. The line is drawn so the *tool* send is the
    // crossing — each send spends one call: model to 4, store to 3 — so the
    // note enters the mailbox during the tool call, ahead of the store's
    // reply. The loop's `recv` on the tool's correlation resolves the note
    // first; it must not be a cancellation, must not be fed to the model,
    // and must be in the program's hands once the loop returns.
    let world = World::boot([
        calls(vec![tool_call(
            "call_7",
            "store",
            json!({ "op": "read", "key": "a" }),
        )]),
        end_turn("done"),
    ]);
    let hook = world
        .kernel
        .attach(
            HookPoint::OnBudget {
                dim: DimKey::Calls,
                below: 4,
            },
            HookProgram::native(name("calls-low"), |e| {
                Ok(match e {
                    HookEvent::OnBudget { subject, .. } => Verdict::Emit {
                        to: *subject,
                        payload: b"calls running low".to_vec(),
                    },
                    _ => Verdict::Allow,
                })
            }),
            FailureMode::Open,
        )
        .unwrap();
    let (transcript, result, notes) = run_loop_noted(
        &world,
        world.all_caps(),
        Budget::from_dims([(DimKey::Tokens, 100_000), (DimKey::Calls, 5)]),
        vec![world.store_cap],
        vec![],
        prompt("go", 64),
    )
    .await;
    assert_eq!(result.unwrap().stop, StopReason::EndTurn);
    assert_eq!(
        results_of(&transcript),
        vec![tool_result("call_7", "hello", None)],
        "the note is not a tool result"
    );
    let [note] = notes.as_slice() else {
        panic!("one crossing, one note: {notes:?}");
    };
    assert_eq!(note.hook, hook);
    assert_eq!(
        world.kernel.read(note.payload).unwrap(),
        b"calls running low"
    );
    assert_eq!(world.model.seen().len(), 2, "the loop ran to end_turn");
}
