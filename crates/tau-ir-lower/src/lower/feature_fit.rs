//! EPIC 4.2: strict build-time IR-feature-fit check.
//!
//! The sibling of [`capability_fit`](super::capability_fit) one level up: where
//! capability-fit asks "does the target enforce the capability shapes this
//! workflow needs?", feature-fit asks "can the target *execute* the IR features
//! this workflow uses?". Refuses the build (`LowerError::FeatureUnsupported`) if
//! the workflow uses any [`IrFeature`] absent from the target's
//! `supported_features`. **No override flag** — matches the Rust-like
//! build-time enforcement principle.
//!
//! As of ADR-0068 (#621), `any-wasi-strict` lists `[Branch, Parallel, Loop]`:
//! the wasm guest executes these in-guest via `run_pipeline`. `Suspend` (no
//! `SuspensionStore` channel in the WIT world) and `Dynamic` (EPIC 4.5
//! pending) stay absent from `supported_features`, so a workflow using either
//! is refused for wasm here. Every native/host target supports all features,
//! so this check is a no-op for them.

use alloc::vec::Vec;
use tau_domain::IrFeature;
use tau_ports::target::{registry, TargetTriple};

use crate::error::LowerError;

use super::parse::Parsed;

/// Run the feature-fit check on a `Parsed` workflow against a target.
///
/// Returns `Ok(())` if every [`IrFeature`] the workflow uses is listed in the
/// target's `supported_features`. Returns `Err(LowerError::FeatureUnsupported)`
/// with the sorted list of unsupported features otherwise. An unknown target
/// triple also fails (with an empty `missing` list) — the caller's diagnostic
/// handles this as "unknown target", mirroring `capability_fit`.
pub(super) fn check(parsed: &Parsed, target: &TargetTriple) -> Result<(), LowerError> {
    let entry = registry::lookup(target).ok_or_else(|| LowerError::FeatureUnsupported {
        missing: Vec::new(),
        target: *target,
    })?;
    let supported = entry.supported_features;

    let Some(pipeline) = &parsed.workflow.pipeline else {
        return Ok(());
    };
    let used = tau_ir::features_in_pipeline(pipeline);

    let missing: Vec<IrFeature> = used
        .into_iter()
        .filter(|f| !supported.contains(f))
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(LowerError::FeatureUnsupported {
            missing,
            target: *target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::parse;
    use crate::lower::parse::no_prompt_files;
    use tau_pkg::project::ProjectConfig;

    const BRANCH_TOML: &str = r#"
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

    const LEAF_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "a"
run = "agent:a"
input = "${input}"
"#;

    const DYNAMIC_TOML: &str = r#"
[project]
name = "demo"

[agent.kinds.researcher]
capabilities = {}

[[pipeline.steps]]
id = "fanout"
[pipeline.steps.dynamic]
spawns = ["researcher"]
ceiling = {}
max_spawns = 1
max_concurrency = 1
"#;

    const SUSPEND_TOML: &str = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "a"
run = "agent:a"
input = "${input}"

[[pipeline.steps]]
id = "pause"
run = "suspend:human"
"#;

    fn parsed(toml: &str) -> Parsed {
        let config = ProjectConfig::parse_str(toml).expect("toml parses");
        parse::parse(&config, &no_prompt_files).expect("parse stage")
    }

    #[test]
    fn native_target_accepts_control_flow() {
        let t: TargetTriple = "linux-native-strict".parse().unwrap();
        assert!(check(&parsed(BRANCH_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_target_accepts_branch() {
        let t: TargetTriple = "any-wasi-strict".parse().unwrap();
        assert!(check(&parsed(BRANCH_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_target_accepts_leaf_only_pipeline() {
        let t: TargetTriple = "any-wasi-strict".parse().unwrap();
        assert!(check(&parsed(LEAF_TOML), &t).is_ok());
    }

    #[test]
    fn wasm_target_rejects_suspend() {
        let t: TargetTriple = "any-wasi-strict".parse().unwrap();
        let err = check(&parsed(SUSPEND_TOML), &t).expect_err("wasm must refuse Suspend");
        match err {
            LowerError::FeatureUnsupported { missing, target } => {
                assert_eq!(missing, alloc::vec![IrFeature::Suspend]);
                assert_eq!(target, t);
            }
            other => panic!("expected FeatureUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn wasm_target_rejects_dynamic_region() {
        // EPIC 4.4: a `StepRun::Dynamic` region is control-flow the wasm
        // guest's `run_ir_streaming` path cannot execute, same as
        // Branch/Parallel/Loop/Suspend — refuse the build.
        let t: TargetTriple = "any-wasi-strict".parse().unwrap();
        let err = check(&parsed(DYNAMIC_TOML), &t).expect_err("wasm must refuse dynamic regions");
        match err {
            LowerError::FeatureUnsupported { missing, target } => {
                assert_eq!(missing, alloc::vec![IrFeature::Dynamic]);
                assert_eq!(target, t);
            }
            other => panic!("expected FeatureUnsupported, got {other:?}"),
        }
    }
}
