// Decide which goal-predicate machinery the baked IR can actually reach.
//
// `include!`d by `build.rs` (which turns the answer into `cargo:rustc-cfg`)
// and by `tests/ir_scan.rs` (which pins the fn-name literal and covers the
// reachability paths). It is a plain `include!` rather than a module
// because `build.rs` cannot depend on the crate's own `src/`, and `src/` is
// wasm32-only — on a host target `lib.rs` compiles to nothing.
//
// # Why a scan at all (#689)
//
// Linking the in-guest `DeterministicRegistry` costs ~770 KiB on a ~2.8 MB
// component, because reaching `matches` pulls `regex-automata` AND
// `regex_syntax` AND regex's Unicode tables. A workflow with no predicates
// pays that for nothing. Cargo features cannot express "depends on the IR
// being baked in" — they resolve before build scripts run — so the lever is
// reachability: the guest stops *referencing* the predicate it cannot need,
// and wasm-ld garbage-collects the engine. That is the same mechanism the
// `tau_cap_net_http` / `tau_cap_fs_*` arms already rely on.
//
// # Bias
//
// Deliberately conservative: no reachability analysis, no pruning of
// unreferenced table entries. Over-detecting costs bytes; under-detecting
// costs a run, because the guest's registry answers an unexpected fn with a
// hard "no wasm execution path" error. When in doubt, link it.

/// `__tau::goal::matches` — the one predicate whose body needs a regex engine.
///
/// A local literal, exactly as `tau-ir-lower`'s `predicate_fit::GUEST_FNS`
/// is one, and for the same reason: naming it via
/// `tau_native_tools::goal_predicates::FN_MATCHES` would put a
/// `tau-native-tools`(`goal-predicates`) edge — and therefore
/// `regex-automata` — into this crate's BUILD-dependency graph, which is the
/// cost this whole scan exists to avoid.
///
/// `tests/ir_scan.rs::fn_matches_literal_matches_tau_native_tools` pins it
/// against the authoritative constant through a dev-dependency, so the
/// copy cannot drift without a test failure.
const FN_MATCHES: &str = "__tau::goal::matches";

/// What the baked IR needs linked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct GoalUse {
    /// The IR can reach the `DeterministicRegistry` at all.
    any: bool,
    /// The IR can reach `__tau::goal::matches`, which needs the regex engine.
    matches: bool,
}

impl GoalUse {
    /// Everything on — the answer for an IR we could not read or decode.
    ///
    /// Unknown must mean "link it": a guest built without a predicate it
    /// turns out to need fails at RUN time, on wasm only. This is also what
    /// keeps CI's standalone link gate (`cargo build -p tau-wasm-guest
    /// --target wasm32-wasip2 --release`, no `TAU_IR_BYTES`) compiling and
    /// linking the full predicate path.
    const LINK_EVERYTHING: Self = Self {
        any: true,
        matches: true,
    };
}

