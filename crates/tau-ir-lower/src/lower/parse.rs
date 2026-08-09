//! First lowering stage: pull typed declarations out of `ProjectConfig`.
//!
//! This stage does no I/O and no resolution. It walks the
//! `ProjectConfig`'s agent/tool/step tables and produces an
//! in-memory `Parsed` value: a partially-populated `Workflow` whose
//! `ToolImpl::Native::content_hash` and similar resolution slots are
//! filled with zero bytes (the `resolve` stage fills them).

use alloc::collections::BTreeMap;
use tau_pkg::project::{PipelineRunRef, ProjectConfig};
use tau_pkg::{PromptEntry, ToolBody};

use crate::error::LowerError;
use tau_ir::capability::{CapabilityRequirements, CapabilityTable};
use tau_ir::check::{Check, CheckVerify, GoalPredicate, JudgeRef, Locus, OnFail, RetryPolicy};
use tau_ir::ids::{AgentId, CheckId, PipelineStepId, StepId, ToolId};
use tau_ir::module::Workflow;
use tau_ir::node::{Agent, Deterministic, Tool, ToolSpec};
use tau_ir::pipeline::{Pipeline, PipelineStep, StepRun};
use tau_ir::prompt::PromptSource;
use tau_ir::subflow::SubflowEdge;
use tau_ir::tool_impl::{Hash256, NativeFnRef, ToolImpl};
use tau_ir::trigger::{
    Backoff, BackoffStrategy, RetryPolicy as TriggerRetryPolicy, TriggerBinding, TriggerKind,
};
use tau_ir::AgentBudget;

/// Output of the parse stage.
#[derive(Debug)]
pub(super) struct Parsed {
    /// Partially-populated workflow (content hashes are zero pending `resolve`).
    pub(super) workflow: Workflow,
    /// Trigger bindings, canonically ordered by name (BTreeMap iteration).
    pub(super) triggers: alloc::vec::Vec<TriggerBinding>,
    /// Content-addressed asset blobs (currently `system_file` prompts) read
    /// at build time, keyed by hash (`"sha256:" + 64 hex`). Deduped by hash.
    pub(super) assets: BTreeMap<alloc::string::String, tau_ir::asset::AssetBlob>,
}

