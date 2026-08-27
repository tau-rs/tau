//! Task 4: wasm-only build-time fn-availability gate (`predicate_fit`).
//!
//! Sibling of [`feature_fit`](super::feature_fit): where feature-fit asks
//! "can the target *execute* this IR shape?", predicate-fit asks "can the
//! wasm guest's no_std goal-predicate registry *answer* this fn?"
//! ADR-0068's guest drives `run_pipeline` in-guest against a registry that
//! answers only the five no_std goal predicates
//! (`tau_native_tools::goal_predicates::SUPPORTED`):
//! `__tau::goal::{exists,non_empty,equals,matches,min_count}`. Anything else
//! — `GoalPredicate::SchemaValid` (needs the std-only `jsonschema` crate),
//! `GoalPredicate::NativeFn` (a user-registered fn with no guest impl), or a
//! `Deterministic` step whose `fn_ref.name` is outside the five — has no
//! guest execution path and refuses the build for Wasi targets. Every
//! native/host target is unaffected — this gate is a no-op for every
//! non-`Wasi` `AdapterFamily`. **No override flag**, matching the Rust-like
//! build-time enforcement principle `feature_fit` follows.

use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};

use tau_ir::check::{CheckVerify, Condition, GoalPredicate};
use tau_ir::pipeline::{PipelineStep, StepRun};
use tau_ports::target::{AdapterFamily, TargetTriple};

use crate::error::LowerError;

use super::parse::Parsed;

/// The five goal-predicate fn names the wasm guest's no_std registry can
/// answer.
///
/// Kept as a local literal (rather than `tau_native_tools::goal_predicates::
/// SUPPORTED`) so `tau-ir-lower`'s *production* dependency graph does not
/// gain a `tau-native-tools`(`goal-predicates`) edge — that feature pulls in
/// `regex-automata`, and every crate that lowers IR for a *native* target
/// (not just wasm builds) would pay for it transitively. `tau-ir-lower` is a
/// dependency of `tau-cli`, `tau-ts-extract`, `tau-sdk-codegen`, and
/// `tau-wasm-host`, so a production edge here is workspace-wide, not
/// wasm-scoped. `tests::guest_fns_match_tau_native_tools_supported` pins
/// this list against the authoritative constant via a dev-dependency, which
/// only affects this crate's own test builds.
const GUEST_FNS: &[&str; 5] = &[
    "__tau::goal::exists",
    "__tau::goal::non_empty",
    "__tau::goal::equals",
    "__tau::goal::matches",
    "__tau::goal::min_count",
];

/// Run the predicate-fit check on a `Parsed` workflow against a target.
///
/// Returns `Ok(())` for every non-Wasi target (this gate is wasm-only) and
/// for a Wasi target whose pipeline uses only guest-answerable goal
/// predicates. Returns `Err(LowerError::WasmFnUnavailable)` with the sorted,
/// deduped list of offending fn names otherwise.
pub(super) fn check(parsed: &Parsed, target: &TargetTriple) -> Result<(), LowerError> {
    if target.adapter_family != AdapterFamily::Wasi {
        return Ok(());
    }
    let Some(pipeline) = &parsed.workflow.pipeline else {
        return Ok(());
    };

    let mut offending: BTreeSet<String> = BTreeSet::new();
    walk(&pipeline.steps, parsed, &mut offending);

    if offending.is_empty() {
        Ok(())
    } else {
        Err(LowerError::WasmFnUnavailable {
            fn_names: offending.into_iter().collect(),
            target: *target,
        })
    }
}

