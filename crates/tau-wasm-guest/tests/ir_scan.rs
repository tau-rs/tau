//! Cover `build/ir_scan.rs` — the scan that decides which goal-predicate
//! machinery a component links (#689).
//!
//! This runs on the HOST. `tau-wasm-guest`'s `src/` is wasm32-only (`lib.rs`
//! compiles to nothing anywhere else), so the scan is `include!`d here the
//! same way `build.rs` includes it, rather than imported from the crate.
//!
//! What is worth testing here is not the arithmetic — it is that every path
//! by which the interpreter can reach the `DeterministicRegistry` is
//! detected. A path this scan misses produces a component that builds
//! cleanly, links no registry, and then fails at RUN time on wasm only.

#![allow(dead_code)]

use tau_ir::check::{Check, CheckVerify, Condition, GoalPredicate, Locus, OnFail, RetryPolicy};
use tau_ir::ids::{CheckId, PipelineStepId, StepId};
use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
use tau_ir::node::Deterministic;
use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
use tau_ir::tool_impl::NativeFnRef;

include!("../build/ir_scan.rs");

/// The literal in `build/ir_scan.rs` must be the same string the predicate
/// registry dispatches on.
///
/// The scan keeps its own copy deliberately — naming the constant directly
/// would put a `tau-native-tools`(`goal-predicates`) edge, and therefore
/// `regex-automata`, into the guest's BUILD-dependency graph, which is the
/// cost the scan exists to avoid. This test is what makes the copy safe: it
/// reaches the authoritative constant through a DEV-dependency, which the
/// wasm artifact never links.
#[test]
fn fn_matches_literal_matches_tau_native_tools() {
    assert_eq!(
        FN_MATCHES,
        tau_native_tools::goal_predicates::FN_MATCHES,
        "build/ir_scan.rs's FN_MATCHES drifted from the registry's; a scan \
         that cannot spot `matches` links no regex engine and the guest \
         fails at run time"
    );
}

fn empty_module() -> IrModule {
    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target: tau_ports::target::registry::list_available()
            .next()
            .expect("registry has at least one target")
            .triple,
        workflow: Workflow::default(),
        triggers: Vec::new(),
    }
}

fn step(id: &str, run: StepRun) -> PipelineStep {
    PipelineStep {
        id: PipelineStepId(id.into()),
        run,
        input: "${input}".into(),
    }
}

fn condition(predicate: GoalPredicate) -> Condition {
    Condition {
        evaluates: Locus::Output(PipelineStepId("x".into())),
        predicate,
    }
}

fn with_pipeline(steps: Vec<PipelineStep>) -> IrModule {
    let mut m = empty_module();
    m.workflow.pipeline = Some(Pipeline { steps });
    m
}

fn deterministic_node(fn_name: &str) -> Deterministic {
    Deterministic {
        id: StepId("d".into()),
        fn_ref: NativeFnRef {
            name: fn_name.into(),
        },
        input_schema: serde_json::json!({}),
        output_schema: serde_json::json!({}),
    }
}

fn goal_check(predicate: GoalPredicate) -> Check {
    Check {
        id: CheckId("c".into()),
        verify: CheckVerify::Goal {
            evaluates: Locus::Output(PipelineStepId("x".into())),
            predicate,
        },
        retry: RetryPolicy {
            on_fail: OnFail::Abort,
            max_attempts: 1,
            gate: PipelineStepId("x".into()),
        },
    }
}

/// The whole point of #689: an agents-only pipeline links neither the
/// registry nor the regex engine.
#[test]
fn agent_only_pipeline_needs_nothing() {
    let m = with_pipeline(vec![step(
        "a",
        StepRun::Agent(tau_ir::ids::AgentId("draft".into())),
    )]);
    assert_eq!(scan(&m), GoalUse::default());
}

/// Path (1): a Branch condition.
#[test]
fn branch_condition_needs_the_registry() {
    let m = with_pipeline(vec![step(
        "b",
        StepRun::Branch {
            on: condition(GoalPredicate::NonEmpty),
            then: vec![],
            otherwise: vec![],
        },
    )]);
    assert_eq!(
        scan(&m),
        GoalUse {
            any: true,
            matches: false
        }
    );
}

