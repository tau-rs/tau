//! Workflow runner: dispatches each step in order.
//!
//! For v1, the runner is linear. Each step's output feeds future steps'
//! `${steps.<id>.output}` templates. A failed step aborts the run.
//! Persistence is append-only JSONL (see `crate::persistence`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tau_domain::{
    Address, AgentDefinition, AgentInstanceId, Message, MessagePayload, PackageManifest,
};
use tau_runtime::Runtime;

use crate::error::WorkflowError;
use crate::model::{StepKind, Workflow};
use crate::persistence::{run_log_path, RunLog, StepRecord, StepStatus};
use crate::template::resolve as resolve_template;

/// One workflow execution.
pub struct Runner {
    runtime: Arc<Runtime>,
    scope_root: PathBuf,
}

/// Per-run options.
///
/// # Examples
///
/// Build a fresh `RunOpts` for a first run with no prior completed steps:
///
/// ```
/// use tau_workflow::RunOpts;
///
/// let opts = RunOpts {
///     input: "analyse the Q3 report".into(),
///     run_id: None,
///     completed: vec![],
///     agents: std::collections::BTreeMap::new(),
/// };
///
/// assert_eq!(opts.input, "analyse the Q3 report");
/// assert!(opts.run_id.is_none());
/// assert!(opts.completed.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// Caller-supplied input string available as `${input}`.
    pub input: String,
    /// Optional pre-existing run id (used by `--resume`). When `None`,
    /// a fresh ULID is allocated.
    pub run_id: Option<String>,
    /// Already-completed step records (from replay). The runner skips
    /// any step whose id appears here with status Ok.
    pub completed: Vec<StepRecord>,
    /// Agent definitions resolved from tau.toml by the caller.
    pub agents: BTreeMap<String, (AgentDefinition, PackageManifest)>,
}

/// Outcome of a single run invocation.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// The run id (ULID).
    pub run_id: String,
    /// Path to the JSONL log on disk.
    pub log_path: PathBuf,
    /// Whether every step completed with status Ok.
    pub success: bool,
    /// The last step's output (or empty on failure).
    pub last_output: String,
}

impl Runner {
    /// Construct a runner.
    ///
    /// # Examples
    ///
    /// (`no_run` because `Runtime::builder().build()` requires a registered
    /// LLM backend and project config that are only available at runtime.)
    ///
    /// ```no_run
    /// use tau_workflow::Runner;
    /// use tau_runtime::Runtime;
    /// use std::sync::Arc;
    /// use std::path::PathBuf;
    ///
    /// # tokio_test::block_on(async {
    /// let runtime = Arc::new(
    ///     Runtime::builder().build().expect("runtime should build"),
    /// );
    /// let runner = Runner::new(runtime, PathBuf::from("/workspace"));
    /// // runner is ready to execute workflows against the given scope root.
    /// # });
    /// ```
    pub fn new(runtime: Arc<Runtime>, scope_root: PathBuf) -> Self {
        Self {
            runtime,
            scope_root,
        }
    }