/// Recursively walk a pipeline step slice, collecting offending fn names.
fn walk(steps: &[PipelineStep], parsed: &Parsed, out: &mut BTreeSet<String>) {
    for step in steps {
        match &step.run {
            StepRun::Branch {
                on,
                then,
                otherwise,
            } => {
                condition(on, out);
                walk(then, parsed, out);
                walk(otherwise, parsed, out);
            }
            StepRun::Loop { body, until, .. } => {
                condition(until, out);
                walk(body, parsed, out);
            }
            StepRun::Parallel { branches } => {
                for branch in branches {
                    walk(branch, parsed, out);
                }
            }
            StepRun::Check(id) => {
                if let Some(check) = parsed.workflow.checks.get(id) {
                    if let CheckVerify::Goal { predicate, .. } = &check.verify {
                        goal_predicate(predicate, out);
                    }
                    // CheckVerify::Deliverable runs an in-guest judge agent — allowed.
                }
            }
            StepRun::Deterministic(id) => {
                if let Some(node) = parsed.workflow.steps.get(id) {
                    if !GUEST_FNS.contains(&node.fn_ref.name.as_str()) {
                        out.insert(node.fn_ref.name.clone());
                    }
                }
            }
            StepRun::Agent(_)
            | StepRun::Tool(_)
            | StepRun::Suspend { .. }
            | StepRun::Dynamic { .. } => {}
        }
    }
}

/// A `Branch`/`Loop` condition reads a single goal predicate.
fn condition(cond: &Condition, out: &mut BTreeSet<String>) {
    goal_predicate(&cond.predicate, out);
}