/// Path (1), regex arm: `matches` in a Loop's `until`.
#[test]
fn loop_until_matches_needs_the_regex_engine() {
    let m = with_pipeline(vec![step(
        "l",
        StepRun::Loop {
            body: vec![],
            until: condition(GoalPredicate::Matches("APPROVED".into())),
            max_iters: 3,
        },
    )]);
    assert_eq!(
        scan(&m),
        GoalUse {
            any: true,
            matches: true
        }
    );
}

/// Conditions nested inside Parallel branches and Branch arms are reached —
/// a scan that only looked at top-level steps would miss the north-star
/// shape, where the Loop lives inside the Branch's then-arm.
#[test]
fn nested_conditions_are_reached() {
    let inner_loop = step(
        "l",
        StepRun::Loop {
            body: vec![],
            until: condition(GoalPredicate::Matches("APPROVED".into())),
            max_iters: 3,
        },
    );
    let branch = step(
        "b",
        StepRun::Branch {
            on: condition(GoalPredicate::NonEmpty),
            then: vec![inner_loop],
            otherwise: vec![],
        },
    );
    let m = with_pipeline(vec![step(
        "p",
        StepRun::Parallel {
            branches: vec![vec![branch]],
        },
    )]);
    assert_eq!(
        scan(&m),
        GoalUse {
            any: true,
            matches: true
        }
    );
}

/// Path (2): `CheckVerify::Goal`, detected from the `checks` table without
/// needing a `StepRun::Check` to reference it.
#[test]
fn goal_check_needs_the_registry() {
    let mut m = empty_module();
    m.workflow
        .checks
        .insert(CheckId("c".into()), goal_check(GoalPredicate::Exists));
    assert_eq!(
        scan(&m),
        GoalUse {
            any: true,
            matches: false
        }
    );
}

/// Paths (3) and (4): a deterministic node is reachable BOTH from
/// `StepRun::Deterministic` and from an agent's `ToolImpl::Step`. Walking
/// `workflow.steps` wholesale covers both without resolving ids — this test
/// pins that, since the table alone (no pipeline at all) must be enough.
#[test]
fn deterministic_step_table_alone_needs_the_registry() {
    let mut m = empty_module();
    m.workflow.steps.insert(
        StepId("d".into()),
        deterministic_node("__tau::goal::non_empty"),
    );
    assert_eq!(
        scan(&m),
        GoalUse {
            any: true,
            matches: false
        }
    );
}

/// A deterministic step can name `matches` directly, with no `Condition`
/// anywhere in the module.
#[test]
fn deterministic_step_naming_matches_needs_the_regex_engine() {
    let mut m = empty_module();
    m.workflow
        .steps
        .insert(StepId("d".into()), deterministic_node(FN_MATCHES));
    assert_eq!(
        scan(&m),
        GoalUse {
            any: true,
            matches: true
        }
    );
}

/// Unreadable input fails OPEN. An empty file is the standalone/CI build
/// (no `TAU_IR_BYTES`), and undecodable bytes are a real error the guest
/// reports at run time — neither may silently produce a component that
/// cannot evaluate its own predicates. Keeping the empty case fully linked
/// is also what preserves CI's standalone link gate over this path.
#[test]
fn unreadable_ir_links_everything() {
    assert_eq!(scan_baked_ir(&[]), GoalUse::LINK_EVERYTHING);
    assert_eq!(scan_baked_ir(b"not json at all"), GoalUse::LINK_EVERYTHING);
}

/// Round-trip through the real canonical encoder, so the scan is pinned
/// against the bytes `build.rs` is actually handed rather than an in-memory
/// module the encoder might not reproduce.
#[test]
fn scan_survives_a_canonical_round_trip() {
    let m = with_pipeline(vec![step(
        "l",
        StepRun::Loop {
            body: vec![],
            until: condition(GoalPredicate::Matches("APPROVED".into())),
            max_iters: 3,
        },
    )]);
    let bytes = tau_ir::canonical::to_canonical_bytes(&m);
    assert_eq!(
        scan_baked_ir(&bytes),
        GoalUse {
            any: true,
            matches: true
        }
    );
}
