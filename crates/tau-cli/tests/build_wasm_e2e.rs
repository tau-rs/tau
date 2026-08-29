//! β.7.5 PR-E2 DoD: `tau build wasm` of a trivial 1-agent cassette project
//! produces a component that runs in wasmtime and returns a typed RunEvent
//! stream. Requires `wasm32-wasip2` installed.

use std::path::PathBuf;

mod common;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest with the trivial fixture's IR baked in, via the same
/// lowering the CLI uses, and return the component bytes.
fn build_trivial_component() -> Vec<u8> {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("trivial")).expect("lowers");
    common::wasm_component::build_component_with_ir(&bytes)
}

#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn build_wasm_then_run_returns_typed_stream() {
    let component = build_trivial_component();
    let response =
        r#"{"text":"done","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string();
    let (_out, emitted) =
        tau_wasm_host::run_component(&component, "hi", vec![response]).expect("runs");

    // Events now stream via `emit-event`, one JSON-encoded RunEvent per entry,
    // rather than being buffered into the `run` return payload.
    let events: Vec<tau_runtime_core::stream::RunEvent> = emitted
        .iter()
        .map(|e| serde_json::from_str(e).expect("each emitted entry is a RunEvent"))
        .collect();
    assert!(
        matches!(
            events.last(),
            Some(tau_runtime_core::stream::RunEvent::RunCompleted { .. })
        ),
        "stream must end with RunCompleted; got {:?}",
        events.last()
    );
}

/// #621 PR-2: a linear (agent → agent) pipeline already passes wasm
/// feature-fit today, but until `run_pipeline` landed in-guest, such a
/// build produced a component that errored at runtime with "pipelines are
/// not yet executed in-wasm". This proves the gap is closed end to end:
/// the guest drives the whole pipeline and the `run` export payload is the
/// LAST leaf step's rendered output (not the first agent's).
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn build_wasm_linear_pipeline_runs_in_guest_and_returns_last_leaf() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("pipeline")).expect("lowers");
    let component = common::wasm_component::build_component_with_ir(&bytes);
    let response = |text: &str| {
        format!(r#"{{"text":"{text}","tool_uses":[],"stop_reason":"EndTurn","usage":null}}"#)
    };
    let (payload, _events) = tau_wasm_host::run_component(
        &component,
        "hello",
        vec![response("the draft"), response("the polished reply")],
    )
    .expect("guest runs the pipeline");
    assert_eq!(
        payload, "the polished reply",
        "payload must be the LAST leaf step's rendered output"
    );
}

/// #621 review follow-up: `any-wasi-strict` declares `Parallel` supported,
/// so prove the guest EXECUTES a fan-out rather than merely lowering one.
///
/// This is the only in-guest exercise of `buffered(PARALLEL_CAP)` (ADR-0059
/// Decision 2) — the one interpreter construct that parks child futures on
/// `FuturesUnordered`'s wakers rather than completing on first poll. The
/// guest's executor is a noop-waker busy-poll loop
/// (`tau-wasm-guest/src/executor.rs`), so this test also pins the invariant
/// that the fork-join makes progress there; a regression shows up as a hang
/// (nextest's per-test timeout), not a wrong answer.
///
/// The assertion is the JOIN step's output, and the join's input template
/// reads BOTH branches' outputs — template resolution hard-errors on
/// unresolved refs, so completion proves both branches ran and merged.
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn build_wasm_parallel_pipeline_runs_in_guest() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("parallel")).expect("lowers");
    let component = common::wasm_component::build_component_with_ir(&bytes);
    let response = |text: &str| {
        format!(r#"{{"text":"{text}","tool_uses":[],"stop_reason":"EndTurn","usage":null}}"#)
    };
    // Distinct per-turn texts: the payload can only be "joined" if the guest
    // ran the fan-out and then the join leaf, in that order.
    let (payload, _events) = tau_wasm_host::run_component(
        &component,
        "today",
        vec![response("sunny"), response("quiet"), response("joined")],
    )
    .expect("guest runs the Parallel fan-out");
    assert_eq!(
        payload, "joined",
        "payload must be the join leaf's output, which is reachable only if \
         BOTH parallel branches produced their outputs in-guest"
    );
}