/// Flag `SchemaValid`/`NativeFn` as offending; the five guest-answerable
/// predicates are no-ops.
fn goal_predicate(predicate: &GoalPredicate, out: &mut BTreeSet<String>) {
    match predicate {
        GoalPredicate::SchemaValid(_) => {
            out.insert("__tau::goal::schema_valid".to_string());
        }
        GoalPredicate::NativeFn(r) => {
            out.insert(r.name.clone());
        }
        GoalPredicate::Exists
        | GoalPredicate::NonEmpty
        | GoalPredicate::Equals(_)
        | GoalPredicate::Matches(_)
        | GoalPredicate::MinCount(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::parse;
    use crate::lower::parse::no_prompt_files;
    use alloc::vec::Vec;
    use tau_ir::check::Check;
    use tau_ir::check::{JudgeRef, Locus, OnFail, RetryPolicy};
    use tau_ir::ids::{CheckId, PipelineStepId};
    use tau_ir::model_ref::ModelRef;
    use tau_pkg::project::ProjectConfig;

    /// `check = "schema_valid"` on a `Branch` condition — mirrors
    /// `feature_fit`'s `BRANCH_TOML` fixture but swaps the predicate.
    const SCHEMA_VALID_BRANCH_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "schema_valid", schema = {} }
[[pipeline.steps.then]]
id = "go"
run = "agent:triage"
"#;

    /// `check = "non_empty"` on a `Branch` condition — every predicate is
    /// guest-answerable.
    const NON_EMPTY_BRANCH_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "triage"
run = "agent:triage"
input = "${input}"

[[pipeline.steps]]
id = "route"
branch = { evaluates = "steps.triage.output", check = "non_empty" }
[[pipeline.steps.then]]
id = "go"
run = "agent:triage"
"#;

    /// `fn = "custom::loop_fn"` on a `Loop` condition — `NativeFn`, not
    /// guest-answerable.
    const NATIVE_FN_LOOP_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "refine"
until = { evaluates = "steps.draft.output", fn = "custom::loop_fn" }
max_iters = 3
[[pipeline.steps.body]]
id = "draft"
run = "agent:writer"
input = "${input}"
"#;

    /// `[goals.structured]` with `check = "schema_valid"` — auto-appended as
    /// a `StepRun::Check` step (exercises the `Check`/`CheckVerify::Goal`
    /// walker arm).
    const SCHEMA_VALID_GOAL_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "gather"
run = "agent:triage"
input = "${input}"

[goals.structured]
evaluates = "steps.gather.output"
check = "schema_valid"
schema = {}
"#;

    /// A `deterministic:<id>` step whose `[steps.<id>]` fn is outside the
    /// five guest-answerable names.
    const UNSUPPORTED_DETERMINISTIC_TOML: &str = r#"
[project]
name = "demo"

[steps.custom]
deterministic = "custom_transform"

[[pipeline.steps]]
id = "d"
run = "deterministic:custom"
input = "${input}"
"#;

    /// A `deterministic:<id>` step whose fn happens to be named one of the
    /// five guest-answerable predicate names — allowed.
    const GUEST_FN_DETERMINISTIC_TOML: &str = r#"
[project]
name = "demo"

[steps.custom]
deterministic = "__tau::goal::exists"

[[pipeline.steps]]
id = "d"
run = "deterministic:custom"
input = "${input}"
"#;

    /// Two parallel branches, each with a `Deterministic` step referencing
    /// an unsupported fn (dedup/sort coverage: `zzz` sorts after `custom`).
    const PARALLEL_UNSUPPORTED_DETERMINISTIC_TOML: &str = r#"
[project]
name = "demo"

[steps.custom]
deterministic = "zzz_transform"

[steps.other]
deterministic = "custom_transform"

[[pipeline.steps]]
id = "fanout"
[[pipeline.steps.branches]]
[[pipeline.steps.branches.steps]]
id = "a"
run = "deterministic:custom"
[[pipeline.steps.branches]]
[[pipeline.steps.branches.steps]]
id = "b"
run = "deterministic:other"
"#;

    fn parsed(toml: &str) -> Parsed {
        let config = ProjectConfig::parse_str(toml).expect("toml parses");
        parse::parse(&config, &no_prompt_files).expect("parse stage")
    }

    fn wasm() -> TargetTriple {
        "any-wasi-strict".parse().unwrap()
    }

    fn native() -> TargetTriple {
        "linux-native-strict".parse().unwrap()
    }

    #[test]
    fn wasm_rejects_schema_valid_branch_condition() {
        let t = wasm();
        let err = check(&parsed(SCHEMA_VALID_BRANCH_TOML), &t)
            .expect_err("wasm must refuse schema_valid");
        match err {
            LowerError::WasmFnUnavailable { fn_names, target } => {
                assert_eq!(
                    fn_names,
                    alloc::vec!["__tau::goal::schema_valid".to_string()]
                );
                assert_eq!(target, t);
            }
            other => panic!("expected WasmFnUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn wasm_accepts_non_empty_branch_condition() {
        let t = wasm();
        assert!(check(&parsed(NON_EMPTY_BRANCH_TOML), &t).is_ok());
    }

    #[test]
    fn native_target_accepts_schema_valid_branch_condition() {
        // The gate is Wasi-only: a native target never refuses on predicate
        // availability, no matter which predicate is used.
        let t = native();
        assert!(check(&parsed(SCHEMA_VALID_BRANCH_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_rejects_native_fn_loop_condition() {
        let t = wasm();
        let err = check(&parsed(NATIVE_FN_LOOP_TOML), &t).expect_err("wasm must refuse NativeFn");
        match err {
            LowerError::WasmFnUnavailable { fn_names, target } => {
                assert_eq!(fn_names, alloc::vec!["custom::loop_fn".to_string()]);
                assert_eq!(target, t);
            }
            other => panic!("expected WasmFnUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn native_target_accepts_native_fn_loop_condition() {
        let t = native();
        assert!(check(&parsed(NATIVE_FN_LOOP_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_rejects_schema_valid_check_step() {
        let t = wasm();
        let err = check(&parsed(SCHEMA_VALID_GOAL_TOML), &t)
            .expect_err("wasm must refuse a schema_valid goal check");
        match err {
            LowerError::WasmFnUnavailable { fn_names, target } => {
                assert_eq!(
                    fn_names,
                    alloc::vec!["__tau::goal::schema_valid".to_string()]
                );
                assert_eq!(target, t);
            }
            other => panic!("expected WasmFnUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn native_target_accepts_schema_valid_check_step() {
        let t = native();
        assert!(check(&parsed(SCHEMA_VALID_GOAL_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_rejects_unsupported_deterministic_fn() {
        let t = wasm();
        let err = check(&parsed(UNSUPPORTED_DETERMINISTIC_TOML), &t)
            .expect_err("wasm must refuse an unregistered deterministic fn");
        match err {
            LowerError::WasmFnUnavailable { fn_names, target } => {
                assert_eq!(fn_names, alloc::vec!["custom_transform".to_string()]);
                assert_eq!(target, t);
            }
            other => panic!("expected WasmFnUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn wasm_accepts_deterministic_fn_named_like_a_guest_predicate() {
        let t = wasm();
        assert!(check(&parsed(GUEST_FN_DETERMINISTIC_TOML), &t).is_ok());
    }

    #[test]
    fn native_target_accepts_unsupported_deterministic_fn() {
        let t = native();
        assert!(check(&parsed(UNSUPPORTED_DETERMINISTIC_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_walks_parallel_branches_sorted_and_deduped() {
        let t = wasm();
        let err = check(&parsed(PARALLEL_UNSUPPORTED_DETERMINISTIC_TOML), &t)
            .expect_err("wasm must refuse both parallel branches' fns");
        match err {
            LowerError::WasmFnUnavailable { fn_names, target } => {
                // BTreeSet ordering: "custom_transform" < "zzz_transform".
                assert_eq!(
                    fn_names,
                    alloc::vec!["custom_transform".to_string(), "zzz_transform".to_string()]
                );
                assert_eq!(target, t);
            }
            other => panic!("expected WasmFnUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn wasm_accepts_leaf_only_pipeline() {
        // Agent-only steps: the default no-op arm of the walker.
        let t = wasm();
        let toml = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "a"
run = "agent:a"
input = "${input}"
"#;
        assert!(check(&parsed(toml), &t).is_ok());
    }

    #[test]
    fn wasm_no_pipeline_is_ok() {
        let t = wasm();
        let toml = r#"
[project]
name = "demo"
"#;
        assert!(check(&parsed(toml), &t).is_ok());
    }

    /// `CheckVerify::Deliverable` is not TOML-authorable in a way that
    /// isolates just this walker arm without a full judge/model fixture, so
    /// per the brief this arm is covered with a direct-IR `Parsed` value
    /// instead: a `Deliverable` check produces no offending fn names — it
    /// runs an in-guest judge agent, not a goal predicate.
    #[test]
    fn wasm_accepts_deliverable_check_step_direct_ir() {
        use alloc::collections::BTreeMap;
        use tau_ir::ids::AgentId;
        use tau_ir::module::Workflow;
        use tau_ir::pipeline::{Pipeline, PipelineStep};

        let mut checks = BTreeMap::new();
        checks.insert(
            CheckId("d".into()),
            Check {
                id: CheckId("d".into()),
                verify: CheckVerify::Deliverable {
                    locus: Locus::Output(PipelineStepId("a".into())),
                    must_satisfy: "is good".into(),
                    judge: JudgeRef::Default {
                        model_ref: ModelRef {
                            backend: "d".into(),
                            model_id: "m".into(),
                        },
                    },
                },
                retry: RetryPolicy {
                    on_fail: OnFail::Abort,
                    max_attempts: 1,
                    gate: PipelineStepId("d".into()),
                },
            },
        );
        let workflow = Workflow {
            pipeline: Some(Pipeline {
                steps: alloc::vec![
                    PipelineStep {
                        id: PipelineStepId("a".into()),
                        run: StepRun::Agent(AgentId("a".into())),
                        input: "${input}".into(),
                    },
                    PipelineStep {
                        id: PipelineStepId("d".into()),
                        run: StepRun::Check(CheckId("d".into())),
                        input: "${steps.a.output}".into(),
                    },
                ],
            }),
            checks,
            ..Default::default()
        };
        let parsed = Parsed {
            workflow,
            triggers: Vec::new(),
            assets: BTreeMap::new(),
        };
        assert!(check(&parsed, &wasm()).is_ok());
    }

    /// Pins the local [`GUEST_FNS`] literal against the authoritative
    /// `tau_native_tools::goal_predicates::SUPPORTED` constant so the two
    /// lists cannot silently drift (see [`GUEST_FNS`]'s doc comment for why
    /// this crate does not depend on `tau-native-tools` in production).
    #[test]
    fn guest_fns_match_tau_native_tools_supported() {
        let mut ours: Vec<&str> = GUEST_FNS.to_vec();
        ours.sort_unstable();
        let mut theirs: Vec<&str> = tau_native_tools::goal_predicates::SUPPORTED.to_vec();
        theirs.sort_unstable();
        assert_eq!(ours, theirs);
    }
}
