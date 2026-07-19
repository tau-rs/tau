//! Strict build-time IR feature-fit (mirrors `capability_fit`). Refuses the
//! build if the workflow walks any [`IrFeature`] the target's backend does
//! not support. **No override flag.**

use alloc::vec::Vec;
use tau_ir::feature::{backend_features, required_features, IrFeature};
use tau_ir::module::IrModule;
use tau_ports::target::TargetTriple;

use crate::error::LowerError;

/// Returns `Ok(())` iff every feature the module requires is supported by the
/// target's backend profile. On a miss, `Err(LowerError::FeatureFitFailed)`
/// with the full unsupported set.
pub(super) fn check(module: &IrModule, target: &TargetTriple) -> Result<(), LowerError> {
    let supported = backend_features(target.adapter_family);
    let required = required_features(module);
    let unsupported: Vec<IrFeature> = required.difference(&supported).copied().collect();
    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(LowerError::FeatureFitFailed {
            unsupported,
            target: *target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::check::{Condition, GoalPredicate, Locus};
    use tau_ir::ids::PipelineStepId;
    use tau_ir::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
    use tau_ports::target::registry;

    #[test]
    fn branch_module_is_rejected_today() {
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow {
                pipeline: Some(Pipeline {
                    steps: alloc::vec![PipelineStep {
                        id: PipelineStepId("b".into()),
                        run: StepRun::Branch {
                            on: Condition {
                                evaluates: Locus::Path("/x".into()),
                                predicate: GoalPredicate::Exists,
                            },
                            then: alloc::vec![],
                            otherwise: alloc::vec![],
                        },
                        input: "${input}".into(),
                    }],
                }),
                ..Workflow::default()
            },
            triggers: alloc::vec::Vec::new(),
        };
        assert!(matches!(
            check(&m, &target),
            Err(LowerError::FeatureFitFailed { .. })
        ));
    }

    #[test]
    fn agent_only_module_passes() {
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        };
        assert!(check(&m, &target).is_ok());
    }
}