    /// Run (or resume) a workflow.
    pub async fn run(
        &self,
        workflow: &Workflow,
        opts: RunOpts,
    ) -> Result<RunOutcome, WorkflowError> {
        let run_id = opts
            .run_id
            .clone()
            .unwrap_or_else(|| ulid::Ulid::new().to_string());
        let log_path = run_log_path(&self.scope_root, &workflow.name, &run_id);
        let mut log = RunLog::open_for_write(&log_path).await?;

        // Seed prior_outputs from completed records (resume path).
        let mut prior_outputs: BTreeMap<String, String> = BTreeMap::new();
        let mut completed_ids: BTreeMap<String, ()> = BTreeMap::new();
        for record in &opts.completed {
            if record.status == StepStatus::Ok {
                prior_outputs.insert(record.step_id.clone(), record.output.clone());
                completed_ids.insert(record.step_id.clone(), ());
            }
        }

        let mut last_output = String::new();

        for (idx, step) in workflow.steps.iter().enumerate() {
            if completed_ids.contains_key(&step.id) {
                last_output = prior_outputs.get(&step.id).cloned().unwrap_or_default();
                continue;
            }

            let started_at = Utc::now();
            let (input_str, output_result) = match &step.kind {
                StepKind::AgentRun { agent, input } => {
                    let resolved_input = resolve_template(
                        input,
                        &opts.input,
                        &prior_outputs,
                        &workflow.name,
                        &step.id,
                    )?;
                    let (agent_def, manifest) =
                        opts.agents
                            .get(agent)
                            .ok_or_else(|| WorkflowError::AgentNotFound {
                                workflow: workflow.name.clone(),
                                step_id: step.id.clone(),
                                agent: agent.clone(),
                            })?;
                    let initial_message = build_user_message(resolved_input.clone());
                    let result = self
                        .runtime
                        .run(
                            agent_def.clone(),
                            manifest.clone(),
                            initial_message,
                            tau_runtime::RunOptions::default(),
                        )
                        .await;
                    (resolved_input, agent_outcome_to_string(result))
                }
                StepKind::ToolCall { tool, args } => {
                    // Resolve any template strings inside args (string values only).
                    let resolved_args =
                        resolve_args(args, &opts.input, &prior_outputs, &workflow.name, &step.id)?;
                    let default_agent_id = workflow.default_agent.as_ref().ok_or_else(|| {
                        WorkflowError::ParseFailed {
                            path: workflow.source_path.clone(),
                            message: "tool.call step requires [workflow].default-agent".into(),
                        }
                    })?;
                    let (agent_def, manifest) =
                        opts.agents.get(default_agent_id).ok_or_else(|| {
                            WorkflowError::AgentNotFound {
                                workflow: workflow.name.clone(),
                                step_id: step.id.clone(),
                                agent: default_agent_id.clone(),
                            }
                        })?;
                    // Convert serde_json::Value → tau_domain::Value for invoke_tool.
                    let tau_args = json_to_tau_value(&resolved_args);
                    // Inherent `invoke_tool` on the kernel Runtime — returns
                    // tau_runtime_core::error::RuntimeError. We map below into
                    // `tool_outcome_to_string` which accepts either flavor.
                    let result = self
                        .runtime
                        .invoke_tool(agent_def, manifest, tool, tau_args)
                        .await;
                    let input_repr = serde_json::to_string(&resolved_args).unwrap_or_default();
                    (input_repr, tool_outcome_to_string(result))
                }
            };

            let ended_at = Utc::now();
            let duration_ms = (ended_at - started_at).num_milliseconds().max(0) as u64;

            let (output, status, error, detail) = match output_result {
                Ok(out) => (out, StepStatus::Ok, None, None),
                Err((err_kind, err_detail)) => (
                    String::new(),
                    StepStatus::Failed,
                    Some(err_kind),
                    Some(err_detail),
                ),
            };

            let record = StepRecord {
                ts: ended_at,
                run_id: run_id.clone(),
                step_id: step.id.clone(),
                step_index: idx,
                kind: match &step.kind {
                    StepKind::AgentRun { .. } => "agent.run".into(),
                    StepKind::ToolCall { .. } => "tool.call".into(),
                },
                input: input_str,
                output: output.clone(),
                started_at,
                ended_at,
                duration_ms,
                status,
                error,
                detail,
            };
            log.append(&record).await?;

            if status == StepStatus::Failed {
                return Ok(RunOutcome {
                    run_id,
                    log_path,
                    success: false,
                    last_output: output,
                });
            }

            prior_outputs.insert(step.id.clone(), output.clone());
            last_output = output;
        }

        Ok(RunOutcome {
            run_id,
            log_path,
            success: true,
            last_output,
        })
    }
}

/// Construct a `Message` representing the user's initial input.
///
/// Uses `Message::new` with `Address::User` sender and a fresh
/// `Address::Agent(AgentInstanceId::new())` recipient — matching the
/// same pattern used by `tau-cli/src/cmd/run.rs`. The kernel replaces
/// the recipient placeholder with its own `AgentInstanceId` once the
/// loop starts.
fn build_user_message(text: String) -> Message {
    Message::new(
        Address::User,
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text { content: text },
    )
}