/// #689: an IR that reaches no goal predicate links no predicate registry,
/// and therefore no regex engine.
///
/// `pipeline` is agent-only — two agents, no Branch, Loop, check or
/// deterministic step — so nothing in it can reach
/// `tau_native_tools::goal_predicates`. Before the gate it paid for the full
/// engine anyway: measured on this exact fixture, 2,777,914 B with the
/// registry linked against 1,988,452 B without, a difference of 789,462 B
/// (~771 KiB, 28.4% of the component). The saving is that large because
/// dropping the reference drops `regex_syntax` (the parser, the bigger half
/// of the code) and regex's Unicode tables in the data section, not just the
/// matcher.
///
/// The byte ceiling below is a tripwire, not a target — it is set well above
/// the measured size so unrelated growth does not flap it, while a
/// re-linked engine (+771 KiB) blows straight through. `TAU_WASM_SIZE_BUDGET`
/// cannot catch this: it gates the EMPTY-IR floor build, which has no
/// pipeline to reach a predicate from.
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn goal_free_component_links_no_regex_engine() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("pipeline")).expect("lowers");
    let component = common::wasm_component::build_component_with_ir(&bytes);

    assert!(
        !common::wasm_component::links_regex_engine(&component),
        "an agent-only pipeline reaches no goal predicate, so build.rs must \
         emit neither tau_goal_predicates nor tau_goal_matches and wasm-ld \
         must collect the regex engine"
    );

    // Paired with `north_star_…`'s positive assertion, which fails if the
    // name section ever stops being emitted and makes the check above vacuous.
    const CEILING: usize = 2_300_000;
    assert!(
        component.len() < CEILING,
        "goal-free component is {} B, over the {CEILING} B tripwire — the \
         regex engine (~771 KiB) has most likely been re-linked; check what \
         made `deterministic_registry()` reachable again",
        component.len()
    );
}

/// #689 middle arm: an IR that reaches a predicate but NOT `matches` links
/// the four allocation-only predicates and still no regex engine.
///
/// This is the arm neither other fixture covers, and the only one that
/// exercises `goal_predicates::invoke_alloc_only` in the guest. Both failure
/// modes are silent without it: linking nothing makes the run die on "branch
/// … needs a deterministic registry", and linking everything hands the
/// ~771 KiB back while every other test still passes.
///
/// The run matters as much as the size. `summary`'s template reads
/// `steps.handle.output` — a step nested inside the branch's then-arm — and
/// unresolved refs hard-error, so a completed run proves the registry was
/// consulted and answered `non_empty` correctly, not merely linked.
#[test]
#[ignore = "builds a wasm component; run with --run-ignored"]
fn predicate_without_matches_runs_in_guest_without_the_regex_engine() {
    let (_module, bytes) =
        tau_cli::cmd::build_wasm::lower_to_wasm_ir(&fixture("goal-no-regex")).expect("lowers");
    let component = common::wasm_component::build_component_with_ir(&bytes);

    assert!(
        !common::wasm_component::links_regex_engine(&component),
        "`non_empty` is allocation-only; a pipeline that never reaches \
         `matches` must not link the regex engine"
    );

    let response = |text: &str| {
        serde_json::json!({
            "text": text,
            "tool_uses": [],
            "stop_reason": "EndTurn",
            "usage": null,
        })
        .to_string()
    };
    let (payload, _events) = tau_wasm_host::run_component(
        &component,
        "a report worth triaging",
        vec![
            response("triaged: needs handling"),
            response("handled"),
            response("NO-REGEX-SUMMARY"),
        ],
    )
    .expect("guest evaluates a non-regex predicate and runs the branch");

    assert_eq!(
        payload, "NO-REGEX-SUMMARY",
        "the then-arm must have run and the last leaf rendered; a registry \
         that was linked but never consulted could not have got here"
    );
}
