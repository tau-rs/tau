//! IR-level errors raised during parsing, lowering, capability-fit
//! checking, canonicalization, and hashing.

use alloc::string::String;
use alloc::vec::Vec;
use tau_domain::{CapabilityShape, IrFeature};
use thiserror::Error;

use tau_ir::ids::{AgentId, StepId, SubflowId, ToolId};
use tau_ports::target::TargetTriple;

/// IR-level error type.
#[derive(Debug, Error)]
pub enum LowerError {
    /// Workflow-shape error: an Agent references a Tool that doesn't
    /// exist in the workflow.
    #[error("agent {agent:?} references unknown tool {tool:?}")]
    UnknownToolRef {
        /// Agent that contains the bad reference.
        agent: AgentId,
        /// The unknown tool id.
        tool: ToolId,
    },

    /// Workflow-shape error: a SubflowEdge::Spawn targets an Agent that
    /// doesn't exist.
    #[error("subflow {subflow:?} targets unknown agent {agent:?}")]
    UnknownSubflowTarget {
        /// The subflow.
        subflow: SubflowId,
        /// The unknown target.
        agent: AgentId,
    },

    /// Workflow-shape error: a SubflowEdge::Spawn's `cap_subset` is
    /// not a subset of the parent agent's grant.
    #[error("subflow {subflow:?}: cap_subset is not a subset of parent agent grant")]
    SubflowCapNotSubset {
        /// The offending subflow.
        subflow: SubflowId,
    },

    /// Capability-fit failure (D-3b). One or more required capability
    /// shapes are not supported by the build target.
    #[error("workflow needs unsupported capability shape(s) on target: {missing:?}")]
    CapabilityFitFailed {
        /// The shapes that the target does not support.
        missing: Vec<CapabilityShape>,
        /// Diagnostic: which tools required them.
        tools: Vec<ToolId>,
    },

    /// Feature-fit failure (EPIC 4.2). The workflow uses one or more IR
    /// execution features the build target cannot run — e.g. any control-flow
    /// (`Branch`/`Parallel`/`Loop`) for a wasm target, whose guest drives
    /// `run_ir_streaming` and has no `run_pipeline` control-flow path.
    #[error("workflow uses IR feature(s) unsupported by target {target}: {missing:?}")]
    FeatureUnsupported {
        /// The features the target does not support.
        missing: Vec<IrFeature>,
        /// The target that lacks them.
        target: TargetTriple,
    },

    /// A Deterministic step references a function name that the lowering
    /// registry doesn't know.
    #[error("deterministic step {step:?} references unknown fn `{fn_name}`")]
    UnknownDeterministicFn {
        /// The step id.
        step: StepId,
        /// The unresolved name.
        fn_name: String,
    },

    /// A `ToolImpl::Native` reference's content hash could not be resolved
    /// (the native tool registry did not know the symbolic name).
    #[error("native tool {tool:?} references unknown fn `{fn_name}`")]
    UnknownNativeTool {
        /// The tool id that contains the unresolved native ref.
        tool: ToolId,
        /// The native fn name that was not resolved.
        fn_name: String,
    },

    /// A `ToolImpl::Subflow` tool targets an agent that is not present in
    /// the workflow.
    #[error("subflow tool {tool:?} targets unknown agent {agent:?}")]
    UnknownSubflowToolTarget {
        /// The tool id whose `Subflow` variant points at a missing agent.
        tool: ToolId,
        /// The unresolved target agent id.
        agent: AgentId,
    },

    /// A `ToolImpl::Step` tool references a step id that is not present in
    /// the workflow's `steps` table.
    #[error("step tool {tool:?} references unknown step {step:?}")]
    UnknownStepToolTarget {
        /// The tool id whose `Step` variant points at a missing step.
        tool: ToolId,
        /// The unresolved step id.
        step: StepId,
    },

    /// An agent's `system_file` prompt could not be read at build time.
    /// This deliberately moves prompt-file existence from run time to build
    /// time (D6-B): a missing or unreadable prompt file is a hard build error.
    #[error("agent {agent:?}: cannot read prompt file {path:?}: {reason}")]
    PromptFileUnreadable {
        /// The agent whose `system_file` prompt failed to load.
        agent: AgentId,
        /// The prompt file path, as written in the config.
        path: String,
        /// Why the read failed (from the injected `prompt_file` reader).
        reason: String,
    },

    /// Generic parse failure surfacing from the upstream TOML parser.
    #[error("tau.toml parse error: {0}")]
    Parse(String),