/// Scan a decoded module for every path that reaches the registry.
///
/// Four paths reach it, not the two a reading of "Branch and Loop
/// conditions" suggests:
///
/// 1. `StepRun::Branch { on }` / `StepRun::Loop { until }` — a `Condition`;
/// 2. `CheckVerify::Goal { predicate }` behind a `StepRun::Check`;
/// 3. `StepRun::Deterministic(id)` → `workflow.steps[id]`
///    (`interpreter/pipeline.rs`, "pipeline step … needs a deterministic
///    registry");
/// 4. `ToolImpl::Step { id }` → `workflow.steps[id]` — an AGENT-invoked
///    deterministic step (`interpreter/agent_loop.rs`, "agent invoked step
///    tool … but the dispatcher did not provide a DeterministicRegistry").
///
/// (3) and (4) both resolve through `workflow.steps`, so walking that whole
/// table covers each of them without inspecting `workflow.tools` or
/// resolving ids.
fn scan(module: &tau_ir::IrModule) -> GoalUse {
    use tau_ir::check::{CheckVerify, Condition, GoalPredicate};
    use tau_ir::pipeline::{PipelineStep, StepRun};

    let mut use_ = GoalUse::default();

    fn note_predicate(predicate: &GoalPredicate, use_: &mut GoalUse) {
        use_.any = true;
        match predicate {
            GoalPredicate::Matches(_) => use_.matches = true,
            // `predicate_fit` refuses `SchemaValid` and any `NativeFn`
            // outside the five for a Wasi target, so neither can reach a
            // guest build. Flag a `NativeFn` that names `matches` anyway —
            // costs bytes if it ever could, costs a run if it could and we
            // missed it.
            GoalPredicate::NativeFn(r) if r.name == FN_MATCHES => use_.matches = true,
            GoalPredicate::Exists
            | GoalPredicate::NonEmpty
            | GoalPredicate::Equals(_)
            | GoalPredicate::MinCount(_)
            | GoalPredicate::SchemaValid(_)
            | GoalPredicate::NativeFn(_) => {}
        }
    }

    fn note_condition(cond: &Condition, use_: &mut GoalUse) {
        note_predicate(&cond.predicate, use_);
    }

    fn walk(steps: &[PipelineStep], use_: &mut GoalUse) {
        for step in steps {
            match &step.run {
                StepRun::Branch {
                    on,
                    then,
                    otherwise,
                } => {
                    note_condition(on, use_);
                    walk(then, use_);
                    walk(otherwise, use_);
                }
                StepRun::Loop { body, until, .. } => {
                    note_condition(until, use_);
                    walk(body, use_);
                }
                StepRun::Parallel { branches } => {
                    for branch in branches {
                        walk(branch, use_);
                    }
                }
                // `Check` is covered by walking `workflow.checks` wholesale
                // below; `Deterministic` by walking `workflow.steps`.
                StepRun::Agent(_)
                | StepRun::Tool(_)
                | StepRun::Deterministic(_)
                | StepRun::Check(_)
                | StepRun::Suspend { .. }
                | StepRun::Dynamic { .. } => {}
            }
        }
    }

    // Paths (3) and (4): every deterministic node in the table, whether or
    // not this scan can see who references it.
    for node in module.workflow.steps.values() {
        use_.any = true;
        if node.fn_ref.name == FN_MATCHES {
            use_.matches = true;
        }
    }

    // Path (2): every goal check, whether or not the pipeline reaches it.
    for check in module.workflow.checks.values() {
        if let CheckVerify::Goal { predicate, .. } = &check.verify {
            note_predicate(predicate, &mut use_);
        }
    }

    // Path (1): Branch/Loop conditions, nested arbitrarily deep.
    if let Some(pipeline) = &module.workflow.pipeline {
        walk(&pipeline.steps, &mut use_);
    }

    use_
}

/// Decode baked IR bytes and scan them, failing OPEN on anything unreadable.
///
/// An empty file is the standalone/CI build (no `TAU_IR_BYTES`): the guest's
/// `run` returns its error arm before touching the interpreter, so nothing
/// can execute, but we still link everything so that build keeps exercising
/// the full path as a link gate. A decode error is reported as a
/// `cargo:warning` and likewise links everything, leaving `guest.rs` to
/// surface the real error at run time rather than failing the build here
/// with a worse message.
fn scan_baked_ir(bytes: &[u8]) -> GoalUse {
    if bytes.is_empty() {
        return GoalUse::LINK_EVERYTHING;
    }
    match tau_ir::from_canonical_bytes(bytes) {
        Ok(module) => scan(&module),
        Err(e) => {
            println!(
                "cargo:warning=tau-wasm-guest: could not decode TAU_IR_BYTES ({e}); \
                 linking the full goal-predicate registry. The guest will report \
                 this decode failure at run time."
            );
            GoalUse::LINK_EVERYTHING
        }
    }
}