/// Extract the final assistant text from a `RunOutcome`.
///
/// For `RunOutcome::Completed`, returns the text content of `final_message`.
/// For `RunOutcome::Failed`, returns an error tuple with kind `"agent_failed"`
/// and the status debug string.
fn agent_outcome_to_string(
    result: Result<tau_runtime::RunOutcome, tau_runtime::error::CoreRuntimeError>,
) -> Result<String, (String, String)> {
    match result {
        Ok(tau_runtime::RunOutcome::Completed { final_message, .. }) => {
            Ok(extract_text_from_payload(&final_message.payload))
        }
        Ok(tau_runtime::RunOutcome::Failed { status, .. }) => {
            Err(("agent_failed".into(), format!("{status:?}")))
        }
        // #[non_exhaustive] forward-compat wildcard
        Ok(_) => Err((
            "unknown_outcome".into(),
            "unknown RunOutcome variant".into(),
        )),
        Err(e) => Err(("runtime_error".into(), format!("{e}"))),
    }
}

/// Project a [`MessagePayload`] to a text string. Non-text payloads
/// fall back to a `Debug`-formatted preview, matching `tau-cli`'s
/// `format_message_text` helper.
fn extract_text_from_payload(payload: &MessagePayload) -> String {
    match payload {
        MessagePayload::Text { content } => content.clone(),
        other => format!("{other:?}"),
    }
}

/// Convert a `tau_ports::ToolResult` to a string for the JSONL output column.
///
/// On `is_error = true`, the step is surfaced as failed so the run aborts.
/// On success, the content blocks are serialized as a JSON string.
fn tool_outcome_to_string(
    result: Result<tau_ports::ToolResult, tau_runtime::error::CoreRuntimeError>,
) -> Result<String, (String, String)> {
    match result {
        Ok(tool_result) => {
            if tool_result.is_error {
                // Tool returned an error result. Surface as a failed step.
                return Err((
                    "tool_error".into(),
                    serde_json::to_string(&tool_result).unwrap_or_default(),
                ));
            }
            // Success — render the content as a JSON string for the log.
            Ok(serde_json::to_string(&tool_result).unwrap_or_default())
        }
        Err(e) => Err(("runtime_error".into(), format!("{e}"))),
    }
}