/// Run the parse stage on a `ProjectConfig`.
///
/// `prompt_file` reads an agent `system_file` prompt's bytes (D6-B); see
/// [`crate::Caches::prompt_file`].
pub(super) fn parse(
    config: &ProjectConfig,
    prompt_file: &dyn Fn(&std::path::Path) -> Result<alloc::vec::Vec<u8>, crate::PromptFileError>,
) -> Result<Parsed, LowerError> {
    let mut agents: BTreeMap<AgentId, Agent> = BTreeMap::new();
    let mut tools: BTreeMap<ToolId, Tool> = BTreeMap::new();
    let mut steps: BTreeMap<StepId, Deterministic> = BTreeMap::new();
    let mut assets: BTreeMap<alloc::string::String, tau_ir::asset::AssetBlob> = BTreeMap::new();
    let edges: alloc::vec::Vec<SubflowEdge> = alloc::vec::Vec::new();
    let mut capability_table: BTreeMap<ToolId, CapabilityRequirements> = BTreeMap::new();

    // --- Tools ---------------------------------------------------------
    //
    // `ProjectConfig::tools` is a `BTreeMap<String, ToolEntry>` produced by
    // tau-pkg::config. Each `ToolEntry` discriminates Native vs Mcp vs Subflow.
    for (name, entry) in config.tools.iter() {
        let tool_id = ToolId(name.clone());
        let caps = CapabilityRequirements {
            declared: entry.capabilities.clone(),
        };
        let impl_ = match &entry.body {
            ToolBody::Native(fn_name) => ToolImpl::Native {
                fn_ref: NativeFnRef {
                    name: fn_name.clone(),
                },
                // resolved by `resolve` stage; zero is a sentinel
                content_hash: Hash256::default(),
            },
            ToolBody::Mcp(url) => ToolImpl::Mcp {
                url: url.clone(),
                contract_hash: Hash256::default(),
                capability_subset: caps.clone(),
                // server_tool_name is empty at parse time; the resolve stage
                // expands this single entry into N entries (one per server-tool)
                // with real names filled in.
                server_tool_name: alloc::string::String::new(),
            },
            ToolBody::Subflow(target) => ToolImpl::Subflow {
                target: AgentId(target.clone()),
            },
            // `ToolBody` is `#[non_exhaustive]`; future variants are
            // forwarded as a parse error so existing callers aren't
            // silently broken when a new variant lands.
            _ => {
                return Err(LowerError::Parse("unsupported tool body variant".into()));
            }
        };
        let spec = ToolSpec {
            name: name.clone(),
            description: entry.description.clone(),
            input_schema: entry.input_schema.clone(),
        };
        capability_table.insert(tool_id.clone(), caps.clone());
        tools.insert(
            tool_id.clone(),
            Tool {
                id: tool_id,
                impl_,
                capabilities: caps,
                spec,
            },
        );
    }

    // --- Agents --------------------------------------------------------
    for (name, entry) in config.agents.iter() {
        let agent_id = AgentId(name.clone());
        let tool_refs = entry.tool_refs.iter().cloned().map(ToolId).collect();
        // Lower the prompt source (D6-B). `Inline` maps straight through.
        // `File` is read at build time via the injected `prompt_file` reader,
        // hashed, and emitted as a content-addressed `PromptSource::Asset`;
        // the bytes are collected into `assets` (deduped by hash). Paths never
        // enter the IR, and a missing/unreadable file is a hard build error —
        // this fixes the non-hermetic `system_file` bug (the IR used to carry
        // the path string as the prompt).
        let prompt = match &entry.prompt {
            PromptEntry::Inline(s) => PromptSource::inline(s.clone()),
            PromptEntry::File(p) => {
                let bytes = prompt_file(p).map_err(|e| LowerError::PromptFileUnreadable {
                    agent: agent_id.clone(),
                    path: p.to_string_lossy().into_owned(),
                    reason: e.0,
                })?;
                let hash = tau_ir::asset::asset_hash(&bytes);
                // Identical content across agents dedupes to one blob.
                assets
                    .entry(hash.clone())
                    .or_insert_with(|| tau_ir::asset::AssetBlob::prompt(bytes));
                PromptSource::asset(hash)
            }
            PromptEntry::None => PromptSource::inline(""),
            // `PromptEntry` is `#[non_exhaustive]`, so this wildcard is
            // required. A future variant reaching here is fail-closed (D7-B /
            // ADR-0065): it used to silently become an empty prompt.
            other => {
                return Err(LowerError::UnsupportedPromptKind {
                    agent: agent_id.clone(),
                    detail: alloc::format!("{other:?}"),
                });
            }
        };
        agents.insert(
            agent_id.clone(),
            Agent {
                id: agent_id,
                prompt,
                model_ref: resolve_model_ref(config, &entry.model)?,
                tool_refs,
                context: lower_context(name, entry)?,
                budget: AgentBudget {
                    max_turns: entry.max_turns,
                    max_tokens: entry.max_tokens,
                },
                produces: entry.produces.clone(),
                output_schema: entry.output_schema.clone(),
                durable: lower_durable(entry),
            },
        );
    }

    // --- Deterministic steps ------------------------------------------
    for (name, entry) in config.steps.iter() {
        let step_id = StepId(name.clone());
        steps.insert(
            step_id.clone(),
            Deterministic {
                id: step_id,
                fn_ref: NativeFnRef {
                    name: entry.fn_name.clone(),
                },
                input_schema: entry.input_schema.clone(),
                output_schema: entry.output_schema.clone(),
            },
        );
    }

    // --- Deterministic steps as tools ----------------------------------
    //
    // Each [steps.<name>] block registers BOTH a `Deterministic` node
    // (above) and a `Tool` with `ToolImpl::Step { id }`. The Tool
    // registration is what lets an agent reference the step in its
    // `tool_refs`; the Deterministic node is what the runtime registry
    // dispatches against at invoke time.
    for (name, entry) in config.steps.iter() {
        let step_id = StepId(name.clone());
        let tool_id = ToolId(name.clone());

        // Guard: a [tools.<name>] entry with the same key would be
        // silently overwritten, causing the wrong implementation to be
        // dispatched at runtime. Reject early with a clear error.
        if tools.contains_key(&tool_id) {
            return Err(LowerError::Parse(alloc::format!(
                "step name {name:?} collides with an existing tool name; \
                 rename the step or the tool"
            )));
        }

        let caps = CapabilityRequirements {
            declared: alloc::vec::Vec::new(),
        };
        let spec = ToolSpec {
            name: name.clone(),
            description: alloc::string::String::new(),
            input_schema: entry.input_schema.clone(),
        };
        capability_table.insert(tool_id.clone(), caps.clone());
        tools.insert(
            tool_id.clone(),
            Tool {
                id: tool_id,
                impl_: ToolImpl::Step { id: step_id },
                capabilities: caps,
                spec,
            },
        );
    }

    // --- Triggers (slice 1) --------------------------------------------
    let mut triggers: alloc::vec::Vec<TriggerBinding> = alloc::vec::Vec::new();
    for (name, entry) in config.triggers.iter() {
        let kind = match entry.kind.as_str() {
            "cron" => TriggerKind::Cron,
            "manual" => TriggerKind::Manual,
            // validate_trigger already rejected anything else; defensive.
            other => {
                return Err(LowerError::Parse(alloc::format!(
                    "trigger {name:?}: unsupported kind {other:?} reached lowering"
                )));
            }
        };
        let retry = match entry.retry.as_ref() {
            None => None,
            Some(r) => {
                let strategy = match r.backoff_strategy.as_str() {
                    "fixed" => BackoffStrategy::Fixed,
                    "exponential" => BackoffStrategy::Exponential,
                    // validate_trigger already rejected anything else; defensive.
                    other => {
                        return Err(LowerError::Parse(alloc::format!(
                            "trigger {name:?}: unsupported backoff strategy {other:?} reached lowering"
                        )));
                    }
                };
                Some(TriggerRetryPolicy {
                    max_attempts: r.max_attempts,
                    backoff: Backoff {
                        strategy,
                        base: r.backoff_base.clone(),
                        max: r.backoff_max.clone(),
                    },
                    dead_letter: r.dead_letter.clone(),
                })
            }
        };
        triggers.push(TriggerBinding {
            name: name.clone(),
            kind,
            agent: AgentId(entry.agent.clone()),
            schedule: entry.schedule.clone(),
            timezone: if entry.timezone.is_empty() {
                None
            } else {
                Some(entry.timezone.clone())
            },
            retry,
        });
    }

    // --- Pipeline ---------------------------------------------------------
    let mut pipeline = config.pipeline.as_ref().map(|p| Pipeline {
        steps: p.steps.iter().map(lower_step).collect(),
    });

    // --- Checks (goals + deliverables) -----------------------------------
    let checks = lower_checks(config, &mut pipeline)?;

    Ok(Parsed {
        workflow: Workflow {
            agents,
            tools,
            steps,
            edges,
            capability_table: CapabilityTable(capability_table),
            pipeline,
            checks,
        },
        triggers,
        assets,
    })
}

