//! Walk an [`IrModule`] and collect the set of optional [`IrFeature`]s it uses.
//!
//! Only the EPIC 4.1 control-flow constructs count as features; leaf steps
//! (agent/tool/deterministic/check) are the base surface. The result feeds
//! `tau-ir-lower`'s `feature_fit` build-time gate, which refuses a workflow that
//! uses a feature the target cannot execute (e.g. any control-flow for wasm).

use alloc::collections::BTreeSet;

use tau_domain::IrFeature;

use crate::module::IrModule;
use crate::pipeline::{Pipeline, PipelineStep, StepRun};

/// Collect every [`IrFeature`] used anywhere in `module`'s pipeline, recursing
/// through nested control-flow bodies. Returns an empty set for a leaf-only
/// pipeline (or a module with no pipeline).
pub fn features_used(module: &IrModule) -> BTreeSet<IrFeature> {
    match &module.workflow.pipeline {
        Some(pipeline) => features_in_pipeline(pipeline),
        None => BTreeSet::new(),
    }
}

/// Collect every [`IrFeature`] used in `pipeline`, recursing through nested
/// control-flow bodies. Used by `tau-ir-lower`'s `feature_fit`, which has a
/// pipeline (from the pre-`IrModule` `Parsed` workflow) rather than a module.
pub fn features_in_pipeline(pipeline: &Pipeline) -> BTreeSet<IrFeature> {
    let mut found = BTreeSet::new();
    for step in &pipeline.steps {
        collect_step(step, &mut found);
    }
    found
}

fn collect_step(step: &PipelineStep, found: &mut BTreeSet<IrFeature>) {
    match &step.run {
        StepRun::Agent(_) | StepRun::Tool(_) | StepRun::Deterministic(_) | StepRun::Check(_) => {}
        StepRun::Branch {
            then, otherwise, ..
        } => {
            found.insert(IrFeature::Branch);
            for s in then.iter().chain(otherwise.iter()) {
                collect_step(s, found);
            }
        }
        StepRun::Parallel { branches } => {
            found.insert(IrFeature::Parallel);
            for s in branches.iter().flatten() {
                collect_step(s, found);
            }
        }
        StepRun::Loop { body, .. } => {
            found.insert(IrFeature::Loop);
            for s in body {
                collect_step(s, found);
            }
        }
        StepRun::Suspend { .. } => {
            found.insert(IrFeature::Suspend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentId, PipelineStepId};
    use crate::module::{IrFormatVersion, Workflow};
    use crate::pipeline::Pipeline;
    use alloc::vec;
    use tau_ports::target::registry;

    fn leaf(id: &str) -> PipelineStep {
        PipelineStep {
            id: PipelineStepId(id.into()),
            run: StepRun::Agent(AgentId(id.into())),
            input: "${input}".into(),
        }
    }

    fn module_with(steps: vec::Vec<PipelineStep>) -> IrModule {
        let workflow = Workflow {
            pipeline: Some(Pipeline { steps }),
            ..Workflow::default()
        };
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target: registry::list_available().next().unwrap().triple,
            workflow,
            triggers: vec![],
        }
    }

    #[test]
    fn leaf_only_pipeline_uses_no_features() {
        let m = module_with(vec![leaf("a"), leaf("b")]);
        assert!(features_used(&m).is_empty());
    }

    #[test]
    fn nested_control_flow_is_collected_recursively() {
        // Parallel whose branch contains a Loop whose body contains a Branch.
        let inner_branch = PipelineStep {
            id: PipelineStepId("br".into()),
            run: StepRun::Branch {
                on: cond(),
                then: vec![leaf("t")],
                otherwise: vec![],
            },
            input: "${input}".into(),
        };
        let loop_step = PipelineStep {
            id: PipelineStepId("lp".into()),
            run: StepRun::Loop {
                body: vec![inner_branch],
                until: cond(),
                max_iters: 3,
            },
            input: "${input}".into(),
        };
        let parallel = PipelineStep {
            id: PipelineStepId("par".into()),
            run: StepRun::Parallel {
                branches: vec![vec![loop_step]],
            },
            input: "${input}".into(),
        };
        let feats = features_used(&module_with(vec![parallel]));
        assert!(feats.contains(&IrFeature::Parallel));
        assert!(feats.contains(&IrFeature::Loop));
        assert!(feats.contains(&IrFeature::Branch));
        assert_eq!(feats.len(), 3);
    }

    fn cond() -> crate::check::Condition {
        crate::check::Condition {
            evaluates: crate::check::Locus::Output(PipelineStepId("t".into())),
            predicate: crate::check::GoalPredicate::NonEmpty,
        }
    }
}
