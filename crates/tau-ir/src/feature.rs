//! Walked feature-fit: the set of IR features a module actually uses.
//!
//! Derived by WALKING the module (recursing nested control-flow bodies), so
//! the set can never drift from the module's real shape — there is no
//! declared feature list to lie.

use alloc::collections::BTreeSet;

use tau_ports::target::adapter_family::AdapterFamily;

use crate::module::IrModule;
use crate::pipeline::{PipelineStep, StepRun};
use crate::tool_impl::ToolImpl;

/// A capability the IR can require of an executing backend. `#[non_exhaustive]`
/// so new IR shapes can extend it without a breaking change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IrFeature {
    /// An engine-sequenced `workflow.pipeline` is present.
    Pipeline,
    /// A `StepRun::Branch` block.
    Branch,
    /// A `StepRun::Parallel` block.
    Parallel,
    /// A `StepRun::Loop` block.
    Loop,
    /// A `StepRun::Suspend` block.
    Suspend,
    /// Postcondition checks (`workflow.checks` / `StepRun::Check`).
    Checks,
    /// Subflow edges or `ToolImpl::Subflow` tools.
    Subflow,
    /// MCP-contracted tools (`ToolImpl::Mcp`).
    McpTools,
    /// Statically-linked native tools (`ToolImpl::Native`).
    NativeTools,
    /// Deterministic step nodes (`workflow.steps` / `ToolImpl::Step`).
    DeterministicSteps,
    /// Trigger bindings.
    Triggers,
}

/// The set of [`IrFeature`]s this module requires an executing backend to
/// support. Walks the whole module, recursing nested control-flow bodies.
pub fn required_features(m: &IrModule) -> BTreeSet<IrFeature> {
    let mut f = BTreeSet::new();
    let wf = &m.workflow;

    if !m.triggers.is_empty() {
        f.insert(IrFeature::Triggers);
    }
    if !wf.checks.is_empty() {
        f.insert(IrFeature::Checks);
    }
    if !wf.edges.is_empty() {
        f.insert(IrFeature::Subflow);
    }
    if !wf.steps.is_empty() {
        f.insert(IrFeature::DeterministicSteps);
    }
    for tool in wf.tools.values() {
        match &tool.impl_ {
            ToolImpl::Native { .. } => {
                f.insert(IrFeature::NativeTools);
            }
            ToolImpl::Mcp { .. } => {
                f.insert(IrFeature::McpTools);
            }
            ToolImpl::Subflow { .. } => {
                f.insert(IrFeature::Subflow);
            }
            ToolImpl::Step { .. } => {
                f.insert(IrFeature::DeterministicSteps);
            }
        }
    }
    if let Some(pipeline) = &wf.pipeline {
        f.insert(IrFeature::Pipeline);
        for step in &pipeline.steps {
            walk_step(step, &mut f);
        }
    }
    f
}

/// The features an executing backend for `family` supports today. There is a
/// single backend (the `tau-runtime-core` interpreter) behind every target,
/// so every family maps to the same set until a divergent backend ships.
/// EPIC 4.2 (#399) adds `Branch`/`Parallel`/`Loop`/`Suspend` here.
pub fn backend_features(family: AdapterFamily) -> BTreeSet<IrFeature> {
    let interpreter = || {
        let mut f = BTreeSet::new();
        for x in [
            IrFeature::Pipeline,
            IrFeature::Checks,
            IrFeature::Subflow,
            IrFeature::McpTools,
            IrFeature::NativeTools,
            IrFeature::DeterministicSteps,
            IrFeature::Triggers,
        ] {
            f.insert(x);
        }
        f
    };
    match family {
        AdapterFamily::Native
        | AdapterFamily::Container
        | AdapterFamily::Remote
        | AdapterFamily::Wasi
        | AdapterFamily::Passthrough => interpreter(),
        // `AdapterFamily` is `#[non_exhaustive]` and defined in `tau-ports`, so
        // rustc requires this wildcard even though every known variant is
        // listed above. A future variant added upstream lands here silently
        // (same interpreter set) rather than failing to compile — the
        // exhaustive listing above is what actually forces a reviewer's eye
        // when a new family is introduced.
        _ => interpreter(),
    }
}