/// Test-only stub reader for `parse`: no file prompts. Callers that don't
/// exercise `system_file` prompts pass this so the reader is never invoked.
#[cfg(test)]
pub(crate) fn no_prompt_files(
    _: &std::path::Path,
) -> Result<alloc::vec::Vec<u8>, crate::PromptFileError> {
    Ok(alloc::vec::Vec::new())
}

/// Lower `[goals.*]`/`[deliverables.*]` into IR [`Check`]s and position a
/// `StepRun::Check` step for each.
///
/// Checks already placed explicitly via a `check:<id>` pipeline step keep
/// that position; the rest are appended at the pipeline tail in
/// deterministic order (all goals by id, then all deliverables by id). If
/// the config has no pipeline but does declare checks, a fresh pipeline is
/// synthesized to hold the appended check steps.
///
/// Producer binding and gate resolution were performed by tau-pkg's
/// validator (`DeliverableEntry::producer` / `::gate`), so this is a pure
/// structural copy — no re-derivation here.
/// Resolve a model alias to a concrete [`ModelRef`] via `[models]`, or via
/// `[allow.models]` when the project declares an `[allow]` ceiling (a governed
/// project moves its alias map under `[allow.models]`, per ADR-0057 / EPIC 1.2).
///
/// Infallible in practice — `validate_models` (tau-pkg) guarantees the alias
/// exists in one of the two tables before lowering runs; the error arm is
/// defense-in-depth.
fn resolve_model_ref(
    config: &ProjectConfig,
    alias: &str,
) -> Result<tau_ir::model_ref::ModelRef, LowerError> {
    let m = config
        .models
        .get(alias)
        .or_else(|| config.allow.as_ref().and_then(|a| a.models.get(alias)))
        .ok_or_else(|| {
            LowerError::Parse(alloc::format!(
                "model alias `{alias}` not in [models] or [allow.models]"
            ))
        })?;
    Ok(tau_ir::model_ref::ModelRef {
        backend: m.backend.clone(),
        model_id: m.model.clone(),
    })
}