/// Convert `serde_json::Value` → `tau_domain::Value`.
///
/// `tau_domain::Value` has no `From<serde_json::Value>` impl, so we
/// pattern-match each variant manually.
///
/// - `serde_json::Number`: integers fit in `i64` → `Value::Integer`;
///   everything else → `Value::Float`.
/// - `serde_json::Null` → `Value::Null`.
/// - Array/Object recurse.
fn json_to_tau_value(value: &serde_json::Value) -> tau_domain::Value {
    match value {
        serde_json::Value::Null => tau_domain::Value::Null,
        serde_json::Value::Bool(b) => tau_domain::Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                tau_domain::Value::Integer(i)
            } else {
                tau_domain::Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => tau_domain::Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            tau_domain::Value::Array(arr.iter().map(json_to_tau_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out = std::collections::BTreeMap::new();
            for (k, v) in map {
                out.insert(k.clone(), json_to_tau_value(v));
            }
            tau_domain::Value::Object(out)
        }
    }
}

fn resolve_args(
    args: &serde_json::Value,
    input: &str,
    prior: &BTreeMap<String, String>,
    workflow: &str,
    step_id: &str,
) -> Result<serde_json::Value, WorkflowError> {
    match args {
        serde_json::Value::String(s) => {
            let resolved = resolve_template(s, input, prior, workflow, step_id)?;
            Ok(serde_json::Value::String(resolved))
        }
        serde_json::Value::Array(arr) => {
            let resolved: Result<Vec<_>, _> = arr
                .iter()
                .map(|v| resolve_args(v, input, prior, workflow, step_id))
                .collect();
            Ok(serde_json::Value::Array(resolved?))
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), resolve_args(v, input, prior, workflow, step_id)?);
            }
            Ok(serde_json::Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// Compare a workflow's current step ids against a log's recorded step
/// ids. Returns Err(DriftDetected) when they diverge; returns Ok(()) when
/// the log's ids are a prefix of the workflow's (the resume case).
///
/// "Prefix match" means: every record in `logged_records` (in order)
/// matches the corresponding step in `workflow.steps`. Trailing steps in
/// the workflow that aren't yet in the log are fine (that's the work
/// the resume will do).
///
/// # Examples
///
/// A partial log (prefix match) is accepted for `--resume`:
///
/// ```
/// use tau_workflow::{check_drift, Workflow, WorkflowError};
/// use tau_workflow::{StepRecord, StepStatus};
/// use std::path::Path;
/// use chrono::Utc;
///
/// let src = r#"
/// [[steps]]
/// id = "step-a"
/// kind = "agent.run"
/// agent = "helper"
/// input = "${input}"
///
/// [[steps]]
/// id = "step-b"
/// kind = "agent.run"
/// agent = "helper"
/// input = "${steps.step-a.output}"
/// "#;
/// let workflow = Workflow::from_str(src, Path::new("workflows/demo.toml"))
///     .expect("workflow TOML should parse");
///
/// // Only step-a has been logged so far; step-b is still pending.
/// let now = Utc::now();
/// let logged = vec![StepRecord {
///     ts: now,
///     run_id: "01HKZTEST".into(),
///     step_id: "step-a".into(),
///     step_index: 0,
///     kind: "agent.run".into(),
///     input: "hello".into(),
///     output: "world".into(),
///     started_at: now,
///     ended_at: now,
///     duration_ms: 10,
///     status: StepStatus::Ok,
///     error: None,
///     detail: None,
/// }];
///
/// assert!(check_drift(&workflow, &logged).is_ok(), "prefix match should be accepted");
///
/// // A mismatched step id triggers DriftDetected.
/// let mut drifted = logged.clone();
/// drifted[0].step_id = "WRONG".into();
/// assert!(matches!(
///     check_drift(&workflow, &drifted),
///     Err(WorkflowError::DriftDetected { .. })
/// ));
/// ```
pub fn check_drift(
    workflow: &Workflow,
    logged_records: &[StepRecord],
) -> Result<(), WorkflowError> {
    if logged_records.len() > workflow.steps.len() {
        return Err(WorkflowError::DriftDetected {
            logged: logged_records.iter().map(|r| r.step_id.clone()).collect(),
            current: workflow.steps.iter().map(|s| s.id.clone()).collect(),
        });
    }
    for (idx, record) in logged_records.iter().enumerate() {
        if workflow.steps[idx].id != record.step_id {
            return Err(WorkflowError::DriftDetected {
                logged: logged_records.iter().map(|r| r.step_id.clone()).collect(),
                current: workflow.steps.iter().map(|s| s.id.clone()).collect(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod drift_tests {
    use super::*;
    use chrono::Utc;

    fn step(id: &str, agent: &str) -> crate::model::Step {
        crate::model::Step {
            id: id.into(),
            kind: crate::model::StepKind::AgentRun {
                agent: agent.into(),
                input: String::new(),
            },
        }
    }

    fn workflow_with_steps(steps: Vec<crate::model::Step>) -> Workflow {
        Workflow {
            name: "t".into(),
            source_path: PathBuf::from("t.toml"),
            description: None,
            default_agent: None,
            steps,
        }
    }

    fn record(idx: usize, step_id: &str) -> StepRecord {
        let now = Utc::now();
        StepRecord {
            ts: now,
            run_id: "01HK".into(),
            step_id: step_id.into(),
            step_index: idx,
            kind: "agent.run".into(),
            input: String::new(),
            output: String::new(),
            started_at: now,
            ended_at: now,
            duration_ms: 0,
            status: StepStatus::Ok,
            error: None,
            detail: None,
        }
    }

    #[test]
    fn prefix_match_is_ok() {
        let wf = workflow_with_steps(vec![step("a", "x"), step("b", "y"), step("c", "z")]);
        let records = vec![record(0, "a"), record(1, "b")];
        check_drift(&wf, &records).unwrap();
    }

    #[test]
    fn full_match_is_ok() {
        let wf = workflow_with_steps(vec![step("a", "x")]);
        let records = vec![record(0, "a")];
        check_drift(&wf, &records).unwrap();
    }

    #[test]
    fn mismatched_id_is_drift() {
        let wf = workflow_with_steps(vec![step("a", "x"), step("b", "y")]);
        let records = vec![record(0, "a"), record(1, "WRONG")];
        let err = check_drift(&wf, &records).unwrap_err();
        assert!(matches!(err, WorkflowError::DriftDetected { .. }));
    }

    #[test]
    fn extra_logged_records_are_drift() {
        let wf = workflow_with_steps(vec![step("a", "x")]);
        let records = vec![record(0, "a"), record(1, "b")];
        let err = check_drift(&wf, &records).unwrap_err();
        assert!(matches!(err, WorkflowError::DriftDetected { .. }));
    }
}