    /// MCP-specific build error (per β.3 design doc §5).
    #[error("MCP build: {0}")]
    McpBuild(#[from] crate::lower::McpBuildError),

    /// SubflowEdge::Compose is not yet implemented (v0 reserves the variant).
    #[error("subflow {subflow:?}: Compose variant is not supported in v0")]
    UnsupportedComposeSubflow {
        /// The offending subflow.
        subflow: SubflowId,
    },

    /// A trigger binding names an entrypoint agent that is not present in
    /// the workflow.
    #[error("trigger {trigger:?} references unknown agent {agent:?}")]
    UnknownTriggerAgent {
        /// The trigger name.
        trigger: String,
        /// The unresolved entrypoint agent id.
        agent: AgentId,
    },

    /// A pipeline step's `run` target does not exist in the workflow.
    #[error("pipeline step {step:?}: run target {target} not found")]
    UnknownPipelineRun {
        /// The pipeline step id.
        step: String,
        /// The unresolved `kind:id` target, e.g. `agent:writer`.
        target: String,
    },

    /// Two pipeline steps share an id.
    #[error("pipeline step id {id:?} is declared more than once")]
    DuplicatePipelineStepId {
        /// The duplicated id.
        id: String,
    },

    /// `${steps.x.output}` references a step that runs at or after this one.
    #[error(
        "pipeline step {step:?} references output of {referenced:?}, which is not an earlier step"
    )]
    ForwardOutputRef {
        /// The referencing step.
        step: String,
        /// The referenced (later/self) step id.
        referenced: String,
    },

    /// `${steps.x.output}` references a step id not in the pipeline.
    #[error("pipeline step {step:?} references unknown step output {referenced:?}")]
    UnknownOutputRef {
        /// The referencing step.
        step: String,
        /// The unknown referenced id.
        referenced: String,
    },

    /// A pipeline input template was malformed (unterminated/unrecognized).
    #[error("pipeline step {step:?}: bad input template: {detail}")]
    BadPipelineTemplate {
        /// The step id.
        step: String,
        /// Human-readable template error.
        detail: String,
    },

    /// A `StepRun::Check` references a check id absent from `workflow.checks`.
    #[error("pipeline step '{step}' runs check '{check}' but no such check is defined")]
    UnknownCheckRef {
        /// The pipeline step id that contains the bad reference.
        step: String,
        /// The check id that was not found in `workflow.checks`.
        check: String,
    },

    /// A context pipeline names a transformer that is neither a known
    /// builtin nor a declared custom node.
    #[error("agent '{agent}': context transformer '{transformer}' is not a known builtin or custom node")]
    UnknownContextTransformer {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The offending transformer name.
        transformer: String,
    },

    /// A context pipeline's last step is not the builtin `fit_budget`.
    #[error("agent '{agent}': the last context step must be `fit_budget` (found '{last}')")]
    ContextFitBudgetNotLast {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The actual last transformer name.
        last: String,
    },

    /// A context pipeline repeats a transformer name.
    #[error("agent '{agent}': duplicate context transformer '{transformer}'")]
    DuplicateContextTransformer {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The repeated transformer name.
        transformer: String,
    },

    /// A check's `Locus::Output` references an unknown or later pipeline step.
    #[error(
        "check '{check}' evaluates output of '{output}' which is not an earlier pipeline step"
    )]
    UnknownCheckLocus {
        /// The check id whose locus is invalid.
        check: String,
        /// The referenced step id that is missing or comes at/after the check step.
        output: String,
    },

    /// A `StepRun::Loop` has `max_iters == 0`, which would never execute a
    /// body iteration. The bound must be at least 1.
    #[error("pipeline step '{step}': Loop.max_iters must be > 0")]
    LoopMaxItersZero {
        /// The pipeline step id containing the offending Loop.
        step: String,
    },

    /// A dynamic region references an agent kind with no `[agent.kinds.<name>]`
    /// definition (EPIC 4.4).
    #[error("dynamic region in step '{step}' spawns unknown agent kind '{kind}' — declare [agent.kinds.{kind}]")]
    UnknownAgentKind {
        /// The undefined kind name.
        kind: String,
        /// The pipeline-step id of the region.
        step: String,
    },

    /// A dynamic region references an agent kind that has no `prompt` or no
    /// `model` set, so it cannot actually be spawned at runtime (EPIC 4.5).
    #[error("dynamic region in step '{step}' offers kind '{kind}' which is not runnable — [agent.kinds.{kind}] must declare `prompt` and `model`")]
    DynamicKindNotRunnable {
        /// The kind name that is missing `prompt` and/or `model`.
        kind: String,
        /// The pipeline-step id of the region.
        step: String,
    },

    /// A `StepRun::Dynamic` has `max_spawns == 0`, which would never spawn a
    /// region member. The bound must be at least 1. Defense-in-depth: mirrors
    /// `LoopMaxItersZero` — tau-pkg validates this at author time, but
    /// hand-constructed IR that bypasses tau-pkg must still be rejected.
    #[error("pipeline step '{step}': Dynamic.max_spawns must be > 0")]
    DynamicMaxSpawnsZero {
        /// The pipeline step id containing the offending Dynamic region.
        step: String,
    },

    /// A `StepRun::Dynamic` has `max_concurrency` outside `1..=max_spawns`
    /// (either zero, or greater than the total-spawn cap). Defense-in-depth:
    /// mirrors `LoopMaxItersZero` for the Dynamic region's own bounds.
    #[error("pipeline step '{step}': Dynamic.max_concurrency must be in 1..={max_spawns} (got {max_concurrency})")]
    DynamicConcurrencyInvalid {
        /// The pipeline step id containing the offending Dynamic region.
        step: String,
        /// The region's configured total-spawn cap.
        max_spawns: u64,
        /// The offending concurrency value.
        max_concurrency: u64,
    },

    /// A context step declares a determinism class string that lowering does
    /// not recognize (D7-B / ADR-0065). Replaces the former silent
    /// `_ => DeterminismClass::Pure` default, which downgraded an unknown
    /// string to the most permissive class.
    #[error("agent '{agent}': context transformer '{transformer}' declares unknown determinism '{determinism}' (want one of: pure, llm_backed, stateful)")]
    UnknownDeterminism {
        /// The agent id whose context pipeline is invalid.
        agent: String,
        /// The offending transformer name.
        transformer: String,
        /// The unrecognized determinism string as authored.
        determinism: String,
    },

    /// Lowering encountered a `PromptEntry` variant it does not know how to
    /// lower (D7-B / ADR-0065). Replaces the former silent wildcard that
    /// mapped an unknown prompt kind to an empty prompt. Only reachable if a
    /// future `#[non_exhaustive]` `PromptEntry` variant is added upstream
    /// without a corresponding lowering arm — a fail-closed guard, not a
    /// user-authoring error.
    #[error("agent {agent:?}: unsupported prompt kind (this tau cannot lower {detail}); rebuild with a matching tau")]
    UnsupportedPromptKind {
        /// The agent whose prompt could not be lowered.
        agent: AgentId,
        /// Debug rendering of the offending `PromptEntry`.
        detail: String,
    },

    /// A `Condition::evaluates` is a `Locus::Output(id)` that names a step
    /// not yet visible at this point in the pipeline.
    #[error(
        "condition in step '{step}' references unknown or out-of-scope output step '{output}'"
    )]
    ConditionUnknownOutput {
        /// The pipeline step id containing the condition.
        step: String,
        /// The referenced step id that is missing or out of scope.
        output: String,
    },

    /// A `Suspend` step appeared below the top-level pipeline slice (inside a
    /// Branch arm, Loop body, or Parallel branch). Suspend is top-level only.
    #[error("suspend step {step:?} is nested inside a control-flow block; suspend is only allowed at the top level of the pipeline (EPIC 4.3)")]
    SuspendNotTopLevel {
        /// The nested suspend step id.
        step: String,
    },

    /// A step template references `${steps.<id>.output}` where `<id>` is a
    /// `Suspend` step, which produces no output.
    #[error("step {step:?} references {referenced:?}.output, but {referenced:?} is a suspend step and produces no output")]
    SuspendHasNoOutput {
        /// The referencing step.
        step: String,
        /// The suspend step id whose (nonexistent) output was referenced.
        referenced: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn unknown_determinism_renders_the_offending_values() {
        let e = LowerError::UnknownDeterminism {
            agent: "reviewer".into(),
            transformer: "trim_old".into(),
            determinism: "sometimes".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("reviewer"), "got: {msg}");
        assert!(msg.contains("trim_old"), "got: {msg}");
        assert!(msg.contains("sometimes"), "got: {msg}");
        assert!(msg.contains("pure"), "should name the valid set: {msg}");
    }

    // `UnsupportedPromptKind` is only reachable if a future
    // `#[non_exhaustive]` `PromptEntry` variant is added upstream without a
    // lowering arm; it cannot be triggered through the public lowering API
    // (the three current variants are all handled). This asserts its Display
    // is actionable so the fail-closed message is not vacuous.
    #[test]
    fn unsupported_prompt_kind_renders_actionably() {
        let e = LowerError::UnsupportedPromptKind {
            agent: AgentId("writer".into()),
            detail: "SomeFutureVariant".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("writer"), "got: {msg}");
        assert!(msg.contains("SomeFutureVariant"), "got: {msg}");
        assert!(msg.contains("rebuild"), "got: {msg}");
    }
}