fn lower_checks(
    config: &ProjectConfig,
    pipeline: &mut Option<Pipeline>,
) -> Result<BTreeMap<CheckId, Check>, LowerError> {
    let mut checks: BTreeMap<CheckId, Check> = BTreeMap::new();

    // Build the checks map, goals first then deliverables (BTreeMap order).
    for (id, goal) in config.goals.iter() {
        checks.insert(
            CheckId(id.clone()),
            Check {
                id: CheckId(id.clone()),
                verify: CheckVerify::Goal {
                    evaluates: lower_locus(&goal.evaluates),
                    predicate: lower_predicate(&goal.predicate),
                },
                // Goals always abort on failure; the gate is unused but
                // must be a valid step id — set it to the check's own id.
                retry: RetryPolicy {
                    on_fail: OnFail::Abort,
                    max_attempts: 1,
                    gate: PipelineStepId(id.clone()),
                },
            },
        );
    }
    for (id, d) in config.deliverables.iter() {
        // `gate` was resolved by tau-pkg validation (empty for abort);
        // fall back to the check's own id so the field is always valid.
        let gate = if d.gate.is_empty() {
            id.clone()
        } else {
            d.gate.clone()
        };
        checks.insert(
            CheckId(id.clone()),
            Check {
                id: CheckId(id.clone()),
                verify: CheckVerify::Deliverable {
                    locus: lower_locus(&d.locus),
                    must_satisfy: d.must_satisfy.clone(),
                    judge: lower_judge(config, d)?,
                },
                retry: RetryPolicy {
                    on_fail: lower_on_fail(&d.on_fail),
                    max_attempts: d.max_attempts,
                    gate: PipelineStepId(gate),
                },
            },
        );
    }

    if checks.is_empty() {
        return Ok(checks);
    }

    // Collect the check ids that are already explicitly positioned via a
    // `check:<id>` pipeline step — those are not auto-appended.
    let mut placed: alloc::collections::BTreeSet<alloc::string::String> =
        alloc::collections::BTreeSet::new();
    if let Some(p) = pipeline.as_ref() {
        for s in p.steps.iter() {
            if let StepRun::Check(CheckId(id)) = &s.run {
                placed.insert(id.clone());
            }
        }
    }

    // Append a synthetic check step for every check not already placed, in
    // deterministic order: goals by id, then deliverables by id.
    let mut appended: alloc::vec::Vec<PipelineStep> = alloc::vec::Vec::new();
    for id in config.goals.keys().chain(config.deliverables.keys()) {
        if placed.contains(id) {
            continue;
        }
        appended.push(PipelineStep {
            id: PipelineStepId(id.clone()),
            run: StepRun::Check(CheckId(id.clone())),
            input: "${input}".into(),
        });
    }

    match pipeline {
        Some(p) => p.steps.extend(appended),
        None => *pipeline = Some(Pipeline { steps: appended }),
    }

    Ok(checks)
}