/// Recurse one pipeline step, recording its feature and descending into any
/// nested bodies. Mirrors the typecheck walk (`validate_step_run`) so it
/// cannot miss what typecheck sees.
fn walk_step(step: &PipelineStep, f: &mut BTreeSet<IrFeature>) {
    match &step.run {
        StepRun::Agent(_) | StepRun::Tool(_) | StepRun::Deterministic(_) => {}
        StepRun::Check(_) => {
            f.insert(IrFeature::Checks);
        }
        StepRun::Branch {
            then, otherwise, ..
        } => {
            f.insert(IrFeature::Branch);
            for s in then.iter().chain(otherwise.iter()) {
                walk_step(s, f);
            }
        }
        StepRun::Parallel { branches } => {
            f.insert(IrFeature::Parallel);
            for branch in branches {
                for s in branch {
                    walk_step(s, f);
                }
            }
        }
        StepRun::Loop { body, .. } => {
            f.insert(IrFeature::Loop);
            for s in body {
                walk_step(s, f);
            }
        }
        StepRun::Suspend { .. } => {
            f.insert(IrFeature::Suspend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{Condition, GoalPredicate, Locus};
    use crate::ids::{AgentId, PipelineStepId};
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use crate::pipeline::{Pipeline, PipelineStep, StepRun};
    use tau_ports::target::registry;

    fn module_with(pipeline: Option<Pipeline>) -> IrModule {
        let target = registry::list_available().next().unwrap().triple;
        IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow {
                pipeline,
                ..Workflow::default()
            },
            triggers: alloc::vec::Vec::new(),
        }
    }

    #[test]
    fn agent_only_module_requires_nothing() {
        let m = module_with(None);
        assert!(required_features(&m).is_empty());
    }

    #[test]
    fn pipeline_requires_pipeline_feature() {
        let m = module_with(Some(Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("a".into()),
                run: StepRun::Agent(AgentId("a".into())),
                input: "${input}".into(),
            }],
        }));
        let f = required_features(&m);
        assert!(f.contains(&IrFeature::Pipeline));
        assert!(!f.contains(&IrFeature::Branch));
    }

    #[test]
    fn backend_omits_unimplemented_control_flow_today() {
        use tau_ports::target::adapter_family::AdapterFamily;
        let native = backend_features(AdapterFamily::Native);
        // Implemented today:
        assert!(native.contains(&IrFeature::Pipeline));
        assert!(native.contains(&IrFeature::Checks));
        // NOT implemented until EPIC 4.2:
        assert!(!native.contains(&IrFeature::Branch));
        assert!(!native.contains(&IrFeature::Parallel));
        assert!(!native.contains(&IrFeature::Loop));
        assert!(!native.contains(&IrFeature::Suspend));
        // One interpreter today ⇒ every family maps to the same set.
        assert_eq!(native, backend_features(AdapterFamily::Wasi));
        assert_eq!(native, backend_features(AdapterFamily::Passthrough));
    }

    #[test]
    fn nested_branch_inside_loop_is_walked() {
        let inner = PipelineStep {
            id: PipelineStepId("inner".into()),
            run: StepRun::Branch {
                on: Condition {
                    evaluates: Locus::Path("/x".into()),
                    predicate: GoalPredicate::Exists,
                },
                then: alloc::vec![],
                otherwise: alloc::vec![],
            },
            input: "${input}".into(),
        };
        let m = module_with(Some(Pipeline {
            steps: alloc::vec![PipelineStep {
                id: PipelineStepId("l".into()),
                run: StepRun::Loop {
                    body: alloc::vec![inner],
                    until: Condition {
                        evaluates: Locus::Path("/y".into()),
                        predicate: GoalPredicate::Exists
                    },
                    max_iters: 3,
                },
                input: "${input}".into(),
            }],
        }));
        let f = required_features(&m);
        assert!(f.contains(&IrFeature::Loop));
        assert!(f.contains(&IrFeature::Branch)); // proves the walk recurses bodies
    }
}