/// Lower a tau-pkg [`AgentEntry`]'s `[agents.<id>.context]` pipeline into an
/// IR [`ContextConfig`]. Returns `None` when no context pipeline is declared
/// (the default behaviour — the agent loop applies no context transforms).
///
/// tau-pkg's validator shape-checks the custom-node `(source, package)` pairs
/// but does **not** constrain the determinism string (it passes any value
/// through via `unwrap_or_else(|| "pure")`). D7-B / ADR-0065 makes an
/// unrecognized determinism string a hard `LowerError::UnknownDeterminism`
/// here — the only build-time gate — instead of the former silent
/// `_ => DeterminismClass::Pure` downgrade.
fn lower_context(
    agent: &str,
    entry: &tau_pkg::project::AgentEntry,
) -> Result<Option<tau_ir::context::ContextConfig>, LowerError> {
    use tau_ir::context::{ContextConfig, ContextNodeKind, ContextStep, DeterminismClass};
    if entry.context.is_empty() {
        return Ok(None);
    }
    let mut pipeline = alloc::vec::Vec::with_capacity(entry.context.len());
    for s in entry.context.iter() {
        let determinism = match s.determinism.as_str() {
            "pure" => DeterminismClass::Pure,
            "llm_backed" => DeterminismClass::LlmBacked,
            "stateful" => DeterminismClass::Stateful,
            _ => {
                return Err(LowerError::UnknownDeterminism {
                    agent: agent.into(),
                    transformer: s.transformer.clone(),
                    determinism: s.determinism.clone(),
                });
            }
        };
        pipeline.push(ContextStep {
            transformer: s.transformer.clone(),
            determinism,
            kind: match &s.custom {
                Some((source, package)) => ContextNodeKind::Custom {
                    source: source.clone(),
                    package: package.clone(),
                },
                None => ContextNodeKind::Builtin,
            },
            config: s.config.clone(),
        });
    }
    // `ContextConfig` is `#[non_exhaustive]`; it cannot be built with a
    // struct literal from outside `tau-ir`. Build via `Default` + the
    // public `pipeline` field instead (behavior-identical).
    let mut cfg = ContextConfig::default();
    cfg.pipeline = pipeline;
    Ok(Some(cfg))
}

/// Convert a validated tau-pkg [`DurableEntry`] into the IR
/// [`tau_ir::durable::Durability`] (ADR-0053).
///
/// tau-pkg's validator guarantees the strings are known values, so the
/// mapping is total; the wildcard arms are defense-in-depth and resolve
/// to the A-minimal defaults.
pub fn durable_entry_to_ir(
    entry: &tau_pkg::project::project::DurableEntry,
) -> tau_ir::durable::Durability {
    use tau_ir::durable::{CheckpointGranularity, Durability, DurabilityIntent, DurableStore};
    use tau_pkg::project::project::DurableEntry;
    match entry {
        DurableEntry::Intent(s) => {
            // tau-pkg validated the string; the mapping is total.
            debug_assert_eq!(s, "survive-restarts");
            Durability::Intent(DurabilityIntent::SurviveRestarts)
        }
        DurableEntry::Explicit {
            checkpoint,
            store: _,
        } => {
            // tau-pkg validated both strings; wildcard arms are defence-in-depth.
            let checkpoint = match checkpoint.as_str() {
                "per_tool_call" => CheckpointGranularity::PerToolCall,
                _ => CheckpointGranularity::PerTurn,
            };
            // TODO: map store when DurableStore::Kv lands (A-full)
            let store = DurableStore::File;
            Durability::Explicit { checkpoint, store }
        }
    }
}

/// Lower the validated `[agents.<id>.durable]` block into an IR
/// [`tau_ir::durable::Durability`] (ADR-0053). `None` when the agent is
/// not durable.
fn lower_durable(entry: &tau_pkg::project::AgentEntry) -> Option<tau_ir::durable::Durability> {
    entry.durable.as_ref().map(durable_entry_to_ir)
}

/// Lower one validated pipeline step to an IR [`PipelineStep`], recursing
/// into `Branch` arms. Reference/scope integrity is validated separately by
/// the IR typecheck (`check_pipeline`), so this mapping stays infallible.
fn lower_step(s: &tau_pkg::project::PipelineStepConfig) -> PipelineStep {
    let run = match &s.run {
        PipelineRunRef::Agent(id) => StepRun::Agent(AgentId(id.clone())),
        PipelineRunRef::Tool(id) => StepRun::Tool(ToolId(id.clone())),
        PipelineRunRef::Deterministic(id) => StepRun::Deterministic(StepId(id.clone())),
        PipelineRunRef::Check(id) => StepRun::Check(CheckId(id.clone())),
        PipelineRunRef::Suspend { resume_signal } => StepRun::Suspend {
            resume_signal: resume_signal.clone(),
        },
        PipelineRunRef::Branch {
            on,
            then,
            otherwise,
        } => StepRun::Branch {
            on: lower_condition(on),
            then: then.iter().map(lower_step).collect(),
            otherwise: otherwise.iter().map(lower_step).collect(),
        },
        PipelineRunRef::Parallel { branches } => StepRun::Parallel {
            branches: branches
                .iter()
                .map(|b| b.iter().map(lower_step).collect())
                .collect(),
        },
        PipelineRunRef::Loop {
            body,
            until,
            max_iters,
        } => StepRun::Loop {
            body: body.iter().map(lower_step).collect(),
            until: lower_condition(until),
            max_iters: *max_iters,
        },
    };
    PipelineStep {
        id: PipelineStepId(s.id.clone()),
        run,
        input: s.input.clone(),
    }
}

/// Lower a tau-pkg [`ConditionConfig`](tau_pkg::project::ConditionConfig) to an
/// IR [`Condition`](tau_ir::check::Condition), reusing the same locus/predicate
/// mappings as `[goals.*]`.
fn lower_condition(c: &tau_pkg::project::ConditionConfig) -> tau_ir::check::Condition {
    tau_ir::check::Condition {
        evaluates: lower_locus(&c.evaluates),
        predicate: lower_predicate(&c.predicate),
    }
}

/// Map a tau-pkg [`LocusConfig`] to an IR [`Locus`].
fn lower_locus(locus: &tau_pkg::project::LocusConfig) -> Locus {
    use tau_pkg::project::LocusConfig;
    match locus {
        LocusConfig::Path(p) => Locus::Path(p.clone()),
        LocusConfig::Output(s) => Locus::Output(PipelineStepId(s.clone())),
    }
}

/// Map a tau-pkg [`GoalPredicateConfig`] to an IR [`GoalPredicate`].
fn lower_predicate(p: &tau_pkg::project::GoalPredicateConfig) -> GoalPredicate {
    use tau_pkg::project::GoalPredicateConfig;
    match p {
        GoalPredicateConfig::Exists => GoalPredicate::Exists,
        GoalPredicateConfig::NonEmpty => GoalPredicate::NonEmpty,
        GoalPredicateConfig::Equals(s) => GoalPredicate::Equals(s.clone()),
        GoalPredicateConfig::Matches(s) => GoalPredicate::Matches(s.clone()),
        GoalPredicateConfig::MinCount(n) => GoalPredicate::MinCount(*n),
        GoalPredicateConfig::SchemaValid(v) => GoalPredicate::SchemaValid(v.clone()),
        // Mirror how `Deterministic.fn_ref` is built above:
        // `NativeFnRef { name }`.
        GoalPredicateConfig::NativeFn(name) => {
            GoalPredicate::NativeFn(NativeFnRef { name: name.clone() })
        }
    }
}

/// Map a tau-pkg [`OnFailConfig`] to an IR [`OnFail`].
fn lower_on_fail(o: &tau_pkg::project::OnFailConfig) -> OnFail {
    use tau_pkg::project::OnFailConfig;
    match o {
        OnFailConfig::Abort => OnFail::Abort,
        OnFailConfig::Retry => OnFail::Retry,
    }
}

/// Map a tau-pkg [`JudgeConfig`] to an IR [`JudgeRef`], resolving the model.
///
/// The canonical judge (`Default`) resolves its model from `[models]`: an
/// explicit `judge_model` alias if present, otherwise it inherits the
/// producing agent's resolved model (Q4). The producer is resolved by
/// tau-pkg validation into `DeliverableEntry.producer`.
fn lower_judge(
    config: &ProjectConfig,
    d: &tau_pkg::project::DeliverableEntry,
) -> Result<JudgeRef, LowerError> {
    use tau_pkg::project::JudgeConfig;
    match &d.judge {
        JudgeConfig::Default { model } => {
            let model_ref = match model {
                Some(alias) => resolve_model_ref(config, alias)?,
                None => {
                    let producer = config.agents.get(&d.producer).ok_or_else(|| {
                        LowerError::Parse(alloc::format!(
                            "deliverable `{}` producer `{}` not found",
                            d.id,
                            d.producer
                        ))
                    })?;
                    resolve_model_ref(config, &producer.model)?
                }
            };
            Ok(JudgeRef::Default { model_ref })
        }
        JudgeConfig::Agent(n) => Ok(JudgeRef::Agent(AgentId(n.clone()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ir::tool_impl::ToolImpl;
    use tau_pkg::project::ProjectConfig;

    #[test]
    fn parse_registers_tool_node_for_subflow_body() {
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
model        = "default"
tool_refs    = ["notify"]

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
model        = "default"
tool_refs    = []

[tools.notify]
subflow     = "worker"
description = "Hand off to worker"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config, &no_prompt_files).expect("parse stage");

        let tool = parsed
            .workflow
            .tools
            .get(&ToolId("notify".into()))
            .expect("notify tool registered");
        assert!(
            matches!(&tool.impl_, ToolImpl::Subflow { target } if target.0 == "worker"),
            "expected ToolImpl::Subflow targeting worker; got {:?}",
            tool.impl_
        );
    }

    #[test]
    fn parse_registers_tool_node_for_each_step() {
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
model        = "default"
tool_refs    = ["normalize"]

[steps.normalize]
deterministic = "parse_celsius"
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config, &no_prompt_files).expect("parse stage");

        // Step registered in workflow.steps:
        assert!(parsed
            .workflow
            .steps
            .contains_key(&StepId("normalize".into())));

        // AND registered as a Tool with ToolImpl::Step:
        let tool = parsed
            .workflow
            .tools
            .get(&ToolId("normalize".into()))
            .expect("normalize tool registered");
        assert!(
            matches!(&tool.impl_, ToolImpl::Step { id } if id.0 == "normalize"),
            "expected ToolImpl::Step{{normalize}}; got {:?}",
            tool.impl_
        );
    }

    #[test]
    fn lowers_suspend_leaf() {
        // EPIC 4.3: a top-level `run = "suspend:<signal>"` step lowers to
        // `StepRun::Suspend { resume_signal }`.
        let toml = r#"
            [project]
            name = "p"
            [[pipeline.steps]]
            id = "pause"
            run = "suspend:go"
        "#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config, &no_prompt_files).expect("parse stage");

        let pipeline = parsed.workflow.pipeline.expect("pipeline lowered");
        assert_eq!(
            pipeline.steps[0].run,
            StepRun::Suspend {
                resume_signal: "go".into()
            }
        );
    }

    #[test]
    fn parse_rejects_step_tool_collision() {
        // A [tools.foo] and a [steps.foo] with the same name must be
        // rejected; if the insert were allowed the step would silently
        // overwrite the tool, dispatching the wrong implementation.
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
model        = "default"
tool_refs    = ["foo"]

[tools.foo]
native      = "Foo"
capabilities = []

[steps.foo]
deterministic = "do_foo"
"#;
        let config = ProjectConfig::parse_str(toml).expect("toml parse");
        let result = parse(&config, &no_prompt_files);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("parse stage should reject collision but returned Ok"),
        };
        match &err {
            LowerError::Parse(msg) => {
                assert!(
                    msg.contains("collide") || msg.contains("collision"),
                    "error message should mention collision; got: {msg:?}"
                );
            }
            other => panic!("expected LowerError::Parse; got {other:?}"),
        }
    }

    #[test]
    fn lowers_pipeline_steps_in_order() {
        let toml = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "a"
run = "agent:a"
input = "${input}"

[[pipeline.steps]]
id = "b"
run = "agent:b"
input = "${steps.a.output}"
"#;
        let config = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let parsed = parse(&config, &no_prompt_files).expect("parses");
        let pipe = parsed.workflow.pipeline.expect("pipeline present");
        assert_eq!(pipe.steps.len(), 2);
        assert_eq!(pipe.steps[0].id.0, "a");
        assert_eq!(pipe.steps[1].input, "${steps.a.output}");
    }

    #[test]
    fn lowers_parallel_step_to_nested_branches() {
        let toml = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "fanout"

  [[pipeline.steps.branches]]
    [[pipeline.steps.branches.steps]]
    id = "weather"
    run = "agent:weather"

  [[pipeline.steps.branches]]
    [[pipeline.steps.branches.steps]]
    id = "news"
    run = "agent:news"
    [[pipeline.steps.branches.steps]]
    id = "digest"
    run = "agent:digest"
    input = "${steps.news.output}"
"#;
        let config = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let parsed = parse(&config, &no_prompt_files).expect("parses");
        let pipe = parsed.workflow.pipeline.expect("pipeline present");
        match &pipe.steps[0].run {
            StepRun::Parallel { branches } => {
                assert_eq!(branches.len(), 2);
                assert_eq!(branches[0].len(), 1);
                assert_eq!(
                    branches[0][0].run,
                    StepRun::Agent(AgentId("weather".into()))
                );
                assert_eq!(branches[1].len(), 2);
                assert_eq!(branches[1][1].input, "${steps.news.output}");
            }
            other => panic!("expected StepRun::Parallel, got {other:?}"),
        }
    }

    #[test]
    fn lowers_loop_step_with_until_and_bound() {
        let toml = r#"
[project]
name = "demo"

[[pipeline.steps]]
id = "refine"
until = { evaluates = "steps.draft.output", check = "matches", pattern = "APPROVED" }
max_iters = 4

  [[pipeline.steps.body]]
  id = "draft"
  run = "agent:writer"
  input = "${input}"
"#;
        let config = tau_pkg::project::ProjectConfig::parse_str(toml).unwrap();
        let parsed = parse(&config, &no_prompt_files).expect("parses");
        let pipe = parsed.workflow.pipeline.expect("pipeline present");
        match &pipe.steps[0].run {
            StepRun::Loop {
                body,
                until,
                max_iters,
            } => {
                assert_eq!(*max_iters, 4);
                assert_eq!(
                    until.evaluates,
                    Locus::Output(PipelineStepId("draft".into()))
                );
                assert_eq!(until.predicate, GoalPredicate::Matches("APPROVED".into()));
                assert_eq!(body.len(), 1);
                assert_eq!(body[0].run, StepRun::Agent(AgentId("writer".into())));
            }
            other => panic!("expected StepRun::Loop, got {other:?}"),
        }
    }

    #[test]
    fn parse_emits_no_subflow_edge_for_subflow_body() {
        // v0 routes subflow dispatch through ToolImpl::Subflow exclusively;
        // SubflowEdge is reserved for SubflowKind::Compose (future). This
        // test pins the new shape so a regression that re-introduces the
        // edge gets caught.
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.parent]
display_name = "Parent"
package      = "p@^0.1"
model        = "default"
tool_refs    = ["notify"]

[agents.worker]
display_name = "Worker"
package      = "p@^0.1"
model        = "default"
tool_refs    = []

[tools.notify]
subflow     = "worker"
description = "Hand off to worker"
capabilities = []
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let parsed = parse(&config, &no_prompt_files).expect("parse stage");
        assert!(
            parsed.workflow.edges.is_empty(),
            "expected no SubflowEdge entries; got {:?}",
            parsed.workflow.edges
        );
    }

    #[test]
    fn lower_durable_maps_per_tool_call() {
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "p@^0.1"
model        = "default"
tool_refs    = []

[agents.a.durable]
checkpoint = "per_tool_call"
store      = "file"
"#;
        let config = ProjectConfig::parse_str(toml).expect("parse");
        let agent_entry = config.agents.get("a").expect("agent a");
        let durable = super::lower_durable(agent_entry).expect("durable present");
        assert_eq!(
            durable,
            tau_ir::durable::Durability::Explicit {
                checkpoint: tau_ir::durable::CheckpointGranularity::PerToolCall,
                store: tau_ir::durable::DurableStore::File,
            }
        );
    }

    #[test]
    fn lower_durable_maps_intent() {
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "p"

[models]
default = { backend = "mock-llm", model = "mock-model" }

[agents.a]
display_name = "A"
package      = "p@^0.1"
model        = "default"
tool_refs    = []
durable      = "survive-restarts"
"#;
        let project = tau_pkg::project::project::ProjectConfig::parse_str(toml).expect("parse");
        let agent_entry = project.agents.get("a").expect("agent a");
        let durable = super::lower_durable(agent_entry).expect("durable present");
        assert_eq!(
            durable,
            tau_ir::durable::Durability::Intent(tau_ir::durable::DurabilityIntent::SurviveRestarts)
        );
    }
}
